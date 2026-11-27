use crate::process::OSC7577_TOKEN;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use signal_hook::{
    consts::{SIGCONT, SIGINT},
    iterator::{Handle, Signals},
    low_level::emulate_default_handler,
};
use std::{
    env,
    fs::File,
    io::{self, IsTerminal, Write},
    os::fd::AsRawFd,
    path::Path,
    thread::{self, JoinHandle},
    time::SystemTime,
};

pub struct Resume {
    signals: Handle,
    thread: Option<JoinHandle<()>>,
    reset_interrupt: bool,
}

pub struct Terminal {
    terminal: File,
    token: String,
}

struct Reporter {
    target: Terminal,
    command: String,
}

impl Terminal {
    pub fn open() -> Option<Self> {
        if !io::stdout().is_terminal() {
            return None;
        }
        let token = env::var(OSC7577_TOKEN).ok().filter(|token| valid_token(token))?;
        let terminal = File::options().write(true).open("/dev/tty").ok()?;
        Some(Self { terminal, token })
    }

    pub fn register(self, dir: &Path) -> Option<Resume> {
        let exe = env::current_exe().ok()?;
        let args = [exe.to_str()?, "wait", "--run-dir", dir.to_str()?];
        if args.iter().any(|arg| arg.chars().any(char::is_control)) {
            return None;
        }
        let argv = args.map(|arg| STANDARD.encode(arg)).join(",");
        let command = format!("argv={argv}");
        let packet = report(&self.token, &command, std::process::id(), SystemTime::now())?;
        let mut reporter = Reporter { target: self, command };
        // Let an inherited SIGINT ignore stay in effect.
        let mut disposition = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: sigaction writes a complete action on success and does not install a handler here.
        let default_interrupt = unsafe {
            libc::sigaction(SIGINT, std::ptr::null(), disposition.as_mut_ptr()) == 0
                && disposition.assume_init().sa_sigaction == libc::SIG_DFL
        };
        let mut signals = Signals::new(if default_interrupt { &[SIGCONT, SIGINT][..] } else { &[SIGCONT] }).ok()?;
        let mut registered = foreground(&reporter.target.terminal);
        if registered && reporter.target.terminal.write_all(&packet).is_err() {
            drop(signals);
            restore_interrupt(default_interrupt);
            return None;
        }
        let handle = signals.handle();
        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                if signal == SIGINT {
                    if registered {
                        reporter.send("clear=1");
                    }
                    let _ = emulate_default_handler(SIGINT);
                    return;
                }
                if foreground(&reporter.target.terminal) {
                    registered |= reporter.refresh();
                }
            }
            // A normal return closes the iterator. Hangup and termination bypass it.
            if registered {
                reporter.send("clear=1");
            }
        });
        Some(Resume { signals: handle, thread: Some(thread), reset_interrupt: default_interrupt })
    }
}

impl Drop for Resume {
    fn drop(&mut self) {
        self.signals.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        restore_interrupt(self.reset_interrupt);
    }
}

impl Reporter {
    fn refresh(&mut self) -> bool {
        let Some(packet) = report(&self.target.token, &self.command, std::process::id(), SystemTime::now()) else {
            return false;
        };
        self.target.terminal.write_all(&packet).is_ok()
    }

    fn send(&mut self, fields: &str) {
        if let Some(packet) = report(&self.target.token, fields, std::process::id(), SystemTime::now()) {
            let _ = self.target.terminal.write_all(&packet);
        }
    }
}

fn foreground(terminal: &File) -> bool {
    // SAFETY: the open file owns the descriptor, and both calls only read process-group IDs.
    unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) == libc::getpgrp() }
}

fn restore_interrupt(changed: bool) {
    if changed {
        // SAFETY: this CLI owns its temporary SIGINT handler and the previous action was default.
        unsafe { libc::signal(SIGINT, libc::SIG_DFL) };
    }
}

fn valid_token(token: &str) -> bool {
    (22..=64).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

fn report(token: &str, fields: &str, pid: u32, clock: SystemTime) -> Option<Vec<u8>> {
    let time = clock.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let body = format!("pid={pid}:time={time}:{fields}");
    let mac = Sha256::digest(format!("{body}:{token}").as_bytes());
    let osc = format!("\x1b]7577;{body}:mac={mac:x}\x1b\\");
    (osc.len() <= 2048).then(|| osc.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rejects_empty_short_long_or_malformed_tokens() {
        for invalid in ["", "short-token", "valid-length:bad-char-", &"x".repeat(65)] {
            assert!(!valid_token(invalid));
        }
        assert!(valid_token("abcde-fghij_0123456789"));
        assert!(valid_token(&"x".repeat(64)));
    }

    #[test]
    fn signs_the_published_resume_protocol_example() {
        let fields = "argv=cmVzdW1lLXdyYXA=,c3No,ZGV2Ym94";
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1795543200);
        let packet = report("dummy-token-replace-me", fields, 90412, time).unwrap();
        assert_eq!(
            packet,
            b"\x1b]7577;pid=90412:time=1795543200:argv=cmVzdW1lLXdyYXA=,c3No,ZGV2Ym94:mac=e76149d0ba54a83570ffab0583867dacdfa23928ba9d18604f0badea88b62b65\x1b\\"
        );
    }
}
