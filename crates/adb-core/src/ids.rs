//! Ids module for the adb-core crate.
//!
use serde::{Deserialize, Serialize};

/// Represents `RowId` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowId(pub u128);

/// Represents `TxId` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TxId(pub u64);

/// Represents `CommitTs` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommitTs(pub u64);

/// Represents `Lsn` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lsn(pub u64);

/// Represents `PageId` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PageId(pub u64);

/// Defines the `SlotId` type alias used by this subsystem.
pub type SlotId = u16;
/// Defines the `FieldId` type alias used by this subsystem.
pub type FieldId = u32;
