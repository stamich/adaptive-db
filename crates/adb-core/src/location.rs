//! Physical address of a tuple inside a heap file.
use serde::{Deserialize, Serialize};

use crate::{PageId, SlotId};

/// Points at one slot of one heap page. Locations are only meaningful to the heap that issued them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowLocation {
    /// Heap page holding the tuple.
    pub page_id: PageId,
    /// Slot inside the page.
    pub slot_id: SlotId,
}
