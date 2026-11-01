use crate::{
    domain::Run,
    process::{checked, home},
};
use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub struct Store {
    pub root: PathBuf,
}
// flock is available on the supported Unix hosts; std File locking needs Rust 1.89.
unsafe extern "C" {
    fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}
pub struct LockedRun {
    pub dir: PathBuf,
    _lock: fs::File,
}
impl LockedRun {
    fn try_acquire(dir: PathBuf) -> Result<Option<Self>> {
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("run.lock"))?;
        // SAFETY: this guard owns the descriptor; LOCK_EX | LOCK_NB never waits.
        while unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        Ok(Some(Self { dir, _lock: file }))
    }
    pub fn load(&self) -> Result<Run> {
        load(&self.dir)
    }
    pub fn save(&self, run: &Run) -> Result<()> {
        atomic(&self.dir.join("run.json"), &serde_json::to_vec_pretty(run)?)
    }
}
impl Store {
    pub fn new() -> Result<Self> {
        Ok(Self {
            root: std::path::absolute(
                env::var_os("CREW_STATE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join(".local/state/ai-skills/crew")),
            )?,
        })
    }
    pub fn resolve(&self, value: &str) -> Result<String> {
        if self.dir(value).is_ok_and(|dir| dir.is_dir()) {
            return Ok(value.to_owned());
        }
        let mut matches = vec![];
        let mut candidates = vec![];
        for id in self.runs()? {
            if let Ok(run) = self.load(&id) {
                candidates.push(format!("{id} ({})", run.name.escape_debug()));
                if run.name == value {
                    matches.push(id);
                }
            }
        }
        ensure!(
            matches.len() == 1,
            "run {value:?}: {} matches; candidates: {}",
            matches.len(),
            if matches.is_empty() { candidates.join(", ") } else { matches.join(", ") }
        );
        Ok(matches.remove(0))
    }
    pub fn dir(&self, id: &str) -> Result<PathBuf> {
        ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "invalid run id");
        Ok(self.root.join("runs").join(id))
    }
    pub fn create(&self) -> Result<(String, LockedRun)> {
        let mut bytes = [0; 2];
        fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let date = checked(Command::new("date").args(["-u", "+%Y%m%d-%H%M%S"]))?;
        let id = format!("{}-{:02x}{:02x}", date.trim(), bytes[0], bytes[1]);
        fs::create_dir_all(self.root.join("runs"))?;
        let dir = self.root.join("runs").join(format!(".{id}.pending"));
        fs::create_dir(&dir)?;
        Ok((id, LockedRun::try_acquire(dir)?.context("new run is already locked")?))
    }
    pub fn publish(&self, locked: &mut LockedRun, run: &Run) -> Result<()> {
        locked.save(run)?;
        let dir = self.dir(&run.id)?;
        fs::rename(&locked.dir, &dir)?;
        locked.dir = dir;
        Ok(())
    }
    pub fn lock(&self, id: &str) -> Result<LockedRun> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(locked) = self.try_lock(id)? {
                return Ok(locked);
            }
            ensure!(Instant::now() < deadline, "run {id} busy: timed out after 2s waiting for run.lock");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn try_lock(&self, id: &str) -> Result<Option<LockedRun>> {
        LockedRun::try_acquire(self.dir(id)?).with_context(|| format!("run {id}"))
    }
    pub fn load(&self, id: &str) -> Result<Run> {
        load(&self.dir(id)?)
    }
    pub fn runs(&self) -> Result<Vec<String>> {
        let path = self.root.join("runs");
        if !path.exists() {
            return Ok(vec![]);
        }
        let mut runs = vec![];
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                runs.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        runs.sort();
        Ok(runs)
    }
}
fn load(dir: &Path) -> Result<Run> {
    let path = dir.join("run.json");
    let run: Run = serde_json::from_slice(&fs::read(&path).with_context(|| path.display().to_string())?)
        .with_context(|| path.display().to_string())?;
    ensure!(Some(run.id.as_ref()) == dir.file_name(), "{}: identity mismatch", path.display());
    Ok(run)
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)?;
    Ok(())
}
pub fn timestamp() -> Result<String> {
    Ok(checked(Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]))?.trim().into())
}
pub fn copy(source: &Path, dest: &Path) -> Result<PathBuf> {
    atomic(dest, &fs::read(source)?)?;
    Ok(dest.to_path_buf())
}

pub fn repo(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    ensure!(path.is_dir(), "repo must be a directory");
    Ok(path)
}
pub fn report(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
