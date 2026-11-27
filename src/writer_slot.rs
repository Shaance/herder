use crate::store::{self, Store};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Serializes protected admissions for one worktree. The last owner survives the process that launched it.
/// The lock file's inode is stable while the owner record is atomically replaced.
pub struct Slot {
    _lock: fs::File,
    owner_file: PathBuf,
}

impl Slot {
    pub fn lock(store: &Store, canonical_repo: &Path) -> Result<Self> {
        let dir = store.root.join("writers");
        fs::create_dir_all(&dir)?;
        let id = format!("{:x}", Sha256::digest(canonical_repo.as_os_str().as_encoded_bytes()));
        let lock =
            fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join(format!("{id}.lock")))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    bail!("another Herder process is updating this checkout's protected writer")
                }
                Err(fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
                Err(fs::TryLockError::Error(error)) => return Err(error).context("lock protected checkout"),
            }
        }
        Ok(Self { _lock: lock, owner_file: dir.join(format!("{id}.owner")) })
    }

    pub fn owner(&self) -> Result<Option<String>> {
        match fs::read_to_string(&self.owner_file) {
            Ok(owner) => Ok(Some(owner)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("read protected checkout owner"),
        }
    }

    pub fn assign(&self, run_id: &str) -> Result<()> {
        store::atomic(&self.owner_file, run_id.as_bytes()).context("record protected checkout owner")
    }
}
