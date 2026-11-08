use anyhow::{Context, Result, ensure};
use std::path::Path;

/// Reads a plain continuation document at the command's input boundary.
/// The source bytes are returned exactly; nothing is trimmed or rewritten.
pub fn document(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read continuation document {}", path.display()))?;
    let text =
        String::from_utf8(bytes).with_context(|| format!("continuation document {} is not UTF-8", path.display()))?;
    ensure!(!text.trim().is_empty(), "continuation document {} is empty or whitespace-only", path.display());
    Ok(text)
}

/// Builds the durable run input: the standard continuation work contract followed by
/// the source document verbatim. The result must pass the shared brief linter in the
/// selected mode before launch.
pub fn prompt(mode: &str, document: &str) -> String {
    let read_only = if mode == "review" {
        "This is a review-mode handoff: stay strictly read-only and report findings; change nothing.\n"
    } else {
        ""
    };
    let mut prompt = include_str!("handoff_prompt.txt").replace("{read_only}", read_only);
    prompt.push_str(document);
    prompt
}
