//! Versioned JSON wire format of physical plans (the JVM -> native boundary).
//!
//! A plan travels inside an envelope that names the wire version:
//!
//! ```json
//! {"wire_version": 2, "plan": {"op": "entity_scan", "entity_id": 7, "columns": [...]}}
//! ```
//!
//! [`decode_json`] is the only way the native side accepts a plan: it bounds the input size,
//! checks the version *before* interpreting the plan (so a JVM/native version mismatch is
//! reported as such, not as a confusing parse error), rejects unknown fields, and validates the
//! decoded plan. [`encode_json`] is the symmetric producer used by Rust tools and tests.
use adb_execution::{PhysicalPlan, PlanShape};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current plan wire version. 1 = Milestones 1.7–2.0.3 (bare plan, field ids); 2 = Milestone
/// 2.1 (envelope, slots, joins, aggregates, sorts).
pub const PLAN_WIRE_VERSION: u32 = 2;
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
    /// The plan tree.
    plan: PhysicalPlan,
}

/// Just the version of an envelope (read before the plan is interpreted).
#[derive(Deserialize)]
struct VersionProbe {
    /// Wire version of the document.
    wire_version: Option<u32>,
}

/// Validates `plan` and encodes it as a version-2 envelope.
pub fn encode_json(plan: &PhysicalPlan) -> Result<Vec<u8>, PlanWireError> {
    plan.validate().map_err(PlanWireError::Invalid)?;
    let bytes = serde_json::to_vec(&Envelope {
        wire_version: PLAN_WIRE_VERSION,
        plan: plan.clone(),
    })
    .map_err(|error| PlanWireError::Malformed(error.to_string()))?;
    check_size(bytes.len())?;
    Ok(bytes)
}

/// Decodes and validates a plan document; returns the plan and its shape.
pub fn decode_json(bytes: &[u8]) -> Result<(PhysicalPlan, PlanShape), PlanWireError> {
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
    Ok((envelope.plan, shape))
}

/// Rejects empty and oversized documents.
fn check_size(len: usize) -> Result<(), PlanWireError> {
    if len == 0 || len > MAX_PLAN_WIRE_BYTES {
        return Err(PlanWireError::Size(len));
    }
    Ok(())
}
