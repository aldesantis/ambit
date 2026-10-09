use crate::harness::adapter::PlannedArtifact;
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact};
use crate::project::prune::PrunedArtifact;
use crate::util::json::{JsonObject, JsonValue};

pub const NO_DETAIL: &str = "-";

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
