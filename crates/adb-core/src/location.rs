//! Location module for the adb-core crate.
//!
use serde::{Deserialize, Serialize};

use crate::{PageId, SlotId};

/// Represents `RowLocation` state used by the src subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowLocation {
    pub page_id: PageId,
    pub slot_id: SlotId,
}
