//! Half-open ranges over [`RowId`] used by scans and change-data-capture filters.
use std::ops::Bound;

use serde::{Deserialize, Serialize};

use crate::RowId;

/// A half-open key range `[start, end)`; `end == None` means "to the end of the key space".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRange {
    /// Inclusive lower bound.
    pub start: RowId,
    /// Exclusive upper bound, or unbounded.
    pub end: Option<RowId>,
}

impl KeyRange {
    /// The whole key space (every entity).
    pub const fn all() -> Self {
        Self {
            start: RowId(0),
            end: None,
        }
    }

    /// All rows of one entity: `[entity << 64, (entity + 1) << 64)`.
    pub const fn entity(entity_id: u64) -> Self {
        let start = RowId::compose(entity_id, 0);
        let end = if entity_id == u64::MAX {
            None
        } else {
            Some(RowId::compose(entity_id + 1, 0))
        };
        Self { start, end }
    }

    /// Index bounds for the part of the range strictly after `after` (keyset pagination).
    pub fn bounds_after(&self, after: Option<RowId>) -> (Bound<RowId>, Bound<RowId>) {
        let start = match after {
            Some(after) if after >= self.start => Bound::Excluded(after),
            _ => Bound::Included(self.start),
        };
        let end = self.end.map_or(Bound::Unbounded, Bound::Excluded);
        (start, end)
    }

    /// Returns whether `row_id` lies inside the range.
    pub fn contains(&self, row_id: RowId) -> bool {
        row_id >= self.start && self.end.is_none_or(|end| row_id < end)
    }
}

/// Unit tests of key ranges.
#[cfg(test)]
mod tests {
    use super::*;

    /// An entity range includes all of its keys and none of its neighbours'.
    #[test]
    fn entity_range_contains_exactly_its_entity() {
        let range = KeyRange::entity(3);
        assert!(range.contains(RowId::compose(3, 0)));
        assert!(range.contains(RowId::compose(3, u64::MAX)));
        assert!(!range.contains(RowId::compose(2, u64::MAX)));
        assert!(!range.contains(RowId::compose(4, 0)));
    }

    /// Keyset pagination resumes strictly after the cursor and ignores cursors before the range.
    #[test]
    fn bounds_after_resumes_strictly_after_the_cursor() {
        let range = KeyRange::entity(1);
        let (start, end) = range.bounds_after(None);
        assert_eq!(start, Bound::Included(RowId::compose(1, 0)));
        assert_eq!(end, Bound::Excluded(RowId::compose(2, 0)));
        let (start, _) = range.bounds_after(Some(RowId::compose(1, 5)));
        assert_eq!(start, Bound::Excluded(RowId::compose(1, 5)));
        let (start, _) = range.bounds_after(Some(RowId::compose(0, 5)));
        assert_eq!(start, Bound::Included(RowId::compose(1, 0)));
    }

    /// The range of the last entity id extends to the end of the key space.
    #[test]
    fn last_entity_range_is_unbounded() {
        let range = KeyRange::entity(u64::MAX);
        assert_eq!(range.end, None);
        assert!(range.contains(RowId(u128::MAX)));
    }
}
