//! How the mutating commands render an artifact.
//!
//! `install`, `prune` and `clean` all report the same two things (artifacts written and artifacts
//! removed), so the projections live here once: the same columns in the same order, the same JSON
//! keys, and a path that is always project-relative so nothing machine-specific reaches either
//! output.

use crate::harness::adapter::PlannedArtifact;
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact};
use crate::project::prune::PrunedArtifact;
use crate::util::json::{JsonObject, JsonValue};

/// Stands in for a cell an artifact kind has nothing to put in: a config file's mode, a
/// directory's keys.
pub const NO_DETAIL: &str = "-";

/// The part of an artifact a report shows, which owned, planned and pruned artifacts all carry.
///
/// It is the owned shape: all three carry it, and it's the only part a report should show. A
/// `target` is absolute and an `entries` list is the harness's business, so neither is here.
pub trait ReportedArtifact {
    fn path(&self) -> &str;
    fn kind(&self) -> ArtifactKind;
    fn mode(&self) -> Option<ArtifactMode>;
    fn managed_keys(&self) -> Option<&[String]>;
}

impl ReportedArtifact for OwnedArtifact {
    fn path(&self) -> &str {
        &self.path
    }

    fn kind(&self) -> ArtifactKind {
        self.kind
    }

    fn mode(&self) -> Option<ArtifactMode> {
        self.mode
    }

    fn managed_keys(&self) -> Option<&[String]> {
        self.managed_keys.as_deref()
    }
}

impl ReportedArtifact for PrunedArtifact {
    fn path(&self) -> &str {
        &self.path
    }

    fn kind(&self) -> ArtifactKind {
        self.kind
    }

    fn mode(&self) -> Option<ArtifactMode> {
        None
    }

    fn managed_keys(&self) -> Option<&[String]> {
        self.managed_keys.as_deref()
    }
}

impl ReportedArtifact for PlannedArtifact {
    fn path(&self) -> &str {
        PlannedArtifact::path(self)
    }

    fn kind(&self) -> ArtifactKind {
        PlannedArtifact::kind(self)
    }

    fn mode(&self) -> Option<ArtifactMode> {
        PlannedArtifact::mode(self)
    }

    fn managed_keys(&self) -> Option<&[String]> {
        match self {
            PlannedArtifact::HarnessConfig(config) => Some(&config.managed_keys),
            _ => None,
        }
    }
}

/// One artifact as a JSON record, with the keys in a fixed order: `kind`, `managedKeys`, `mode`,
/// `path`.
pub fn artifact_json(artifact: &dyn ReportedArtifact) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert(
        "kind".to_owned(),
        JsonValue::String(artifact.kind().as_str().to_owned()),
    );

    if let Some(keys) = artifact.managed_keys() {
        record.insert(
            "managedKeys".to_owned(),
            JsonValue::Array(keys.iter().cloned().map(JsonValue::String).collect()),
        );
    }

    if let Some(mode) = artifact.mode() {
        record.insert(
            "mode".to_owned(),
            JsonValue::String(mode.as_str().to_owned()),
        );
    }

    record.insert(
        "path".to_owned(),
        JsonValue::String(artifact.path().to_owned()),
    );
    record
}

/// One row per artifact written: path, kind, and the mode a skill directory was materialized in.
pub fn artifact_rows<A: ReportedArtifact>(artifacts: &[A]) -> Vec<Vec<String>> {
    artifacts
        .iter()
        .map(|artifact| {
            vec![
                artifact.path().to_owned(),
                artifact.kind().as_str().to_owned(),
                artifact
                    .mode()
                    .map_or(NO_DETAIL, ArtifactMode::as_str)
                    .to_owned(),
            ]
        })
        .collect()
}

/// One row per artifact removed: path, kind, and the keys taken out of a co-owned config file.
///
/// The third column carries keys rather than a mode: a removal needs to say how much of a co-owned
/// file went, since a config file loses keys and stays where it is, while a skill directory goes
/// whole.
pub fn removal_rows<A: ReportedArtifact>(artifacts: &[A]) -> Vec<Vec<String>> {
    artifacts
        .iter()
        .map(|artifact| {
            vec![
                artifact.path().to_owned(),
                artifact.kind().as_str().to_owned(),
                artifact
                    .managed_keys()
                    .map_or_else(|| NO_DETAIL.to_owned(), |keys| keys.join(", ")),
            ]
        })
        .collect()
}
