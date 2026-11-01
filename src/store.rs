use crate::{
    domain::Run,
    process::{checked, home},
};
use anyhow::{Result, ensure};
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

pub struct Store {
    pub root: PathBuf,
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
    pub fn dir(&self, id: &str) -> Result<PathBuf> {
        ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "invalid run id");
        Ok(self.root.join("runs").join(id))
    }
    pub fn create(&self) -> Result<(String, PathBuf)> {
        let mut bytes = [0; 2];
        fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let date = checked(Command::new("date").args(["-u", "+%Y%m%d-%H%M%S"]))?;
        let id = format!("{}-{:02x}{:02x}", date.trim(), bytes[0], bytes[1]);
        fs::create_dir_all(self.root.join("runs"))?;
        let dir = self.dir(&id)?;
        fs::create_dir(&dir)?;
        Ok((id, fs::canonicalize(dir)?))
    }
    pub fn save(&self, run: &Run) -> Result<()> {
        atomic(&self.dir(&run.id)?.join("run.json"), &serde_json::to_vec_pretty(run)?)
    }
    pub fn load(&self, id: &str) -> Result<Run> {
        let run: Run = serde_json::from_slice(&fs::read(self.dir(id)?.join("run.json"))?)?;
        ensure!(run.id == id, "run.json identity mismatch");
        Ok(run)
    }
    pub fn runs(&self) -> Result<Vec<Run>> {
        let path = self.root.join("runs");
        if !path.exists() {
            return Ok(vec![]);
        }
        let mut runs = vec![];
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                runs.push(self.load(&entry.file_name().to_string_lossy())?);
            }
        }
        runs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(runs)
    }
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
