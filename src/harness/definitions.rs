//! The five harness profiles this build ships.

use std::sync::LazyLock;

use crate::harness::profile::HarnessProfile;

/// Claude Code.
///
/// `type` is emitted for `http` because the harness treats a server without one as stdio, and
/// omitted for stdio itself, where `command` already says so.
pub static CLAUDE: LazyLock<HarnessProfile> =
    LazyLock::new(|| todo!("port harness/definitions.ts:claude"));

/// Cursor. Infers the transport from the presence of `url`, so it wants no `type`.
///
/// Its hooks live in their own file, under their own event names, in an entry shape unlike
/// Claude's. All three differences are captured here: a layout, a map, and a renderer.
pub static CURSOR: LazyLock<HarnessProfile> =
    LazyLock::new(|| todo!("port harness/definitions.ts:cursor"));

/// VS Code (Copilot). Its section is `servers`, and it wants an explicit `type` on both
/// transports.
///
/// Uses `${env:VAR}` throughout, including in a stdio server's `env`. VS Code also supports
/// `${input:VAR}`, which prompts the user, but only when the file declares a matching entry in its
/// own `inputs` array; ambit does not write one, so this form is not used.
///
/// Its hooks are Claude's outright: VS Code reads `.claude/settings.json` natively, so this
/// profile reuses Claude's layout and renderer, including the `${CLAUDE_PROJECT_DIR}` placeholder.
/// That placeholder is undocumented for VS Code specifically, so this may resolve to a literal
/// string rather than a path. Written anyway: a separate spelling would require two entries in one
/// array for one declared hook, and both VS Code and Claude would run it.
pub static VSCODE: LazyLock<HarnessProfile> =
    LazyLock::new(|| todo!("port harness/definitions.ts:vscode"));

/// Codex. TOML, and the one harness with a first-class way to keep a credential out of the file.
///
/// The catalog's bearer-token variable becomes `bearer_token_env_var`. A header whose value is
/// nothing but a `${VAR}` reference becomes `env_http_headers`; other values remain static.
///
/// Its hooks live in `.codex/hooks.json` using Claude's own entry shape, so this profile reuses
/// Claude's renderer; the file itself is Codex's own.
pub static CODEX: LazyLock<HarnessProfile> =
    LazyLock::new(|| todo!("port harness/definitions.ts:codex"));

/// opencode. JSONC, `mcp` as its section, and its own vocabulary: `local`/`remote` rather than
/// stdio/http, one `command` array rather than a command and its arguments, and `environment` for
/// the env map.
///
/// Has no declarative hooks; it runs TypeScript plugins instead, which ambit cannot generate from a
/// declaration. No `hooks` field is set, and a project that selects a hook for opencode is told the
/// hook was skipped.
pub static OPENCODE: LazyLock<HarnessProfile> =
    LazyLock::new(|| todo!("port harness/definitions.ts:opencode"));

/// Every profile this build ships, in the order `--help` and error messages list them.
pub static PROFILES: LazyLock<Vec<&'static HarnessProfile>> =
    LazyLock::new(|| vec![&*CLAUDE, &*CODEX, &*CURSOR, &*OPENCODE, &*VSCODE]);
