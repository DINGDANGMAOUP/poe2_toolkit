use crate::{Feature, MarketScope, digest};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_evidence: Option<AnnotationEvidence>,
    pub resource: String,
    pub record_id: String,
    pub feature: Feature,
    pub before: String,
    pub after: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationEvidence {
    pub quote_ids: Vec<String>,
    pub snapshot_id: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl AnnotationEvidence {
    pub fn can_retain(&self, now: DateTime<Utc>, minutes: u32) -> bool {
        let anchor = self
            .observed_at
            .unwrap_or(self.fetched_at)
            .min(self.fetched_at);
        minutes > 0
            && anchor <= now
            && self.fetched_at <= now
            && self.expires_at > now
            && (now - anchor).num_seconds() <= i64::from(minutes) * 60
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMutation {
    pub relative_path: String,
    pub before_hash: Option<String>,
    pub after_hash: String,
    pub content: Vec<u8>,
    /// Remove an owned file. Removal has empty content and the empty digest.
    #[serde(default)]
    pub remove: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<StreamRecipe>,
}

/// Streaming reconstruction of the same file; recipes cannot read other paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRecipe {
    pub source_hash: String,
    pub source_len: u64,
    pub pieces: Vec<StreamPiece>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamPiece {
    Copy { offset: u64, length: u64 },
    Bytes { data: Vec<u8> },
}
impl StreamRecipe {
    pub fn output_len(&self) -> Option<u64> {
        if self.pieces.len() > 4096 || self.source_len > 512 * 1024 * 1024 * 1024 {
            return None;
        }
        self.pieces.iter().try_fold(0u64, |sum, piece| {
            let n = match piece {
                StreamPiece::Copy { offset, length } => {
                    if offset.checked_add(*length)? > self.source_len {
                        return None;
                    }
                    *length
                }
                StreamPiece::Bytes { data } => data.len() as u64,
            };
            sum.checked_add(n)
                .filter(|n| *n <= 512 * 1024 * 1024 * 1024)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileExpectation {
    pub relative_path: String,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanIntent {
    Annotate,
    ClearAnnotations,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    pub id: String,
    pub protocol: u32,
    pub intent: PlanIntent,
    pub installation_id: String,
    pub installation_root: PathBuf,
    pub installation_fingerprint: String,
    pub profile_revision: u64,
    pub scope: Option<MarketScope>,
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub quote_ids: Vec<String>,
    #[serde(default)]
    pub layers: Vec<LayerReport>,
    pub rule_version: String,
    pub created_at: DateTime<Utc>,
    pub changes: Vec<FieldChange>,
    pub mutations: Vec<FileMutation>,
    #[serde(default)]
    pub read_set: Vec<FileExpectation>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

impl PatchPlan {
    pub fn seal(&mut self) -> Result<(), serde_json::Error> {
        self.id.clear();
        self.id = digest(&serde_json::to_vec(self)?);
        Ok(())
    }
    pub fn verify_seal(&self) -> bool {
        let mut copy = self.clone();
        let expected = copy.id.clone();
        copy.seal().is_ok()
            && copy.id == expected
            && self.mutations.iter().all(|m| match &m.rebuild {
                Some(recipe) => {
                    !m.remove
                        && m.content.is_empty()
                        && recipe.output_len().is_some()
                        && m.before_hash.as_ref() == Some(&recipe.source_hash)
                }
                None => digest(&m.content) == m.after_hash && (!m.remove || m.content.is_empty()),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Preparing,
    Prepared,
    Writing,
    Verifying,
    Committed,
    RolledBack,
    FailedUnchanged,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    #[serde(default, skip_serializing_if = "crate::GameId::is_poe2")]
    pub game: crate::GameId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<MarketScope>,
    pub id: String,
    pub plan_id: String,
    pub installation_id: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: OperationState,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedField {
    pub baseline: String,
    pub output: String,
    pub feature: Feature,
}

pub type OwnedFields = BTreeMap<String, OwnedField>;

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreDecision<'a> {
    Restore(&'a str),
    AlreadyRestored,
    Conflict,
}

pub fn restore_field<'a>(owned: &'a OwnedField, current: &str) -> RestoreDecision<'a> {
    if current == owned.output {
        RestoreDecision::Restore(&owned.baseline)
    } else if current == owned.baseline {
        RestoreDecision::AlreadyRestored
    } else {
        RestoreDecision::Conflict
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerReport {
    pub feature: Feature,
    pub enabled: bool,
    pub records: usize,
    pub changed: usize,
    pub skipped: usize,
    pub detail: String,
}
