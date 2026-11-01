use crate::store::{Store, atomic, timestamp};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs};

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Logged {
        id: String,
        title: String,
        observed: String,
        expected: String,
        evidence: Vec<String>,
        area: String,
        run: Option<String>,
        timestamp: String,
    },
    Resolved {
        id: String,
        note: String,
        timestamp: String,
    },
}
#[derive(Serialize)]
pub struct Issue {
    pub log: Event,
    pub resolution: Option<Event>,
}
fn events(store: &Store) -> Result<Vec<Event>> {
    let path = store.root.join("issues.jsonl");
    if !path.exists() {
        return Ok(vec![]);
    }
    fs::read_to_string(path)?.lines().map(|s| Ok(serde_json::from_str(s)?)).collect()
}
pub fn list(store: &Store, open: bool) -> Result<Vec<Issue>> {
    let mut issues = BTreeMap::new();
    for event in events(store)? {
        match &event {
            Event::Logged { id, .. } => {
                issues.insert(id.clone(), Issue { log: event, resolution: None });
            }
            Event::Resolved { id, .. } => {
                let issue =
                    issues.get_mut(id).ok_or_else(|| anyhow::anyhow!("resolution references missing issue {id}"))?;
                issue.resolution = Some(event);
            }
        }
    }
    Ok(issues.into_values().filter(|i| !open || i.resolution.is_none()).collect())
}
pub fn append(store: &Store, event: Event) -> Result<Event> {
    fs::create_dir_all(&store.root)?;
    let path = store.root.join("issues.jsonl");
    let mut bytes = if path.exists() { fs::read(&path)? } else { vec![] };
    bytes.extend(serde_json::to_vec(&event)?);
    bytes.push(b'\n');
    atomic(&path, &bytes)?;
    Ok(event)
}
pub fn next_id(store: &Store) -> Result<String> {
    Ok(format!("issue-{:04}", list(store, false)?.len() + 1))
}
pub fn resolve(store: &Store, id: String, note: String) -> Result<Event> {
    ensure!(
        list(store, false)?.iter().any(|i| matches!(&i.log, Event::Logged { id: existing, .. } if existing == &id)),
        "unknown issue {id}"
    );
    append(store, Event::Resolved { id, note, timestamp: timestamp() })
}
