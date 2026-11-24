use crate::{domain::Run, process::home};
use anyhow::{Context, Result, ensure};
use std::{
    env, fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

pub struct RunId(String);

impl RunId {
    fn parse(value: &str) -> Option<Self> {
        (!value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            .then(|| Self(value.to_owned()))
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub struct Store {
    pub root: PathBuf,
}
pub struct LockedRun {
    pub dir: PathBuf,
    pub root: PathBuf,
    _lock: fs::File,
}
impl LockedRun {
    fn try_acquire(dir: PathBuf, root: &Path) -> Result<Option<Self>> {
        let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("run.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { dir, root: root.to_owned(), _lock: file })),
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
                env::var_os("HERDER_STATE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home().join(".local/state/herder")),
            )?,
        })
    }
    pub fn resolve(&self, value: &str) -> Result<RunId> {
        if let Some(id) = RunId::parse(value)
            && self.dir(&id).is_dir()
        {
            return Ok(id);
        }
        let mut matches = vec![];
        let mut similar = vec![];
        for id in self.runs()?.into_iter().flatten() {
            if let Ok(run) = self.load(&id) {
                if run.name == value {
                    matches.push(id);
                } else if run.name.contains(value) {
                    similar.push(format!("{id} ({})", run.name.escape_debug()));
                }
            }
        }
        ensure!(
            matches.len() == 1,
            "run {value:?}: {} matches; {}",
            matches.len(),
            match (matches.is_empty(), similar.is_empty()) {
                (false, _) =>
                    format!("candidates: {}", matches.iter().map(|id| id.0.as_str()).collect::<Vec<_>>().join(", ")),
                (true, false) => format!("similar names: {}", similar.join(", ")),
                (true, true) => "list runs with `herder ls`".into(),
            }
        );
        Ok(matches.remove(0))
    }
    pub fn dir(&self, id: &RunId) -> PathBuf {
        self.root.join("runs").join(&id.0)
    }
    pub fn create(&self) -> Result<(RunId, LockedRun)> {
        let mut bytes = [0; 2];
        fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let (y, mo, d, h, mi, s) = utc(SystemTime::now());
        let id = RunId(format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{:02x}{:02x}", bytes[0], bytes[1]));
        fs::create_dir_all(self.root.join("runs"))?;
        let dir = self.root.join("runs").join(format!(".{id}.pending"));
        fs::create_dir(&dir)?;
        Ok((id, LockedRun::try_acquire(dir, &self.root)?.context("new run is already locked")?))
    }
    pub fn publish(&self, locked: &mut LockedRun, run: &Run, dir: &Path) -> Result<()> {
        locked.save(run)?;
        fs::rename(&locked.dir, dir)?;
        locked.dir = dir.to_owned();
        Ok(())
    }
    pub fn lock(&self, id: &RunId) -> Result<LockedRun> {
        self.lock_until(id, None)
    }
    pub fn lock_until(&self, id: &RunId, deadline: Option<Instant>) -> Result<LockedRun> {
        let lock_limit = Instant::now() + Duration::from_secs(2);
        let (deadline, timeout) = match deadline {
            Some(end) if end <= lock_limit => (end, "verification deadline elapsed"),
            _ => (lock_limit, "timed out after 2s"),
        };
        loop {
            if let Some(locked) = self.try_lock(id)? {
                return Ok(locked);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "run {id} busy: {timeout} waiting for run.lock");
            std::thread::sleep(Duration::from_millis(20).min(remaining));
        }
    }
    pub fn try_lock(&self, id: &RunId) -> Result<Option<LockedRun>> {
        LockedRun::try_acquire(self.dir(id), &self.root).with_context(|| format!("run {id}"))
    }
    pub fn load(&self, id: &RunId) -> Result<Run> {
        load(&self.dir(id))
    }
    pub fn runs(&self) -> Result<Vec<Result<RunId, String>>> {
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
        Ok(runs.into_iter().map(|name| RunId::parse(&name).ok_or(name)).collect())
    }
}
pub fn load(dir: &Path) -> Result<Run> {
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
    timestamp_at(SystemTime::now())
}
pub fn timestamp_at(clock: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc(clock);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}
fn utc(clock: SystemTime) -> (i32, u8, u8, u8, u8, u8) {
    let secs = clock.duration_since(SystemTime::UNIX_EPOCH).expect("clock after 1970").as_secs();
    let date = time::OffsetDateTime::from_unix_timestamp(secs.try_into().expect("clock outside supported range"))
        .expect("clock outside supported range");
    (date.year(), u8::from(date.month()), date.day(), date.hour(), date.minute(), date.second())
}
pub fn copy(source: &Path, dest: &Path) -> Result<PathBuf> {
    atomic(dest, &fs::read(source)?)?;
    Ok(dest.to_path_buf())
}

pub fn repo(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    ensure!(path.is_dir(), "repo must be a directory");
    let top = crate::process::checked(
        std::process::Command::new("git").arg("-C").arg(&path).args(["rev-parse", "--show-toplevel"]),
    );
    if let Ok(top) = top {
        let top = fs::canonicalize(top.trim())?;
        ensure!(top == path, "--repo must be the repository root: {}", top.display());
    }
    Ok(path)
}
pub fn report(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_preserves_calendar_boundaries_and_truncates_to_seconds() {
        for (secs, expected) in [
            (0, (1970, 1, 1, 0, 0, 0)),
            (86_399, (1970, 1, 1, 23, 59, 59)),
            (86_400, (1970, 1, 2, 0, 0, 0)),
            (68_169_600, (1972, 2, 29, 0, 0, 0)),
            (951_782_400, (2000, 2, 29, 0, 0, 0)),
            (951_868_800, (2000, 3, 1, 0, 0, 0)),
            (4_107_542_399, (2100, 2, 28, 23, 59, 59)),
            (4_107_542_400, (2100, 3, 1, 0, 0, 0)),
            (13_574_563_200, (2400, 2, 29, 0, 0, 0)),
            (253_402_300_799, (9999, 12, 31, 23, 59, 59)),
        ] {
            assert_eq!(utc(SystemTime::UNIX_EPOCH + Duration::new(secs, 999_999_999)), expected, "{secs}");
        }
    }

    #[test]
    #[should_panic(expected = "clock after 1970")]
    fn utc_rejects_pre_epoch_clocks() {
        utc(SystemTime::UNIX_EPOCH - Duration::from_nanos(1));
    }
}
