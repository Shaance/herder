<task>
Independent read-only review of the crew MVP that Codex implemented. Repo: current directory. The change under
review is `git diff REVIEW_BASE..REVIEW_HEAD` (both commits are named at the bottom of this brief). The contract
it had to meet is `docs/brief-mvp.md`; the product design is `docs/design.md`.

Load and apply these skills, in this order, and say in the report which ones you actually loaded:
1. `$write-rust`: Rust ownership, error handling, state modeling, module boundaries.
2. `$make-states-unrepresentable`: find states the types permit but the domain forbids (structs of `Option`
   fields whose valid combinations are fewer than the type allows, stringly-typed states, bools that encode
   modes, raw executor output passed inward instead of parsed once at the boundary). Run it in finder mode
   only: report, do not restructure.
3. `$prove-before-hardening`: for every guard, retry, lock, cache, fallback or defensive check that the
   acceptance criteria do not require, ask whether its premise is proven.

Also check, explicitly:
- Over-engineering: abstractions (traits, generics, wrapper types, helper modules) with fewer than three real
  uses; config or flags nobody asked for; test-only hooks beyond `CREW_STATE_DIR` and `CREW_SKILLS_DIR`;
  code beyond the contract's command list.
- Clean architecture: the three layers in the brief (domain without IO ← adapters/store ← CLI/render).
  Report every import that points the wrong way (domain using `std::process`, `std::fs`, `clap`, printing).
  Files over ~250 lines; any `utils`/`helpers`/`manager` module.
- Contract fit: every command and flag in `docs/brief-mvp.md` exists and behaves as stated; Vibe and Codex
  adapters call the existing scripts instead of re-implementing them; Claude adapter puts the prompt first.
- Tests: does each test fail if the production behavior it names is reverted? Any test asserting on mock
  behavior instead of crew's observable output?
</task>

<workstream_contract>
Read-only. Output is a findings report only.
Rejected alternative: none, no new surface.
</workstream_contract>

<implementation_authority>
Reviewer only. You may run `cargo build --offline`, `cargo test --offline` and `cargo clippy --offline` to
check claims. Do not edit, create or delete any file in the repo.
</implementation_authority>

<action_safety>
Do not edit files, commit, push, install, or launch real Claude, Vibe or Codex workers.
Do not load implementer or delegation skills.
</action_safety>

<default_follow_through_policy>
Keep going on low-risk interpretation. Report a finding only with file:line evidence you read.
</default_follow_through_policy>

<verification_loop>
Before finishing: `git status --short` shows no changes made by you.
For every finding: cite file:line, and for type-state findings show the invalid value the current type allows.
</verification_loop>

<economy_contract>
Report findings, not rewrites. Each finding proposes the smallest fix and its rough line delta. Prefer fixes
that delete code. Do not propose new abstractions unless they delete more than they add.
</economy_contract>

<structured_output_contract>
Under 80 lines. Verdict first: ACCEPT / ACCEPT WITH FIXES / REJECT. Then findings ranked by severity
(P1 wrong behavior, P2 invalid state representable or layer violation, P3 over-engineering or style), each with:
file:line, what is wrong, the invalid state or failure scenario, smallest fix, line delta. Then: skills loaded,
commands run with results, and an economy verdict (new artifacts that should be deleted, duplicated truths).
</structured_output_contract>
