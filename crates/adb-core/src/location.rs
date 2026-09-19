//! Module `location` for crate `adb-core`.
use serde::{Deserialize, Serialize};

use crate::{PageId, SlotId};

/// Represents `RowLocation` state used by this subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowLocation {
    pub page_id: PageId,
    pub slot_id: SlotId,
}
