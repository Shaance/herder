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
    format!(
        "<task>\n\
         You are the receiving worker of a continuation handoff. The outgoing coordinator must stop editing the\n\
         task before launching you; Crew cannot verify that external prerequisite. Read the complete continuation\n\
         document at the end of this input first, then inspect the current repository state and the applicable instructions (AGENTS.md and any skills\n\
         the document names) before acting.\n\
         {read_only}\
         </task>\n\
         <workstream_contract>\n\
         The continuation document below is the whole transferred context: this launch does not transfer any hidden\n\
         conversation, does not stop another writer, does not change Board task owners, and the launch result alone\n\
         does not prove the context was understood. Preserve the user decisions, constraints and remaining work it\n\
         records; verify its claims against the actual repository artifacts before relying on them; continue only the\n\
         remaining work it authorizes. Do not weaken or override explicit authorizations in the document; if it lacks\n\
         essential scope, ask the user instead of inventing it. Rejected alternative: scraping the coordinator's\n\
         transcript or generating context with a model, because neither is durable input owned by the receiver.\n\
         </workstream_contract>\n\
         <implementation_authority>\n\
         Continue the authorized remaining work directly. Authorized paths recorded at launch (--path) are the\n\
         verification scope; with no --path the existing scope behavior applies, not broader permission.\n\
         </implementation_authority>\n\
         <action_safety>\n\
         Do not commit or push, publish, release, install, change credentials or external systems, launch real\n\
         workers, or delegate unless the continuation document or the user expressly authorized it.\n\
         </action_safety>\n\
         <default_follow_through_policy>\n\
         Complete the authorized remaining work and its meaningful verification, then stop; do not expand scope\n\
         beyond what the document authorizes.\n\
         </default_follow_through_policy>\n\
         <verification_loop>\n\
         Run the checks the document names and verify its claims and your own changes against the actual artifacts.\n\
         </verification_loop>\n\
         <economy_contract>\n\
         Make the smallest coherent change the remaining work requires.\n\
         </economy_contract>\n\
         <structured_output_contract>\n\
         In your final report, state what you read back from the continuation document, the resulting journey,\n\
         changed paths, tests actually run, exact limitations, and any remaining blockers.\n\
         </structured_output_contract>\n\
         \n\
         ----- continuation document (verbatim source bytes) -----\n\
         \n\
         {document}"
    )
}
