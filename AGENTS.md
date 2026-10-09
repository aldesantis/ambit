# AGENTS.md

Guidance for AI agents working in this repository.

## Documentation

### The readme

`README.md` is the only user-facing document. There is no `docs/` directory, and adding one is not
the fix for a long readme: fold the content in or cut it.

A readme answers three questions, in this order:

1. **What is this tool?**
2. **Why does it exist?**
3. **How do I use it?**

Everything in the file has to serve one of those three. If a paragraph serves none, delete it.

**Do not write:**

- **Internals.** How resolution is implemented, what is recorded in `.ambit/state.json`, how ambit
  identifies its own entries in a JSON array, which order the resolver walks in. A user does not
  read the readme to learn the algorithm.
- **Historical evolution.** What a feature replaced, what the syntax used to be, why a leftover file
  from an older version is refused. The readme documents the tool as it is today. Git history holds
  the rest.
- **Design justification.** Arguments defending a decision against alternatives the reader never
  proposed: why an entry names its namespace, why two lists instead of one, why there is no
  `ambit catalog` command. State the rule, show an example, move on.
- **Editorializing.** "That is deliberately the author's call." "Which is the answer you wanted."
  "The trade is deliberate." Describe behavior, not your feelings about it.

**Do write:**

- Working examples with real output. A reader copies these.
- Tables for every field, flag, and exit code. Reference material earns its length.
- One short sentence of rationale where a rule would otherwise look arbitrary. One sentence, not a
  section.

**Style:**

- Plain sentences. No em dashes.
- Behavior a user can observe, in the words they would use for it.
- Update the readme in the same change that alters the behavior it documents.

## Code comments

Ambit is an uncommented codebase. Names, types and tests carry the meaning; comments do not.

Write a comment only for behavior that would be extremely hard for a human or an agent to infer
from the code: an ordering requirement whose violation causes a subtle bug, a workaround for an
external tool or OS quirk, the hidden meaning of a sentinel value, or a warning against an obvious
change that would break something. Keep it to one or two plain lines stating the fact.

Do not write doc comments, module headers, section banners, test descriptions, rationale for
ordinary design choices, or comments that restate the code.

## Rust conventions

- **Checks.** Every change passes `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test`, the same three CI runs on Linux, macOS and Windows. The toolchain is pinned in
  `rust-toolchain.toml`.
- **Formatting is rustfmt's.** Do not hand-format around it. Within what rustfmt leaves alone,
  separate logically distinct steps in a function body with a blank line, and keep early returns
  as their own `if` blocks rather than folding them into long expression chains.
- **Lints.** Clippy runs with `pedantic` on. The few allowed pedantic lints are listed, with their
  reason, under `[lints.clippy]` in `Cargo.toml`. Add to that list only for a lint that fires on
  most of the codebase; otherwise fix the code, or put a local `#[allow]` on the item.
- **Process state stays at the edge.** `clippy.toml` disallows reading or mutating environment
  variables, changing the working directory, `std::process::exit`, `std::fs::read_dir` and
  `std::fs::read_to_string`. `main.rs` reads the environment and cwd once and passes them down as
  `util::env::Env` and a path; the filesystem is reached through `util::fs`, which keeps directory
  listings permutable for the determinism tests. `#[allow(clippy::disallowed_methods)]` is allowed
  only in `main.rs`, `src/util` and test code. Tests run in parallel threads, so nothing may mutate
  process-wide state.
- **Order is observable.** Use `IndexMap`/`IndexSet` wherever iteration order reaches output, and
  `util::cmp::js_cmp` for every string sort, so output is identical on every machine.
- **Tests.** Unit tests live next to their module (`mod tests` or a sibling `tests.rs`). CLI,
  fixture and golden-file tests live in `tests/`. `UPDATE_GOLDEN=1 cargo test` regenerates
  `tests/golden/`.
