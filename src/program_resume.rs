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
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::SystemTime,
};

pub struct Resume {
    reporter: Arc<Mutex<Reporter>>,
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
    publication: Publication,
}

#[derive(PartialEq, Eq)]
enum Publication {
    Withdrawn,
    Eligible,
    Published,
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
        let mut reporter = Reporter { target: self, command, publication: Publication::Eligible };
        // Let an inherited SIGINT ignore stay in effect.
        let mut disposition = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: sigaction writes a complete action on success and does not install a handler here.
        let default_interrupt = unsafe {
            libc::sigaction(SIGINT, std::ptr::null(), disposition.as_mut_ptr()) == 0
                && disposition.assume_init().sa_sigaction == libc::SIG_DFL
        };
        let mut signals = Signals::new(if default_interrupt { &[SIGCONT, SIGINT][..] } else { &[SIGCONT] }).ok()?;
        if foreground(&reporter.target.terminal) {
            if reporter.target.terminal.write_all(&packet).is_err() {
                drop(signals);
                restore_interrupt(default_interrupt);
                return None;
            }
            reporter.publication = Publication::Published;
        }
        let handle = signals.handle();
        let reporter = Arc::new(Mutex::new(reporter));
        let signal_reporter = Arc::clone(&reporter);
        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                let mut reporter = signal_reporter.lock().unwrap();
                if signal == SIGINT {
                    reporter.withdraw();
                    // Keep the guard until the process exits so the waiter cannot republish after clear.
                    let _ = emulate_default_handler(SIGINT);
                    return;
                }
                reporter.refresh();
            }
            // A normal return closes the iterator. Hangup and termination bypass it.
            signal_reporter.lock().unwrap().withdraw();
        });
        Some(Resume { reporter, signals: handle, thread: Some(thread), reset_interrupt: default_interrupt })
    }
}

impl Resume {
    pub fn set_eligible(&self, eligible: bool) {
        let mut reporter = self.reporter.lock().unwrap();
        if !eligible {
            reporter.withdraw();
        } else if reporter.publication == Publication::Withdrawn {
            reporter.publication = Publication::Eligible;
            reporter.refresh();
        }
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
    fn refresh(&mut self) {
        if self.publication != Publication::Withdrawn
            && foreground(&self.target.terminal)
            && let Some(packet) = report(&self.target.token, &self.command, std::process::id(), SystemTime::now())
            && self.target.terminal.write_all(&packet).is_ok()
        {
            self.publication = Publication::Published;
        }
    }

    fn withdraw(&mut self) {
        if self.publication == Publication::Published
            && let Some(packet) = report(&self.target.token, "clear=1", std::process::id(), SystemTime::now())
        {
            let _ = self.target.terminal.write_all(&packet);
        }
        self.publication = Publication::Withdrawn;
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
