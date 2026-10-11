//! Versioned checkpoint blobs shared by objective modules, general rules and
//! the runtime. All timestamps are stored relative to the exporting tick's
//! `now_ms` (elapsed ages or remaining durations), never as absolute
//! timestamps: restored processes rebase them onto their own clock.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

/// Current checkpoint schema. Bump when a blob layout changes incompatibly and
/// add a migration branch in the owning restore implementation.
pub const CHECKPOINT_SCHEMA: u32 = 1;
/// Oldest schema version restore implementations still accept.
pub const MIN_SUPPORTED_SCHEMA: u32 = 1;

/// A module-defined state blob with versioning and integrity metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModuleState {
    pub schema: u32,
    /// FNV-1a hash of the prepared module settings at export time. A mismatch
    /// means the checkpoint cannot describe this configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_fingerprint: Option<String>,
    /// True when state was captured outside a safe interruption point.
    #[serde(default)]
    pub degraded: bool,
    /// Human-readable quality notes (in-flight work discarded, partial data).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<String>,
    /// Module-defined state payload.
    #[serde(default)]
    pub data: Value,
}
impl ModuleState {
    pub fn clean(settings_fingerprint: String, data: Value) -> Self {
        Self {
            schema: CHECKPOINT_SCHEMA,
            settings_fingerprint: Some(settings_fingerprint),
            degraded: false,
            issues: Vec::new(),
            data,
        }
    }
    pub fn degraded(data: Value, issues: Vec<String>) -> Self {
        Self {
            schema: CHECKPOINT_SCHEMA,
            settings_fingerprint: None,
            degraded: true,
            issues,
            data,
        }
    }
    pub fn check_schema(&self) -> Result<(), RestoreError> {
        if self.schema > CHECKPOINT_SCHEMA {
            return Err(RestoreError::SchemaTooNew {
                found: self.schema,
                supported: CHECKPOINT_SCHEMA,
            });
        }
        if self.schema < MIN_SUPPORTED_SCHEMA {
            return Err(RestoreError::SchemaTooOld {
                found: self.schema,
                supported: MIN_SUPPORTED_SCHEMA,
            });
        }
        Ok(())
    }
    pub fn check_settings(&self, fingerprint: &str) -> Result<(), RestoreError> {
        if self.settings_fingerprint.as_deref() != Some(fingerprint) {
            return Err(RestoreError::SettingsMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("checkpoint schema {found} is newer than supported schema {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("checkpoint schema {found} is no longer supported (oldest {supported})")]
    SchemaTooOld { found: u32, supported: u32 },
    #[error("checkpoint was written for different module settings")]
    SettingsMismatch,
    #[error("checkpoint does not fit this module: {0}")]
    ShapeMismatch(String),
    #[error("checkpoint is missing required field {0}")]
    MissingField(&'static str),
    #[error("checkpoint is corrupt: {0}")]
    Corrupt(String),
}

/// Stable fingerprint of prepared settings. serde_json objects serialize in
/// key order, so equal values produce identical digests.
pub fn settings_fingerprint(settings: &Value) -> String {
    const BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let bytes = serde_json::to_vec(settings).unwrap_or_default();
    let hash = bytes
        .iter()
        .fold(BASIS, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}

/// Milliseconds since an absolute timestamp, or `None` for absent/zero stamps.
pub fn age_ms(at: Option<u64>, now_ms: u64) -> Option<u64> {
    let at = at.filter(|at| *at > 0)?;
    Some(now_ms.saturating_sub(at))
}
/// Milliseconds until an absolute deadline, or `None` for absent/zero stamps.
pub fn remaining_ms(deadline: Option<u64>, now_ms: u64) -> Option<u64> {
    let deadline = deadline.filter(|deadline| *deadline > 0)?;
    Some(deadline.saturating_sub(now_ms))
}
/// Turn a stored age back into a timestamp relative to the restore tick.
pub fn rebase_age(now_ms: u64, age: Option<u64>) -> Option<u64> {
    age.map(|age| now_ms.saturating_sub(age))
}
/// Turn a stored remaining duration back into a deadline.
pub fn rebase_deadline(now_ms: u64, remaining: Option<u64>) -> Option<u64> {
    remaining.map(|remaining| now_ms.saturating_add(remaining))
}

/// Terminal output kept so a restored sequence reports the same result.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalOutput {
    pub state: TerminalState,
    pub status: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    Completed,
    Failed,
    Cancelled,
}

/// Retained [`crate::TaskSequence`] progress. Sequences resume at step
/// boundaries only; an in-flight operation is never reconstructed — its step
/// re-issues the same idempotency key instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SequenceCheckpoint {
    pub schema: u32,
    /// Ordered step names at export, used to validate the rebuilt sequence.
    #[serde(default)]
    pub step_names: Vec<String>,
    pub index: usize,
    #[serde(default)]
    pub current_step: Option<String>,
    #[serde(default)]
    pub attempt: u32,
    /// Elapsed time already spent in the current step.
    #[serde(default)]
    pub entered_elapsed_ms: Option<u64>,
    /// Time left before a failed step may retry.
    #[serde(default)]
    pub retry_in_ms: Option<u64>,
    #[serde(default)]
    pub results: BTreeMap<String, Value>,
    /// Stable run identity; preserved so retried steps reuse idempotency keys.
    pub run_key: String,
    #[serde(default)]
    pub terminal: Option<TerminalOutput>,
}
