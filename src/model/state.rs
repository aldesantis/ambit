//! `.ambit/state.json`: the record of what ambit actually put on disk.
//!
//! Ambit deletes or overwrites only paths listed here, so a hand-written skill sitting at a target
//! path can never be touched. It is JSON rather than YAML because nothing reads it by hand, and a
//! crash-safety record wants one unambiguous serialization.
//!
//! Emission is sorted and byte-stable, same as the lock, so a state file does not reshuffle between
//! identical runs and hide the one change that matters in diff noise.

use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use serde_json::json;

use crate::errors::{AmbitError, Result, config_error};
use crate::model::documents::{DOCUMENT_FORMATS, DOCUMENT_SHAPES, DocumentFormat, DocumentShape};
use crate::util::cmp::js_cmp;
use crate::util::fs::{io_message, mkdir_p, read_text_opt, write_text};
use crate::util::json::{JsonObject, JsonValue, format_f64, parse, stringify_pretty};
use crate::util::path::join;
use crate::util::string_enum;

/// The machine-local directory ambit keeps its state in. Always gitignored.
pub const STATE_DIRNAME: &str = ".ambit";

/// The state file within it.
pub const STATE_FILENAME: &str = "state.json";

/// The only state version this build understands.
pub const STATE_VERSION: i64 = 1;

string_enum! {
    /// What an owned artifact is. `HarnessConfig` carries `managed_keys` instead of a `mode`.
    pub enum ArtifactKind {
        HarnessConfig => "harness-config",
        HookDir => "hook-dir",
        SkillDir => "skill-dir",
        SkillsLink => "skills-link",
    }
}

/// Every artifact kind, in declaration order.
pub const ARTIFACT_KINDS: &[ArtifactKind] = ArtifactKind::ALL;

string_enum! {
    /// How a materialized directory's source reaches its target: copied for remote sources,
    /// symlinked for local ones.
    pub enum ArtifactMode {
        Copy => "copy",
        Link => "link",
    }
}

/// Every artifact mode, in declaration order.
pub const ARTIFACT_MODES: &[ArtifactMode] = ArtifactMode::ALL;

/// One file or directory ambit created, addressed the only way that survives a move: relatively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedArtifact {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    /// Set for `skill-dir`, `hook-dir` and `skills-link`.
    pub mode: Option<ArtifactMode>,
    /// Set for `harness-config`: the dotted keys within the file ambit owns.
    pub managed_keys: Option<Vec<String>>,
    /// Set for `harness-config`: how the file is parsed and written.
    ///
    /// Recorded because `prune` and `clean` act from state alone: they must edit a
    /// `.codex/config.toml` as TOML without re-resolving the project to find out which harness
    /// wanted it. Absent reads as `json`, which is what every artifact written before this field
    /// existed was.
    pub format: Option<DocumentFormat>,
    /// Set for `harness-config`: how the managed section is laid out.
    ///
    /// Recorded for the same reason `format` is, and it is not derivable from `format`: `.mcp.json`
    /// and `.claude/settings.json` are both JSON, and the second holds one array per event rather
    /// than a table keyed by name. Absent reads as `map`, which is what every artifact written
    /// before this field existed was.
    pub shape: Option<DocumentShape>,
    /// Set for a copied `skill-dir` or `hook-dir`: the
    /// [`tree_digest`](crate::util::hash::tree_digest) of the directory as install wrote it.
    ///
    /// Lets `status` tell a copy edited since install from one whose source moved on, by hashing
    /// the copy alone. Absent for a link, which has no bytes of its own, and for anything written
    /// before this field existed, which `status` compares file by file instead.
    pub digest: Option<String>,
}

/// The contents of `.ambit/state.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: i64,
    /// The harnesses the artifacts were written for.
    pub harnesses: Vec<String>,
    pub artifacts: Vec<OwnedArtifact>,
}

impl State {
    /// What a project with no state file is treated as: ambit owns nothing there yet.
    pub fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            harnesses: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

/// Where the state file lives for a project.
pub fn state_file_path(project_dir: &Path) -> PathBuf {
    join(project_dir, &format!("{STATE_DIRNAME}/{STATE_FILENAME}"))
}

/// The set of paths ambit may delete or overwrite.
pub fn owned_paths(state: &State) -> IndexSet<String> {
    state
        .artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect()
}

fn artifact_json(artifact: &OwnedArtifact) -> JsonValue {
    let mut object = JsonObject::new();

    if let Some(digest) = &artifact.digest {
        object.insert("digest".to_owned(), json!(digest));
    }

    if let Some(format) = artifact.format {
        object.insert("format".to_owned(), json!(format.as_str()));
    }

    object.insert("kind".to_owned(), json!(artifact.kind.as_str()));

    if let Some(keys) = &artifact.managed_keys {
        let mut sorted = keys.clone();

        sorted.sort_by(|a, b| js_cmp(a, b));
        object.insert("managedKeys".to_owned(), json!(sorted));
    }

    if let Some(mode) = artifact.mode {
        object.insert("mode".to_owned(), json!(mode.as_str()));
    }

    object.insert("path".to_owned(), json!(artifact.path));

    if let Some(shape) = artifact.shape {
        object.insert("shape".to_owned(), json!(shape.as_str()));
    }

    JsonValue::Object(object)
}

/// Renders state as the bytes written to disk: keys sorted, artifacts by path, trailing newline.
pub fn serialize_state(state: &State) -> String {
    let mut artifacts: Vec<&OwnedArtifact> = state.artifacts.iter().collect();

    // Stable, as `Array.prototype.sort` is: two artifacts at one path keep their order.
    artifacts.sort_by(|a, b| js_cmp(&a.path, &b.path));

    let mut harnesses: Vec<String> = state
        .harnesses
        .iter()
        .cloned()
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();

    harnesses.sort_by(|a, b| js_cmp(a, b));

    let body = json!({
        "artifacts": artifacts.into_iter().map(artifact_json).collect::<Vec<_>>(),
        "harnesses": harnesses,
        "version": state.version,
    });

    format!("{}\n", stringify_pretty(&body))
}

fn state_error(file: &str, problem: &str) -> AmbitError {
    config_error(
        format!("{file} is not a valid ambit state file"),
        [
            problem.to_owned(),
            "delete it and run `ambit install` again to rebuild it".to_owned(),
        ],
    )
}

fn string_list(value: Option<&JsonValue>, file: &str, label: &str) -> Result<Vec<String>> {
    let problem = || state_error(file, &format!("\"{label}\" must be an array of strings"));
    let Some(JsonValue::Array(items)) = value else {
        return Err(problem());
    };

    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(problem))
        .collect()
}

/// An optional enum-valued field: absent is `None`, anything but one of `values`' spellings is
/// refused.
fn optional_enum<T: Copy + std::fmt::Display>(
    record: &JsonObject,
    key: &str,
    values: &[T],
    parse: fn(&str) -> Option<T>,
    file: &str,
    label: &str,
) -> Result<Option<T>> {
    let Some(value) = record.get(key) else {
        return Ok(None);
    };

    value.as_str().and_then(parse).map(Some).ok_or_else(|| {
        let spelled = values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");

        state_error(
            file,
            &format!("\"{label}.{key}\" must be one of: {spelled}"),
        )
    })
}

fn parse_artifact(value: &JsonValue, file: &str, index: usize) -> Result<OwnedArtifact> {
    let label = format!("artifacts[{index}]");

    let Some(record) = value.as_object() else {
        return Err(state_error(file, &format!("\"{label}\" must be an object")));
    };

    let path = match record.get("path").and_then(JsonValue::as_str) {
        Some(path) if !path.is_empty() => path.to_owned(),
        _ => {
            return Err(state_error(
                file,
                &format!("\"{label}.path\" must be a non-empty string"),
            ));
        }
    };

    let Some(kind) = record
        .get("kind")
        .and_then(JsonValue::as_str)
        .and_then(ArtifactKind::parse)
    else {
        let spelled = ARTIFACT_KINDS
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        return Err(state_error(
            file,
            &format!("\"{label}.kind\" must be one of: {spelled}"),
        ));
    };

    let mode = optional_enum(
        record,
        "mode",
        ARTIFACT_MODES,
        ArtifactMode::parse,
        file,
        &label,
    )?;
    let format = optional_enum(
        record,
        "format",
        DOCUMENT_FORMATS,
        DocumentFormat::parse,
        file,
        &label,
    )?;
    let shape = optional_enum(
        record,
        "shape",
        DOCUMENT_SHAPES,
        DocumentShape::parse,
        file,
        &label,
    )?;
    let digest = match record.get("digest") {
        None => None,
        Some(JsonValue::String(digest)) => Some(digest.clone()),
        Some(_) => {
            return Err(state_error(
                file,
                &format!("\"{label}.digest\" must be a string"),
            ));
        }
    };
    let managed_keys = match record.get("managedKeys") {
        None => None,
        Some(keys) => Some(string_list(
            Some(keys),
            file,
            &format!("{label}.managedKeys"),
        )?),
    };

    Ok(OwnedArtifact {
        path,
        kind,
        mode,
        managed_keys,
        format,
        shape,
        digest,
    })
}

/// Parses a state document. `file` is how it is named in error messages, conventionally
/// project-relative.
///
/// # Errors
///
/// Exit 2 for malformed JSON, an unsupported version, or a bad artifact entry: an unreadable
/// ownership record is exactly when ambit must stop rather than guess.
pub fn parse_state(text: &str, file: &str) -> Result<State> {
    let document = parse(text).map_err(|error| state_error(file, &error.to_string()))?;

    let Some(document) = document.as_object() else {
        return Err(state_error(file, "the document must be a JSON object"));
    };

    // A JSON number is a double, so `1.0` is the integer 1, as `Number.isInteger` judges it.
    let version = match document.get("version").and_then(JsonValue::as_f64) {
        Some(version) if version.is_finite() && version.fract() == 0.0 => version,
        _ => return Err(state_error(file, "\"version\" must be an integer")),
    };

    #[allow(clippy::float_cmp, clippy::cast_precision_loss)]
    if version != STATE_VERSION as f64 {
        return Err(config_error(
            format!(
                "{file} has unsupported state version {}",
                format_f64(version)
            ),
            [
                format!("this build of ambit understands version {STATE_VERSION}"),
                "upgrade ambit, or delete the file and run `ambit install` again".to_owned(),
            ],
        ));
    }

    let Some(JsonValue::Array(artifacts)) = document.get("artifacts") else {
        return Err(state_error(file, "\"artifacts\" must be an array"));
    };

    let harnesses = string_list(document.get("harnesses"), file, "harnesses")?;
    let artifacts = artifacts
        .iter()
        .enumerate()
        .map(|(index, artifact)| parse_artifact(artifact, file, index))
        .collect::<Result<Vec<_>>>()?;

    Ok(State {
        version: STATE_VERSION,
        harnesses,
        artifacts,
    })
}

/// Reads a project's state, treating an absent file as "ambit owns nothing here".
///
/// # Errors
///
/// Exit 2 if the file exists but cannot be trusted.
pub fn read_state(project_dir: &Path) -> Result<State> {
    let target = state_file_path(project_dir);
    let file = format!("{STATE_DIRNAME}/{STATE_FILENAME}");

    let text = match read_text_opt(&target) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(State::empty()),
        Err(error) => {
            return Err(config_error(
                format!("cannot read {file}"),
                [
                    io_message(&error, &target),
                    format!(
                        "make {} readable, or delete it and run `ambit install` again",
                        target.display()
                    ),
                ],
            ));
        }
    };

    parse_state(&text, &file)
}

/// Writes a project's state.
///
/// Called only after the filesystem changes it describes have succeeded, so a crash leaves
/// artifacts owned and recoverable rather than orphaned.
///
/// # Errors
///
/// Exit 1 when the directory or the file cannot be written: nothing anticipates that failure.
pub fn write_state(project_dir: &Path, state: &State) -> Result<()> {
    let target = state_file_path(project_dir);
    let directory = target.parent().expect("the state file has a directory");

    mkdir_p(directory).map_err(|error| AmbitError::unexpected(io_message(&error, directory)))?;
    write_text(&target, &serialize_state(state))
        .map_err(|error| AmbitError::unexpected(io_message(&error, &target)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ExitCode;
    use crate::test_support::tempdir;

    fn artifact(path: &str, kind: ArtifactKind) -> OwnedArtifact {
        OwnedArtifact {
            path: path.to_owned(),
            kind,
            mode: None,
            managed_keys: None,
            format: None,
            shape: None,
            digest: None,
        }
    }

    fn sample() -> State {
        State {
            version: STATE_VERSION,
            harnesses: vec![
                "cursor".to_owned(),
                "claude".to_owned(),
                "cursor".to_owned(),
            ],
            artifacts: vec![
                OwnedArtifact {
                    managed_keys: Some(vec!["mcpServers.b".to_owned(), "mcpServers.a".to_owned()]),
                    format: Some(DocumentFormat::Json),
                    shape: Some(DocumentShape::Map),
                    ..artifact(".mcp.json", ArtifactKind::HarnessConfig)
                },
                OwnedArtifact {
                    mode: Some(ArtifactMode::Copy),
                    digest: Some("sha256-abc".to_owned()),
                    ..artifact(".claude/skills/a", ArtifactKind::SkillDir)
                },
            ],
        }
    }

    #[test]
    fn serializes_sorted_and_byte_stable() {
        assert_eq!(
            serialize_state(&sample()),
            r#"{
  "artifacts": [
    {
      "digest": "sha256-abc",
      "kind": "skill-dir",
      "mode": "copy",
      "path": ".claude/skills/a"
    },
    {
      "format": "json",
      "kind": "harness-config",
      "managedKeys": [
        "mcpServers.a",
        "mcpServers.b"
      ],
      "path": ".mcp.json",
      "shape": "map"
    }
  ],
  "harnesses": [
    "claude",
    "cursor"
  ],
  "version": 1
}
"#
        );
    }

    #[test]
    fn round_trips_through_the_parser() {
        let parsed = parse_state(&serialize_state(&sample()), "state.json").unwrap();

        assert_eq!(parsed.harnesses, ["claude", "cursor"]);
        assert_eq!(parsed.artifacts.len(), 2);
        assert_eq!(parsed.artifacts[0].path, ".claude/skills/a");
        assert_eq!(
            parsed.artifacts[1].managed_keys.as_deref(),
            Some(&["mcpServers.a".to_owned(), "mcpServers.b".to_owned()][..])
        );
    }

    #[test]
    fn refuses_what_it_cannot_trust() {
        let cases = [
            ("[]", "the document must be a JSON object"),
            ("{\"version\": 1.5}", "\"version\" must be an integer"),
            (
                "{\"version\": 1, \"harnesses\": []}",
                "\"artifacts\" must be an array",
            ),
            (
                "{\"version\": 1, \"artifacts\": []}",
                "\"harnesses\" must be an array of strings",
            ),
            (
                r#"{"version": 1, "harnesses": [], "artifacts": [{"path": "x", "kind": "dir"}]}"#,
                "\"artifacts[0].kind\" must be one of: harness-config, hook-dir, skill-dir, skills-link",
            ),
            (
                r#"{"version": 1, "harnesses": [], "artifacts": [{"path": "", "kind": "skill-dir"}]}"#,
                "\"artifacts[0].path\" must be a non-empty string",
            ),
            (
                r#"{"version": 1, "harnesses": [], "artifacts": [{"path": "x", "kind": "skill-dir", "mode": 1}]}"#,
                "\"artifacts[0].mode\" must be one of: copy, link",
            ),
            (
                r#"{"version": 1, "harnesses": [], "artifacts": [{"path": "x", "kind": "harness-config", "format": "yaml"}]}"#,
                "\"artifacts[0].format\" must be one of: json, jsonc, toml",
            ),
        ];

        for (text, problem) in cases {
            let error = parse_state(text, ".ambit/state.json").unwrap_err();

            assert_eq!(error.code, ExitCode::Config);
            assert_eq!(
                error.format(),
                format!(
                    "error: .ambit/state.json is not a valid ambit state file\n       {problem}\n       delete it and run `ambit install` again to rebuild it"
                )
            );
        }
    }

    #[test]
    fn refuses_an_unsupported_version() {
        let error = parse_state("{\"version\": 2}", ".ambit/state.json").unwrap_err();

        assert_eq!(
            error.message,
            ".ambit/state.json has unsupported state version 2"
        );
    }

    #[test]
    fn reads_an_absent_file_as_owning_nothing_and_reads_back_what_it_wrote() {
        let dir = tempdir();

        assert_eq!(read_state(dir.path()).unwrap(), State::empty());

        write_state(dir.path(), &sample()).unwrap();

        assert_eq!(
            read_state(dir.path()).unwrap(),
            parse_state(&serialize_state(&sample()), "x").unwrap()
        );
    }
}
