<!--
The title becomes the commit message on main, so make it a Conventional Commit with the
crate as scope, such as `feat(pdf): write xref streams` or `fix(ffi): free buffer on
error path`. Open the pull request as a draft. CONTRIBUTING.md and CLAUDE.md describe
the rules; outside contributors agree to the Contributor License Agreement (CLA.md) by
posting the comment it asks for.
-->

## What changed

<!-- In plain language, without Rust jargon. -->

## Why

<!-- The milestone, plan or issue this implements (`Closes #…`). -->

## Risks

<!-- What could break, and for whom. -->

## Deviations from CLAUDE.md

<!-- None, or each deviation and the change to CLAUDE.md that records it. -->

## New dependencies

<!-- None, or each with its justification and the `cargo deny check` result. -->

## Corpus numbers

<!-- Before and after, when the change touches a reader or a writer. -->

## How to check by hand

<!-- Exact commands, and what to look at in their output. -->

---

Definition of done (CONTRIBUTING.md):

- [ ] `scripts/check.sh` passes locally.
- [ ] New behaviour has tests. A bug fix has a regression test that failed before the fix.
- [ ] Every new public item has a doc comment. Non-trivial public APIs have a runnable doc
      example.
- [ ] A new parser comes with a fuzz target in `fuzz/`.
- [ ] Every public Rust feature that users need is exposed through the C ABI in the same
      pull request, with the header regenerated and covered by `tests/c` and
      `tests/ffi-python`.
- [ ] No check is weakened to make it pass: no `#[allow]` without a `// reason:` comment, no
      skipped tests, no lowered lint levels.
