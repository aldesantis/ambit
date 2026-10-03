//! How the mutating commands render an artifact.
//!
//! `install`, `prune` and `clean` all report the same two things (artifacts written and artifacts
//! removed), so the projections live here once: the same columns in the same order, the same JSON
//! keys, and a path that is always project-relative so nothing machine-specific reaches either
//! output.

use crate::harness::adapter::PlannedArtifact;
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact};
use crate::project::prune::PrunedArtifact;
use crate::util::json::JsonObject;

/// Stands in for a cell an artifact kind has nothing to put in: a config file's mode, a
/// directory's keys.
pub const NO_DETAIL: &str = "-";

/// The part of an artifact a report shows, which owned, planned and pruned artifacts all carry.
///
/// A `target` is absolute and an `entries` list is the harness's business, so neither is here.
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
    let _ = artifact;
    todo!("port cli/handlers/artifacts.ts:artifactJson")
}

/// One row per artifact written: path, kind, and the mode a skill directory was materialized in.
pub fn artifact_rows<A: ReportedArtifact>(artifacts: &[A]) -> Vec<Vec<String>> {
    let _ = artifacts;
    todo!("port cli/handlers/artifacts.ts:artifactRows")
}

/// One row per artifact removed: path, kind, and the keys taken out of a co-owned config file.
///
/// The third column carries keys rather than a mode: a config file loses keys and stays where it
/// is, while a skill directory goes whole.
pub fn removal_rows<A: ReportedArtifact>(artifacts: &[A]) -> Vec<Vec<String>> {
    let _ = artifacts;
    todo!("port cli/handlers/artifacts.ts:removalRows")
}
