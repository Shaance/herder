use crate::{domain::Run, process::home};
use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

pub struct Store {
    pub root: PathBuf,
}
pub struct LockedRun {
    pub dir: PathBuf,
    _lock: fs::File,
}
impl LockedRun {
    fn try_acquire(dir: PathBuf) -> Result<Option<Self>> {
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("run.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { dir, _lock: file })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
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
        let (y, mo, d, h, mi, s) = utc_now();
        let id = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{:02x}{:02x}", bytes[0], bytes[1]);
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
pub fn timestamp() -> String {
    let (y, mo, d, h, mi, s) = utc_now();
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}
/// Current UTC civil time; days-to-date conversion follows Howard Hinnant's `civil_from_days`.
fn utc_now() -> (i64, u64, u64, u64, u64, u64) {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("clock after 1970").as_secs();
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe as i64 + era * 400 + i64::from(m <= 2), m, d, rem / 3_600, rem % 3_600 / 60, rem % 60)
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
