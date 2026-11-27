use crate::store::Store;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

/// An immutable receipt, shared by all CLI and MCP processes using this store.
/// A published claim survives a disconnect even if its owner never publishes the run.
pub struct DispatchRequest {
    receipt: PathBuf,
    payload_hash: String,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    run_id: String,
    payload_hash: String,
}

#[derive(Debug)]
pub struct Conflict;

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("this request ID already belongs to a different dispatch in this checkout")
    }
}

impl std::error::Error for Conflict {}

impl DispatchRequest {
    pub fn new(store: &Store, repo: &Path, key: &str, payload: &impl Serialize) -> Result<Self> {
        ensure!(
            !key.is_empty() && key.len() <= 200 && !key.chars().any(char::is_control),
            "request id must be 1-200 bytes without control characters"
        );
        // A canonical filesystem path cannot contain NUL, so path and key have an unambiguous boundary.
        let address =
            Sha256::new().chain_update(repo.as_os_str().as_encoded_bytes()).chain_update([0]).chain_update(key);
        let name = format!("{:x}.json", address.finalize());
        let mut digest = Sha256::new();
        serde_json::to_writer(&mut digest, payload)?;
        Ok(Self { receipt: store.root.join("requests").join(name), payload_hash: format!("{:x}", digest.finalize()) })
    }

    pub fn existing(&self) -> Result<Option<String>> {
        let bytes = match fs::read(&self.receipt) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read dispatch request receipt"),
        };
        let receipt: Receipt = serde_json::from_slice(&bytes).context("invalid dispatch request receipt")?;
        ensure!(receipt.payload_hash == self.payload_hash, Conflict);
        Ok(Some(receipt.run_id))
    }

    /// Returns None to the one process authorized to publish and launch this run.
    pub fn claim(&self, run_id: &str) -> Result<Option<String>> {
        let dir = self.receipt.parent().context("dispatch request receipt has no parent directory")?;
        fs::create_dir_all(dir)?;
        let mut pending = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(
            &mut pending,
            &Receipt { run_id: run_id.to_owned(), payload_hash: self.payload_hash.clone() },
        )?;
        pending.flush()?;
        pending.as_file().sync_all()?;
        match pending.persist_noclobber(&self.receipt) {
            Ok(_) => {
                fs::File::open(dir)?.sync_all()?;
                Ok(None)
            }
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                let owner = self.existing()?.context("dispatch request receipt disappeared during claim")?;
                Ok(Some(owner))
            }
            Err(error) => Err(error.error).context("claim dispatch request"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn concurrent_claims_choose_one_owner_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store { root: dir.path().to_owned() };
        let make = || DispatchRequest::new(&store, Path::new("/repo"), "review/42", &("high", "brief")).unwrap();
        let gate = Barrier::new(5);
        let owners = std::thread::scope(|scope| {
            (0..5)
                .map(|i| {
                    let gate = &gate;
                    let request = make();
                    scope.spawn(move || {
                        let candidate = format!("run-{i}");
                        assert!(request.existing().unwrap().is_none());
                        gate.wait();
                        let prior = request.claim(&candidate).unwrap();
                        (candidate, prior)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        let winners: Vec<_> = owners.iter().filter(|(_, prior)| prior.is_none()).collect();
        assert_eq!(winners.len(), 1);
        let winner = &winners[0].0;
        assert!(owners.iter().filter_map(|(_, prior)| prior.as_ref()).all(|id| id == winner));
        assert_eq!(make().existing().unwrap().as_ref(), Some(winner));
        let changed = DispatchRequest::new(&store, Path::new("/repo"), "review/42", &("low", "brief")).unwrap();
        assert!(changed.claim("run-new").unwrap_err().is::<Conflict>());
        let sibling = DispatchRequest::new(&store, Path::new("/other"), "review/42", &("high", "brief")).unwrap();
        assert_eq!(sibling.claim("independent").unwrap(), None);
    }
}
