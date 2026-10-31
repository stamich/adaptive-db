//! Versioned JSON wire format of physical plans (the JVM -> native boundary).
//!
//! A plan travels inside an envelope that names the wire version and, since version 4
//! (Milestone 2.4.0), the catalog epoch the plan was made at:
//!
//! ```json
//! {"wire_version": 4, "schema_epoch": 12, "plan": {"op": "entity_scan", "entity_id": 7, "columns": [...]}}
//! ```
//!
//! The engine refuses a plan whose epoch is not the current one
//! (`ExecutionError::SchemaChanged`): a table or index it was planned against may have changed,
//! so the planner plans it again instead of the engine running it on a different catalog.
//!
//! [`decode_json`] is the only way the native side accepts a plan: it bounds the input size,
//! checks the version *before* interpreting the plan (so a JVM/native version mismatch is
//! reported as such, not as a confusing parse error), rejects unknown fields, and validates the
//! decoded plan. [`encode_json`] is the symmetric producer used by Rust tools and tests.
use adb_core::SchemaEpoch;
use adb_execution::{PhysicalPlan, PlanShape};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current plan wire version. 1 = Milestones 1.7–2.0.3 (bare plan, field ids); 2 = Milestone
/// 2.1 (envelope, slots, joins, aggregates, sorts); 3 = Milestone 2.3.0 (`index_scan` and
/// `row_fetch`); 4 = Milestone 2.4.0 (the envelope carries `schema_epoch`). The JVM and the
/// engine are upgraded together, so only the current version is accepted: a document from an
/// older planner is reported as a version mismatch.
pub const PLAN_WIRE_VERSION: u32 = 4;
/// Largest encoded plan accepted, in bytes.
pub const MAX_PLAN_WIRE_BYTES: usize = 8 * 1024 * 1024;

/// Why a plan could not be encoded or decoded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanWireError {
    /// The document exceeds [`MAX_PLAN_WIRE_BYTES`] (or is empty).
    #[error("plan document is {0} bytes; accepted size is 1..={MAX_PLAN_WIRE_BYTES}")]
    Size(usize),
    /// The document is not a well-formed envelope or plan.
    #[error("malformed plan document: {0}")]
    Malformed(String),
    /// The envelope names a wire version this engine does not speak.
    #[error(
        "unsupported plan wire version {found}; this engine speaks version {PLAN_WIRE_VERSION}"
    )]
    UnsupportedVersion {
        /// Version found in the document.
        found: u32,
    },
    /// The plan decoded but violates a structural rule.
    #[error("invalid plan: {0}")]
    Invalid(String),
}

/// Envelope as written on the wire.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    /// Wire version of `plan`.
    wire_version: u32,
    /// Catalog epoch the plan was made at.
    schema_epoch: u64,
    /// The plan tree.
    plan: PhysicalPlan,
}

/// A decoded, validated plan document.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedPlan {
    /// The plan tree.
    pub plan: PhysicalPlan,
    /// Its shape (row width, output slots).
    pub shape: PlanShape,
    /// Catalog epoch the plan was made at.
    pub schema_epoch: SchemaEpoch,
}

/// Just the version of an envelope (read before the plan is interpreted).
#[derive(Deserialize)]
struct VersionProbe {
    /// Wire version of the document.
    wire_version: Option<u32>,
}

/// Validates `plan` and encodes it in the current envelope, made at `schema_epoch`.
pub fn encode_json(
    plan: &PhysicalPlan,
    schema_epoch: SchemaEpoch,
) -> Result<Vec<u8>, PlanWireError> {
    plan.validate().map_err(PlanWireError::Invalid)?;
    let bytes = serde_json::to_vec(&Envelope {
        wire_version: PLAN_WIRE_VERSION,
        schema_epoch: schema_epoch.0,
        plan: plan.clone(),
    })
    .map_err(|error| PlanWireError::Malformed(error.to_string()))?;
    check_size(bytes.len())?;
    Ok(bytes)
}

/// Decodes and validates a plan document.
pub fn decode_json(bytes: &[u8]) -> Result<DecodedPlan, PlanWireError> {
    check_size(bytes.len())?;
    let probe: VersionProbe = serde_json::from_slice(bytes)
        .map_err(|error| PlanWireError::Malformed(error.to_string()))?;
    match probe.wire_version {
        Some(PLAN_WIRE_VERSION) => {}
        Some(found) => return Err(PlanWireError::UnsupportedVersion { found }),
        None => return Err(PlanWireError::UnsupportedVersion { found: 1 }),
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|error| PlanWireError::Malformed(error.to_string()))?;
    let shape = envelope.plan.validate().map_err(PlanWireError::Invalid)?;
    Ok(DecodedPlan {
        plan: envelope.plan,
        shape,
        schema_epoch: SchemaEpoch(envelope.schema_epoch),
    })
}

/// Rejects empty and oversized documents.
fn check_size(len: usize) -> Result<(), PlanWireError> {
    if len == 0 || len > MAX_PLAN_WIRE_BYTES {
        return Err(PlanWireError::Size(len));
    }
    Ok(())
}
