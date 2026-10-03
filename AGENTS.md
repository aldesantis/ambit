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

### Everything else

Comments in the source are where internals belong. A comment explaining why the code is shaped a
certain way is good and should stay in the code, not migrate to the readme.

## Code comments

Ambit is a commented codebase, but comments must earn their place. A comment exists to tell the
reader something the code cannot: keep the reasoning, cut the rhetoric.

### What to write

- **Doc comments on public items.** Rustdoc (`///`), one-sentence summary first. Describe the
  contract from the caller's side: constraints, return semantics, side effects, and an `# Errors`
  section naming the exit code of each error the function returns. Skip the doc comment entirely
  when the name already says everything.
- **Why, not what.** Rationale for a non-obvious decision, ordering requirements, invariants,
  units, what a sentinel value means, why the obvious alternative was rejected.
- **Negative information.** What is deliberately absent ("no lock here: callers already hold it"),
  so a future "fix" doesn't reintroduce a bug.
- **Module headers only for real design.** A short `//!` block stating the module's design
  decisions and invariants, once. Most files need no header. Never repeat in the header what per-symbol docs
  already say.

### What not to write

- **Editorializing.** State the fact; don't argue for it or perform it. No flourishes, metaphors,
  or persuasion ("that is the point", "a standing bet that...", "the kind of waste a cache exists
  to avoid"). If a comment reads like an essay, cut it to the fact it contains.
- **Restating the code.** No doc comment that rephrases the symbol name
  (`/// Where skills live.` on `SKILLS_DIRNAME: &str = "skills"`). Delete, don't decorate.
- **Play-by-play.** Never narrate what the next line does.
- **Reviewer-directed commentary.** No comments explaining why a change is correct or what the code
  did before. That belongs in the PR description.
- **Commented-out code, change journals, section banners.**

### Style

- Short, plain, factual sentences. Capitalized and punctuated.
- Prefer separate sentences over clauses chained with em dashes.
- One canonical explanation per decision; elsewhere, point to it with an intra-doc link
  (``[`resolve`]``) instead of retelling it.
- Reference constants by name (``[`STALE_THRESHOLD`]``), never restate their value in prose.
- When changing code, update or delete every adjacent comment your change touches.

## Rust conventions

- **Layout.** A Cargo workspace. `crates/ambit-core` is the engine library; its default `cli`
  feature adds the CLI surface and self-update. `crates/ambit` is the `ambit` binary, a thin
  `main.rs`. `crates/ambit-ffi` holds the UniFFI bindings the macOS app links, built against core
  without `cli`.
- **Checks.** Every change passes `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  and `cargo test` from the workspace root, the same three CI runs on Linux, macOS and Windows. CI
  also runs `cargo clippy -p ambit-core --no-default-features --all-targets -- -D warnings`. The
  toolchain is pinned in `rust-toolchain.toml`.
- **Formatting is rustfmt's.** Do not hand-format around it. Within what rustfmt leaves alone,
  separate logically distinct steps in a function body with a blank line, and keep early returns
  as their own `if` blocks rather than folding them into long expression chains.
- **Lints.** Clippy runs with `pedantic` on. The few allowed pedantic lints are listed, with their
  reason, under `[workspace.lints.clippy]` in the root `Cargo.toml`. Add to that list only for a lint that fires on
  most of the codebase; otherwise fix the code, or put a local `#[allow]` with a comment on the item.
- **Process state stays at the edge.** `clippy.toml` disallows reading or mutating environment
  variables, changing the working directory, `std::process::exit`, `std::fs::read_dir` and
  `std::fs::read_to_string`. `crates/ambit/src/main.rs` reads the environment and cwd once and passes them down as
  `util::env::Env` and a path; the filesystem is reached through `util::fs`, which keeps directory
  listings permutable for the determinism tests. `#[allow(clippy::disallowed_methods)]` is allowed
  only in `main.rs`, `crates/ambit-core/src/util` and test code. Tests run in parallel threads, so nothing may mutate
  process-wide state.
- **Order is observable.** Use `IndexMap`/`IndexSet` wherever iteration order reaches output, and
  `util::cmp::js_cmp` for every string sort, so output is identical on every machine.
- **Tests.** Unit tests live next to their module (`mod tests` or a sibling `tests.rs`). A test
  module that drives the CLI in-process (`test_support::run_cli`) is declared
  `#[cfg(all(test, feature = "cli"))]`. CLI, fixture and golden-file tests live in
  `crates/ambit/tests/`; their shared fixtures and helpers live in `crates/ambit-core/tests/`.
  `UPDATE_GOLDEN=1 cargo test` regenerates `crates/ambit/tests/golden/`.
