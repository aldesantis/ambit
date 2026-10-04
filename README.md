# ambit

**ambit is a dependency manager for AI agents.**

Every agent harness (Claude Code, Codex, Copilot, Cursor, Devin, Gemini CLI, Grok, Kiro, opencode)
loads skills, hooks, and MCP servers. Today you copy those files between projects by hand, and they
drift. ambit lets you keep them in a git repo, declare which ones a project wants, and install them
into whatever harness your team uses.

You write a few lines of config. ambit fetches, resolves, and writes the files.

## Table of contents

- [Install](#install)
- [Quick start](#quick-start)
- [What you can select](#what-you-can-select)
- [Configuring your project](#configuring-your-project)
- [Authoring a catalog](#authoring-a-catalog)
- [Staying up to date](#staying-up-to-date)
- [Checking what you install](#checking-what-you-install)
- [CLI reference](#cli-reference)
- [Development](#development)
- [License](#license)

## Install

ambit is a single binary with nothing under it. You need `git` on your `PATH` and nothing else.

On macOS and Linux:

```
curl -fsSL https://raw.githubusercontent.com/aldesantis/ambit/main/install.sh | sh
```

On Windows, in PowerShell:

```
powershell -ExecutionPolicy Bypass -c "irm https://github.com/aldesantis/ambit/releases/latest/download/ambit-installer.ps1 | iex"
```

Both put `ambit` in `~/.local/bin` and add that directory to your `PATH`. The installer checks the
download against its SHA-256 checksum before installing it.

| Variable               | Effect                                                                     |
| ---------------------- | -------------------------------------------------------------------------- |
| `AMBIT_INSTALL_DIR`    | Install somewhere else.                                                    |
| `AMBIT_NO_MODIFY_PATH` | Set to `1` to leave your shell profile alone.                              |
| `AMBIT_VERSION`        | A tag like `v0.5.0` to install instead of the latest. `install.sh` only.   |

You can also download an archive for your machine from the
[releases page](https://github.com/aldesantis/ambit/releases). Each one holds the `ambit` binary and
has a `.sha256` file beside it.

| Archive                                  | For                    |
| ---------------------------------------- | ---------------------- |
| `ambit-aarch64-apple-darwin.tar.xz`      | macOS, Apple silicon   |
| `ambit-x86_64-apple-darwin.tar.xz`       | macOS, Intel           |
| `ambit-x86_64-unknown-linux-gnu.tar.xz`  | Linux, Intel and AMD   |
| `ambit-aarch64-unknown-linux-gnu.tar.xz` | Linux, ARM             |
| `ambit-x86_64-pc-windows-msvc.zip`       | Windows, Intel and AMD |

With a Rust toolchain you can build it from source instead:

```
cargo install --locked --git https://github.com/aldesantis/ambit
```

If you have ambit 0.4 or older, `ambit self-update` cannot find newer releases. Run the install
command above once, and `self-update` works from then on.

To upgrade a binary later, run `ambit self-update`. See
[Updating ambit itself](#updating-ambit-itself).

## Quick start

Start a project:

```
$ ambit init
created (5)
  ambit.yml
  hooks/.gitkeep
  mcps/.gitkeep
  packs/.gitkeep
  skills/.gitkeep
```

That writes `ambit.yml` plus the four item directories. Point it at a catalog and say what you want
from it:

```yaml
version: 1
harnesses: [claude]

catalogs:
  - name: company
    source: acme/skills
    ref: main

requires:
  - pack: "company/engineering" # everything that pack names, transitively
  - skill: "company/core.*" # every skill under the `core` prefix
```

Then install:

```
$ ambit install
harnesses (1)
  claude

artifacts (5)
  .agents/skills/house-style  skill-dir       link
  .agents/skills/code-review  skill-dir       link
  .agents/skills/storybook    skill-dir       link
  .claude/skills              skills-link     link
  .mcp.json                   harness-config  -
```

Your agent now sees those skills. ambit only touches files it created, so anything you wrote by hand
survives `install`, `prune`, and `clean`.

Three commands cover most of what you will do next:

```
$ ambit search "*"          # everything the catalogs offer, whether you selected it or not
$ ambit resolve --explain   # what you would get, and why
$ ambit outdated            # has any catalog moved, and would it change anything?
```

## What you can select

A **catalog** is a git repo (or a local directory) holding up to four kinds of thing:

| Kind      | Lives in                 | What it is                                                              |
| --------- | ------------------------ | ----------------------------------------------------------------------- |
| **Skill** | `skills/<name>/SKILL.md` | Instructions the agent can load                                         |
| **MCP**   | `mcps/<name>.yml`        | A server definition                                                     |
| **Hook**  | `hooks/<name>/hook.yml`  | A command that runs on one harness event                                |
| **Pack**  | `packs/<name>.yml`       | A named group of capabilities; optionally exportable as a Claude plugin |

An item's name is its path inside its directory, with `/` read as `.`. So
`skills/close-crm/SKILL.md` is the skill `close-crm`, and `packs/function/engineering.yml` is the
pack `function.engineering`.

Your project is also a catalog. `ambit init` lists it as one, so a skill you write locally is
selected exactly like a skill from a shared repo.

Supported harnesses, and where ambit writes for each:

| Harness    | Tool                       | Skills           | MCP servers                | Hooks                    |
| ---------- | -------------------------- | ---------------- | -------------------------- | ------------------------ |
| `claude`   | Claude Code                | `.claude/skills` | `.mcp.json`                | `.claude/settings.json`  |
| `codex`    | Codex                      | `.agents/skills` | `.codex/config.toml`       | `.codex/hooks.json`      |
| `copilot`  | GitHub Copilot in VS Code  | `.agents/skills` | `.vscode/mcp.json`         | `.claude/settings.json`  |
| `cursor`   | Cursor                     | `.claude/skills` | `.cursor/mcp.json`         | `.cursor/hooks.json`     |
| `devin`    | Devin Desktop, Devin Local | `.agents/skills` | `.devin/mcp_config.json`   | `.claude/settings.json`  |
| `gemini`   | Gemini CLI                 | `.agents/skills` | `.gemini/settings.json`    | `.gemini/settings.json`  |
| `grok`     | Grok Build                 | `.grok/skills`   | `.grok/config.toml`        | `.claude/settings.json`  |
| `kiro`     | Kiro                       | `.kiro/skills`   | `.kiro/settings/mcp.json`  | `.kiro/hooks/ambit.json` |
| `opencode` | opencode                   | `.agents/skills` | `.opencode/opencode.jsonc` | none                     |

Skills are always written once to `.agents/skills`. A harness that looks elsewhere gets its skills
directory as a symlink to it. Files ambit writes into are merged, so your own entries in them stay.

Some harnesses load a project's config only once you trust it:

- Gemini CLI ignores `.gemini/settings.json` in a folder you have not trusted.
- Grok loads a project's hooks and MCP servers after you run `/hooks-trust`, or start it with
  `--trust`.
- Kiro asks you to approve each environment variable an MCP server's config references.

## Configuring your project

Everything a project declares lives in `ambit.yml`:

```yaml
version: 1
harnesses: [claude]

# Where items come from. Order carries no meaning.
catalogs:
  - name: company
    source: git@github.com:acme/skills.git
    ref: "a1b2c3d4" # tag, branch, or commit. Quote it. Omit for the default branch.
  - name: personal
    source: git@github.com:jane/skills-private.git
    ref: main
  - name: local
    source: path:. # this project's own packs/, skills/, mcps/, hooks/

# What this project selects. Nothing is implicit: an item no entry reaches is
# not installed. Each entry names its kind and carries `<catalog>/<pattern>`.
requires:
  - pack: "company/function.engineering"
  - skill: "company/core.*"
  - skill: "personal/luma"
  - hook: "company/guards.*"
```

| Field       | Type         | Required | Notes                                                                                                                              |
| ----------- | ------------ | -------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `version`   | int          | yes      | Must be `1`.                                                                                                                       |
| `harnesses` | string[]     | no       | Any of the supported harnesses above. Default `[claude]`.                                                                          |
| `catalogs`  | list of maps | no       | `name`, `source`, `ref?`, `path?`. `name` must be unique and hold no `/`, since it is the first half of an address. Dots are fine. |
| `requires`  | list of maps | no       | Each entry: exactly one key of `pack`/`skill`/`mcp`/`hook`, carrying `<catalog>/<pattern>`. An entry matching nothing is an error. |

**Source formats:** `owner/repo`, `owner/repo@ref` (GitHub shorthand),
`https://github.com/owner/repo`, `git@host:owner/repo.git`, `git:<any-git-url>`,
`path:./relative/dir`.

**Catalogs in a subdirectory:** set `path` when the catalog's `skills/`, `mcps/`, `hooks/`, and
`packs/` sit in a directory of the source rather than at its root. It is relative to the source's
root and cannot leave it.

```yaml
catalogs:
  - name: pstack
    source: cursor/plugins
    ref: main
    path: pstack
```

### One install for every project

Your home directory can be the project. `ambit init` and `ambit install` work there like anywhere
else: `~/ambit.yml` says what you want, and it lands in `~/.agents/` and the harness files under `~`,
which is where a harness keeps the config it applies to every project on the machine.

ambit reads that off the root. Your home directory is a user-level install, any other directory is a
project. Claude MCP servers go into `~/.claude.json` for a user-level install and `.mcp.json` for a
project install. A hook that ships a script also uses a different address. A project install
names the script relative to the project root:

```json
"command": "${CLAUDE_PROJECT_DIR}/.agents/hooks/guard-secrets/guard.sh"
```

A user-level install names it outright, since there is no single project to be relative to. A
relative path there would resolve inside whichever project you happen to have open:

```json
"command": "/Users/jane/.agents/hooks/guard-secrets/guard.sh"
```

Which files under `~` a harness actually reads as your own config is the harness's own rule, and it
differs between them. Claude Code reads `~/.claude.json`, `~/.claude/settings.json`, and
`~/.claude/skills` this way, so MCP servers, hooks, and skills installed at home reach every project.

## Authoring a catalog

A catalog is a plain git repo with some of `packs/`, `skills/`, `mcps/`, and `hooks/` in it. There is
no config file and no command that writes into one: it is Markdown and YAML, so use your editor and
run `ambit validate` to check the result.

### Skills

A skill is a directory holding `SKILL.md`, named by its path under `skills/` with `/` read as `.`.
A frontmatter `name` that disagrees is reported by `ambit validate` and otherwise ignored. ambit reads
two optional keys from its frontmatter:

```yaml
---
name: close-crm
description: "Calls the Close CRM REST API…"
ambit:
  requires: # pulled into the bundle alongside this skill
    - skill: company-context
    - hook: guards.*
  expects: # checked by `ambit doctor`
    - env: CLOSE_API_KEY
---
```

`requires` says what this skill cannot work without, so a project that takes the skill gets a
working bundle rather than a broken one. `expects` says what must be true of the machine. Patterns
here are **bare**, with no catalog name, and resolve within the catalog that ships the skill. A
catalog can only require what it ships.

### MCP servers

```yaml
name: sentry

transport:
  http:
    url: https://mcp.sentry.dev/mcp
    bearer_token_env_var: SENTRY_TOKEN

# or, for a locally-spawned server:
# transport:
#   stdio:
#     command: npx
#     args: ["-y", "@acme/close-mcp"]

expects:
  - env: SENTRY_TOKEN
```

| Key                                   | Type     | Required      | Notes                                                       |
| ------------------------------------- | -------- | ------------- | ----------------------------------------------------------- |
| `name`                                | string   | yes           | Must match the filename stem. `.yml` and `.yaml` both work. |
| `transport`                           | map      | yes           | Exactly one key: `stdio` or `http`.                         |
| `transport.stdio.command`             | string   | yes for stdio | Executable to spawn.                                        |
| `transport.stdio.args`                | string[] | no            | Arguments, in order.                                        |
| `transport.stdio.env`                 | map      | no            | Variables the process is given. Same `${VAR}` handling.     |
| `transport.http.url`                  | string   | yes for http  | Server endpoint.                                            |
| `transport.http.bearer_token_env_var` | string   | no            | Environment variable whose value is sent as a bearer token. |
| `transport.http.headers`              | map      | no            | `${VAR}` becomes a reference in each harness's own syntax.  |
| `expects`                             | map[]    | no            | Preconditions. Today only `env:`.                           |

A stdio server is given every variable it `expects`, under the name it expects. `transport.stdio.env`
is for a server whose own variable names are not the ones your machine sets:

```yaml
name: planner

transport:
  stdio:
    command: npx
    args: ["-y", "@acme/planner-mcp"]
    env:
      PLANNER_TOKEN: "${ACME_PLANNER_TOKEN}"

expects:
  - env: ACME_PLANNER_TOKEN
```

The process gets `PLANNER_TOKEN`, taking its value from `ACME_PLANNER_TOKEN`. A variable an entry
references is not also passed under its own name, so two servers that both read `PLANNER_TOKEN` can
each take it from a variable of their own.

### Hooks

A hook is a directory holding `hook.yml`, plus a script if it ships one.

```yaml
name: block-rm
description: Refuses a destructive rm before it runs

event: PreToolUse
matcher: Bash
type: script # or `command`
command: guard.sh # a file this directory ships, since `type` is `script`
timeout: 30

expects:
  - env: SOME_TOKEN
```

| Key           | Type   | Required | Notes                                                                                                                       |
| ------------- | ------ | -------- | --------------------------------------------------------------------------------------------------------------------------- |
| `name`        | string | yes      | Must match the directory path under `hooks/`.                                                                               |
| `description` | string | no       | Carried into reports.                                                                                                       |
| `event`       | string | yes      | One of `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `PreCompact`, `SessionEnd`. |
| `matcher`     | string | no       | Tool-name filter. Valid only on `PreToolUse` and `PostToolUse`.                                                             |
| `type`        | string | yes      | `command` or `script`, saying how to read `command`.                                                                        |
| `command`     | string | yes      | What to run.                                                                                                                |
| `timeout`     | int    | no       | Seconds.                                                                                                                    |
| `expects`     | map[]  | no       | Preconditions. Today only `env:`.                                                                                           |

`type: command` is a command line the harness runs as written, like `npx prettier --write`.
`type: script` names a file this directory ships, optionally with arguments: `guard.sh --strict`.
ambit copies the script next to the harness config and rewrites the path, leaving your arguments
untouched. `${VAR}` in a `command` is left as written, since the harness runs it through a shell.

Hook support varies by harness:

| Harness                              | Written to               | Notes                                                                                                                                                  |
| ------------------------------------ | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `claude`, `copilot`, `devin`, `grok` | `.claude/settings.json`  | Copilot, Devin and Grok read Claude's file natively, so it is written once.                                                                            |
| `cursor`                             | `.cursor/hooks.json`     | Different event names, and no `matcher` field, so a matcher is dropped.                                                                                |
| `codex`                              | `.codex/hooks.json`      | Experimental: needs `[features] codex_hooks = true` in the user's own config. `doctor` warns.                                                          |
| `gemini`                             | `.gemini/settings.json`  | Different event names. Claude tool names in a `matcher` are translated (`Bash` becomes `run_shell_command`). No `SubagentStop` event.                  |
| `kiro`                               | `.kiro/hooks/ambit.json` | No `SubagentStop` or `PreCompact` trigger. A `matcher` is written as declared; Kiro matches it against its own tool names (`shell`, `read`, `write`).  |
| `opencode`                           | none                     | No declarative hooks.                                                                                                                                  |

A hook on an event a harness has no counterpart for is skipped for that harness with a warning, and
the install succeeds.

### Packs

A pack groups skills, MCP servers, hooks, and other packs under one name. Selecting it includes
its requirements. Add `plugin` metadata to export the pack as a Claude Code plugin.

```yaml
# packs/function/engineering.yml
name: function.engineering
description: Everything an Acme engineer needs — reviews, tooling, and the guards around them.

requires:
  - pack: core # packs compose
  - skill: code-review
  - skill: guides.*
  - mcp: linter
  - hook: guard-secrets
```

| Key           | Type   | Required | Notes                                                                      |
| ------------- | ------ | -------- | -------------------------------------------------------------------------- |
| `name`        | string | yes      | Must match the path under `packs/`, extension dropped and `/` read as `.`. |
| `description` | string | no       | What the pack is for. Shown by `ambit search`.                             |
| `requires`    | map[]  | no       | Same grammar as a skill's: one key per entry, bare patterns, same catalog. |
| `plugin`      | map    | no       | Metadata for [exporting a Claude plugin](#exporting-claude-plugins).       |

## Exporting Claude plugins

Select packs in `ambit.yml`, then export them for people who use Claude Code without Ambit:

```yaml
# ambit.yml
version: 1
catalogs:
  - name: local
    source: path:.
requires:
  - pack: local/reviews
```

Define the pack:

```yaml
# packs/reviews.yml
name: reviews
plugin:
  name: acme-reviews
  version: "1.0.0"
  description: Review changes using the team's conventions.
  author:
    name: Acme
requires:
  - skill: code-review
```

Create `skills/code-review/SKILL.md`:

```markdown
---
name: code-review
description: Review changes for correctness and maintainability.
---

Review the diff and report actionable findings.
```

```sh
ambit export --format claude-plugin --output dist/plugins
```

```text
Exported 1 Claude plugins to /path/to/project/dist/plugins
  acme-reviews/ (acme-reviews, 2 files)
```

The result contains `acme-reviews/.claude-plugin/plugin.json` and
`acme-reviews/skills/code-review/SKILL.md`. Validate or try it with Claude:

```sh
claude plugin validate dist/plugins/acme-reviews
claude --plugin-dir ./dist/plugins/acme-reviews
```

Each selected pack must have `plugin` metadata. A requirement on another pack with plugin metadata
adds that plugin to the manifest's `dependencies` and exports it in a separate directory. Packs
without plugin metadata expand into the containing plugin. Skill requirements include transitive
skills, MCP servers, and hooks. A pack containing only plugin dependencies is supported.

Use `plugin.dependencies` for external plugins already available in the consumer's marketplace.
Ambit records their names without downloading or exporting them. List local dependencies through
`requires: [{pack: other-pack}]`. External names appear first in the manifest, followed by local
plugin dependencies in declaration order; repeated names appear once. External plugin dependencies
apply to export only; `ambit install` installs the pack's Ambit requirements.

| `plugin` field | Type     | Required | Behavior                                                                   |
| -------------- | -------- | -------- | -------------------------------------------------------------------------- |
| `name`         | string   | yes      | Claude plugin namespace. Lowercase letters, digits, and single hyphens.    |
| `version`      | string   | no       | Plugin version, such as `"1.0.0"`.                                         |
| `description`  | string   | no       | Plugin description. Separate from the pack's search description.           |
| `author`       | map      | no       | Author with required `name` and optional `email` and `url` strings.        |
| `homepage`     | string   | no       | Plugin homepage URL.                                                       |
| `repository`   | string   | no       | Source repository URL.                                                     |
| `license`      | string   | no       | License identifier.                                                        |
| `keywords`     | string[] | no       | Discovery keywords.                                                        |
| `dependencies` | string[] | no       | External Claude plugin names.                                              |
| `directory`    | string   | no       | Output directory basename. Defaults to `name`; uses the same naming rules. |
| `commands`     | string   | no       | Catalog-relative directory copied into the plugin's `commands/`.           |

| Export flag              | Behavior                                                                                                                                                        |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--format claude-plugin` | Required. Claude Code is the supported format.                                                                                                                  |
| `--output <dir>`         | Required. New output directory, relative to `--project` or the current project. Existing paths require `--force` or `--check`.                                  |
| `--force`                | Replace the entire output directory after validating the export. Preserve JSON formatting when values are unchanged.                                            |
| `--check`                | Compare existing files, executable bits, JSON values, and exact symlink targets without writing. Exit 5 on drift. Cannot combine with `--force` or `--dry-run`. |
| `--link`                 | Create relative symlinks for skill directories and hook assets. Requires local `path:` catalogs.                                                                |
| `--link`                 | Create relative symlinks for skill directories and hook assets. Requires local `path:` catalogs.                                                                |
| `--dry-run`              | Validate packages and report their names and file counts without writing output.                                                                                |
| `--json`                 | Print the output path and plugin names, directories, and file counts as JSON.                                                                                   |
| `--offline`              | Use cached catalogs only.                                                                                                                                       |
| `--project <dir>`        | Read `ambit.yml` and `ambit.lock` from this project.                                                                                                            |

For a marketplace kept in the same repository as its catalog, use
`ambit export --format claude-plugin --output plugins --link`. Skills and hook assets remain linked
to their source files; manifests and slash commands are regular files. Keep the catalogs and exported
plugins in the same relative locations. Omit `--link` to produce standalone copies.

Regenerate an existing marketplace and check it in CI:

```sh
ambit export --format claude-plugin --output plugins --link --force
ambit export --format claude-plugin --output plugins --link --check
```

`--force` removes stale files from the output directory. It refuses to replace a project root,
a catalog root, or a directory containing source skills or hooks. Failed validation leaves the
existing export untouched.

Exports honor catalog revisions in `ambit.lock` and leave the lock and installed harness configuration
unchanged. Pin remote inputs with a lock or an immutable catalog `ref` for reproducible packages.

| Content          | Export behavior and limits                                                                                                                                                                                                     |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Skills           | Copy into `skills/<name>/`, including supporting files. Names must be flat, lowercase and hyphenated, at most 64 characters; frontmatter must include matching `name` and a nonempty `description`. Nested skills are refused. |
| Skill references | Use `/plugin-name:skill-name` for explicit Claude invocations. Ambit checks these against the exported plugin and its dependency plugins; external plugin skill names cannot be verified. Skill prose is copied unchanged.     |
| MCP servers      | Write `.mcp.json` using Claude's environment references. Credential values are never read. Local MCP commands and explicit local-path arguments are refused; reference bundled assets through `${CLAUDE_PLUGIN_ROOT}`.         |
| Hooks            | Write `hooks/hooks.json`; copy script assets into `hooks/` and reference them with `${CLAUDE_PLUGIN_ROOT}`. Assets with conflicting paths are refused.                                                                         |
| Slash commands   | Copy the declared directory into `commands/`. Markdown commands must have YAML frontmatter.                                                                                                                                    |
| Symlinks         | Copy their targets when inside the catalog. External targets, cycles, and relative Markdown links above the plugin root are refused. Executable permissions are preserved.                                                     |

Export does not publish a marketplace. Distribute the generated directories through your own
marketplace. Agent Plugins export is not supported.

## Staying up to date

`ambit.lock` records the exact commit each catalog resolved to, and every later command uses that
commit rather than asking what the `ref` points at now. So `ref: main` keeps meaning one commit,
`ambit install` run twice a week apart installs the same bytes, and a lock committed on one machine
installs the same bytes on another. Commit it.

A catalog with no recorded commit resolves its `ref` against the remote. That happens on a first
install, when you add a catalog, and when you edit a `ref`.

`ambit outdated` asks each remote where its `ref` points now and reports what moving there would
change. It changes nothing itself:

```
$ ambit outdated
catalogs (2)
  company   outdated  a1b2c3d → f9e1a04
  personal  current   3f1a99b

packs (1)
  ~  engineering  requires changed

skills (3)
  +  code-review  required-by:pack:engineering
  +  storybook    required-by:skill:code-review
  ~  house-style  description changed

mcps (2)
  ~  sentry  transport.http.url changed
  -  linear  was required-by:pack:engineering

hooks (1)
  +  guard-secrets  PreToolUse Bash — runs .agents/hooks/guard-secrets/guard.sh
```

**The report is about capabilities, not commits.** A branch that advanced two hundred commits
without touching anything you selected reports a moved commit and an empty diff.

| Freshness     | Meaning                                                              |
| ------------- | -------------------------------------------------------------------- |
| `outdated`    | The `ref` points at a different commit than the project uses now.    |
| `current`     | It points at the same one.                                           |
| `pinned`      | The `ref` is a commit, so it cannot point anywhere else.             |
| `unversioned` | A `path:` source. It has no revision, so use `ambit status` instead. |

`ambit update` is the command that moves the pins forward and then installs. `ambit update
--dry-run` is `ambit outdated` limited to the catalogs you named.

## Checking what you install

### Digests in the lock

For every skill and every script-shipping hook from a git catalog, `ambit.lock` records a `digest`
of its files beside the commit:

```yaml
skills:
  code-review:
    catalog: company
    commit: 3f1a99b0c4e2d6a8b1f7e5c3a9d2b4f6e8a0c1d3
    digest: sha256-9b2c41d7e0f35a8c6b1d2e4f7a9c0b3d5e8f1a2c4b6d8e0f3a5c7b9d1e2f4a6c
    path: skills/code-review
    reason: required-by:pack:engineering
```

`ambit install` and `ambit update` check every digest before writing anything. When the files at a
recorded commit no longer hash to the recorded digest, they stop with exit 5:

```
error: skill "code-review" does not match the digest ambit.lock records
       ambit.lock records sha256-9b2c41d7… for skills/code-review at commit 3f1a99b0…
       the catalog checkout holds sha256-41d7e0f3…
       find out why these files changed while the commit did not; to accept them, delete this entry's `digest` from ambit.lock and run `ambit install` again
```

A commit that moved records a new digest. A lock with no digests gains them on the next install.
`path:` catalogs and MCP servers get none: a working directory has no commit to pin, and a server is
a few config values, not files.

`ambit status` uses the same digest to report a copied skill or hook that was edited after install.

### Auditing content

A skill is a prompt, and some characters render as nothing in a diff view while a model still reads
them. `ambit audit` reads every file of every item in every catalog the project lists and reports
them:

```
$ ambit audit
skills (1)
  !  house-style  3 zero-width joiners (U+200D) at SKILL.md:41

hooks (1)
  ~  guard-secrets  command names /etc/passwd, an absolute path in hook.yml

audit found 2 issues in 2 items
```

`!` is a failure and `~` a warning. Any failure exits 6; warnings alone exit 0. A location is
relative to the skill's or hook's directory, or to the catalog root for a pack or an MCP server.
Files that are not UTF-8 are skipped.

| Check          | Severity | What it finds                                                                                                                                                           |
| -------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `invisible`    | failure  | Zero-width space, non-joiner and joiner, word joiner, soft hyphen, and U+FEFF anywhere but the start of a file.                                                         |
| `bidi`         | failure  | Bidi embeddings, overrides and isolates (U+202A to U+202E, U+2066 to U+2069). The marks U+200E, U+200F and U+061C are warnings.                                         |
| `tag`          | failure  | Tag characters, U+E0000 to U+E007F.                                                                                                                                     |
| `mixed-script` | warning  | A skill, pack, MCP server or hook name mixing Latin, Greek, Cyrillic, Armenian or Cherokee letters.                                                                     |
| `command`      | warning  | A hook command, a script hook's arguments, or a stdio server's command line naming an absolute path, `~`, `$HOME` or a `..` segment, or piping a download into a shell. |

`ambit install` and `ambit update` run the same checks on the items they are about to install. A
failure refuses the install with exit 6 and names each finding. A warning prints to stderr and the
install goes ahead. `--no-audit` skips the checks.

### Updating ambit itself

`ambit self-update` replaces the binary you are running with the newest release:

```
$ ambit self-update
current  0.5.0
target   v0.5.1
asset    ambit-aarch64-apple-darwin.tar.xz
binary   /Users/you/.local/bin/ambit

installed ambit v0.5.1
```

The download is checked against the release's `<asset>.sha256` file before it is installed. A
mismatch leaves the binary you already have alone, and there is no flag to skip the check.

Name a release to install that one instead, which is also how you go back after a bad release:

```
ambit self-update v0.5.0
```

`--dry-run` prints the same report and installs nothing.

Once a day, other commands check whether a newer release exists and print one line to stderr when
there is:

```
ambit v0.5.1 is available; you are on 0.5.0. To upgrade, run `ambit self-update`.
```

The check is skipped when stderr is not a terminal, under CI, with `--json`, with `--offline`, and
when `AMBIT_NO_UPDATE_CHECK` is set to anything. It never delays or fails the command it follows.

## CLI reference

### Commands

| Command                                                             | What it does                                                                                                                                         |
| ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ambit init`                                                        | Scaffold `ambit.yml`, the four item directories, and a `catalogs:` entry naming the project itself. Refuses a directory that already has a config.   |
| `ambit search [--catalog <name>…] [--capability <kind>…] <pattern>` | Search every catalog the project lists, whether anything selects the item or not. Same patterns as `requires`. A pattern matching nothing is exit 0. |
| `ambit resolve [--explain]`                                         | Compute the bundle and print it. `--explain` prints why each item is in it.                                                                          |
| `ambit why <kind:name>`                                             | Explain why one item is in the bundle, as a chain back to the entry that asked for it.                                                               |
| `ambit install [--frozen] [--adopt] [--copy\|--link] [--no-audit]`  | Resolve, write `ambit.lock`, install the files, remove what is no longer selected. See [install flags](#install-flags).                              |
| `ambit export --format claude-plugin --output <dir>`                | Export selected packs and their local plugin dependencies into separate Claude plugin directories.                                                   |
| `ambit outdated`                                                    | Ask each remote where its `ref` points now, and report what moving there would change.                                                               |
| `ambit update [<catalog>…] [--adopt] [--copy\|--link] [--no-audit]` | Move those pins forward, then install. Every catalog when none is named.                                                                             |
| `ambit status [--check]`                                            | Compare what is installed against what resolve produces. `--check` exits 5 on drift.                                                                 |
| `ambit prune`                                                       | Remove installed files that are no longer selected.                                                                                                  |
| `ambit clean`                                                       | Remove everything ambit installed.                                                                                                                   |
| `ambit validate`                                                    | Validate the config and every catalog the project lists. A catalog repo runs this too, since it lists itself.                                        |
| `ambit doctor`                                                      | Check preconditions, the lock, ownership, drift, and harness limits.                                                                                 |
| `ambit audit`                                                       | Scan every catalog the project lists for hidden text and risky command lines. See [auditing content](#auditing-content).                             |
| `ambit self-update [<version>]`                                     | Replace this ambit binary with a released one, checksum verified. The newest release when no version is named.                                       |

### Global flags

| Flag              | Notes                                                                                                  |
| ----------------- | ------------------------------------------------------------------------------------------------------ |
| `--project <dir>` | The project to act on. Default: the current directory. Not on `self-update`, which acts on the binary. |
| `--json`          | Machine-readable output. Every command supports it.                                                    |
| `--offline`       | Resolve from the local cache alone. Refused by `outdated`, `update`, and `self-update`.                |
| `--dry-run`       | On mutating commands: report what would happen and touch nothing.                                      |
| `-h`, `--help`    | Usage for the program or for any command. `ambit help <command>` prints the same.                      |
| `-V`, `--version` | Print the ambit version.                                                                               |

### Install flags

| Flag         | Notes                                                                                   |
| ------------ | --------------------------------------------------------------------------------------- |
| `--frozen`   | Exit 5 instead of writing when resolution would change `ambit.lock`. For CI.            |
| `--adopt`    | Take ownership of existing files ambit did not create, instead of refusing them.        |
| `--copy`     | Copy every skill, including those from `path:` catalogs, which are symlinked otherwise. |
| `--link`     | Symlink every skill instead of copying it.                                              |
| `--no-audit` | Skip the [content audit](#auditing-content) of the items being installed.               |

`ambit update` takes the same flags except `--frozen`.

### Exit codes

| Code | Meaning                                                                                                                                    |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| 0    | Success                                                                                                                                    |
| 1    | Unexpected internal error                                                                                                                  |
| 2    | Config, ownership, export compatibility, or usage error                                                                                    |
| 3    | Resolution error: a pattern matching nothing, missing requirement, cycle, name conflict                                                    |
| 4    | Network or cache error                                                                                                                     |
| 5    | Drift detected (`status --check`, `install --frozen`, `export --check`), or a catalog's files no longer match their digest in `ambit.lock` |
| 6    | A health check found something (`doctor` or `audit` failures), or the audit refused an install                                             |

Every error names the file, the identifier, and one concrete next step:

```
error: `requires` entry "pack:company/function.enginering" matches nothing (ambit.yml line 6)
       no pack in catalog "company" has a name matching "function.enginering"
       correct the pattern, add the item to a catalog, or remove the entry

error: refusing to overwrite unowned path
       .agents/skills/close-crm exists but ambit did not create it
       move it aside, or run `ambit install --adopt` to take ownership
```

## Development

ambit is written in Rust. You need the toolchain pinned in `rust-toolchain.toml` (rustup installs
it on first use) and `git`.

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt                # `cargo fmt --check` is the CI variant
cargo build --release
```

`cargo run -- <args>` is the CLI.

`tests/golden/` holds recorded program output. Regenerate it with `UPDATE_GOLDEN=1 cargo test` and
read the diff.

Releases are built by [cargo-dist](https://github.com/axodotdev/cargo-dist) from
`dist-workspace.toml`. To cut one, set `version` in `Cargo.toml`, commit it, and push a matching
tag: `git tag v0.5.1 && git push --tags`. The release workflow runs the CI checks first and refuses
a tag that does not match the version.

## License

MIT
