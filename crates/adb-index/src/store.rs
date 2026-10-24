//! The persistent projection of one index: maintenance, scans, vacuum, build and checks.
//!
//! [`IndexProjection`] owns a [`ByteTree`] of entries (see [`crate::entry`]) and keeps it equal to
//! the index of the rows the log describes:
//!
//! * [`IndexProjection::apply_row`] applies one committed row change. An unchanged key and
//!   covered payload write nothing (an update of an unindexed column costs no index write);
//!   otherwise the row's open entry is ended at the commit and a new one is started.
//! * [`IndexProjection::scan`] reads a bounded page of entries visible at a snapshot, in index
//!   order or reversed, and returns a resume key for the next page (keyset paging: no state is
//!   held between pages, so memory is bounded by the page size).
//! * [`IndexProjection::vacuum`] removes entries that ended at or before a horizon; snapshots
//!   older than the horizon then fall back to table scans.
//! * Like the other projections it is written only in memory and reaches disk through the
//!   checkpoint journal ([`IndexProjection::journal_writes`]).
//!
//! The projection also tracks its **watermark**, the latest commit it reflects. Indexes are
//! maintained synchronously inside the commit, so the watermark is the latest applied commit;
//! the planner's rule "a projection serves snapshot `s` when `usable_from <= s <= watermark`"
//! is written for any projection, including future asynchronous ones.
use std::{
    ops::Bound,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use adb_btree::{ByteTree, TreeShape};
use adb_core::{prefix_successor, CommitTs, Lsn, Row, RowId};
use adb_journal::FileWrite;

use crate::{
    bounds::IndexBounds,
    entry::{
        decode_entry, encode_included, encode_index_key, end_of, row_prefix, suffix_of, tree_key,
        tree_value, OPEN,
    },
    IndexDefinition, IndexEntry, IndexError,
};

/// Order in which a scan returns entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScanDirection {
    /// Index order.
    #[default]
    Forward,
    /// Reverse index order.
    Backward,
}

/// One page of a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanPage {
    /// Entries visible at the snapshot, in scan order.
    pub entries: Vec<IndexEntry>,
    /// Tree key to resume after, or `None` when the range is exhausted.
    pub resume: Option<Vec<u8>>,
    /// Tree entries examined (visible or not), for profiles.
    pub examined: u64,
}

/// Encoded key and covered payload of one row under an index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Encoded {
    /// Encoded key columns.
    key: Vec<u8>,
    /// Encoded covered values.
    included: Vec<u8>,
}

/// The persistent projection of one index.
pub struct IndexProjection {
    /// The index this projection maintains.
    definition: IndexDefinition,
    /// Directory of the projection's files.
    dir: PathBuf,
    /// Entries.
    tree: ByteTree,
    /// Latest commit reflected (see the module documentation).
    watermark: AtomicU64,
}

/// File of the entry tree inside the projection directory.
const TREE_FILE: &str = "entries.idx";
/// Root pointer of the entry tree.
const TREE_META_FILE: &str = "entries.idx.meta";

impl IndexProjection {
    /// Opens (creating if needed) the projection of `definition` in `dir`; `watermark` is the
    /// latest commit the persisted state reflects.
    pub fn open(
        dir: impl AsRef<Path>,
        definition: IndexDefinition,
        buffer_pages: usize,
        watermark: CommitTs,
    ) -> Result<Self, IndexError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let tree = ByteTree::open(dir.join(TREE_FILE), dir.join(TREE_META_FILE), buffer_pages)?;
        Ok(Self {
            definition,
            dir,
            tree,
            watermark: AtomicU64::new(watermark.0),
        })
    }

    /// The index definition.
    pub fn definition(&self) -> &IndexDefinition {
        &self.definition
    }

    /// Directory of the projection's files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Latest commit the projection reflects.
    pub fn watermark(&self) -> CommitTs {
        CommitTs(self.watermark.load(Ordering::Acquire))
    }

    /// Checks that `row` fits the index limits; called before a commit is logged, so a row that
    /// cannot be indexed is rejected instead of failing after the commit point.
    pub fn check_row(&self, row: &Row) -> Result<(), IndexError> {
        self.encode(row).map(|_| ())
    }

    /// Applies one committed change of `row_id` (an entity row of this index): `before` is the
    /// row as it was (`None` if it did not exist), `after` as it is now (`None` if deleted).
    pub fn apply_row(
        &self,
        row_id: RowId,
        before: Option<&Row>,
        after: Option<&Row>,
        commit_ts: CommitTs,
        lsn: Lsn,
    ) -> Result<(), IndexError> {
        let old = before.map(|row| self.encode(row)).transpose()?;
        let new = after.map(|row| self.encode(row)).transpose()?;
        if old != new {
            if let Some(old) = &old {
                self.end_open_entry(&old.key, row_id, commit_ts, lsn)?;
            }
            if let Some(new) = &new {
                self.tree.insert(
                    &tree_key(&new.key, row_id, commit_ts),
                    &tree_value(OPEN, &new.included),
                    lsn,
                )?;
            }
        }
        self.watermark.fetch_max(commit_ts.0, Ordering::AcqRel);
        Ok(())
    }

    /// Adds the open entry of a row that exists since `begin_ts` (used by builds).
    pub fn insert_current(
        &self,
        row_id: RowId,
        row: &Row,
        begin_ts: CommitTs,
        lsn: Lsn,
    ) -> Result<(), IndexError> {
        let encoded = self.encode(row)?;
        self.tree.insert(
            &tree_key(&encoded.key, row_id, begin_ts),
            &tree_value(OPEN, &encoded.included),
            lsn,
        )?;
        Ok(())
    }

    /// Sets the watermark (after a build that included every commit up to `ts`).
    pub fn set_watermark(&self, ts: CommitTs) {
        self.watermark.fetch_max(ts.0, Ordering::AcqRel);
    }

    /// Up to `limit` entries matching `bounds` that are visible at `snapshot`, in `direction`,
    /// strictly after the tree key `resume` of the previous page. At most `max_examined` tree
    /// entries are read, so a page over a long stretch of invisible history stays bounded; it
    /// may then hold fewer than `limit` entries while `resume` is still set.
    pub fn scan(
        &self,
        bounds: &IndexBounds,
        snapshot: CommitTs,
        direction: ScanDirection,
        resume: Option<&[u8]>,
        limit: usize,
        max_examined: usize,
    ) -> Result<ScanPage, IndexError> {
        let range = bounds.byte_range(&self.definition)?;
        let max_examined = max_examined.max(1);
        let raw = match direction {
            ScanDirection::Forward => {
                let lower = match resume {
                    Some(key) => Bound::Excluded(key),
                    None => as_ref(&range.lower),
                };
                self.tree.range(lower, as_ref(&range.upper), max_examined)?
            }
            ScanDirection::Backward => {
                let upper = match resume {
                    Some(key) => Bound::Excluded(key),
                    None => as_ref(&range.upper),
                };
                self.tree
                    .range_rev(as_ref(&range.lower), upper, max_examined)?
            }
        };
        let mut page = ScanPage {
            entries: Vec::new(),
            resume: None,
            examined: 0,
        };
        let exhausted = raw.len() < max_examined;
        let total = raw.len();
        for (index, (key, value)) in raw.into_iter().enumerate() {
            page.examined += 1;
            // Cheap visibility test before decoding the values.
            let (_, begin) = suffix_of(&key)?;
            let end = end_of(&value)?;
            if begin <= snapshot && snapshot < end {
                page.entries
                    .push(decode_entry(&self.definition, &key, &value)?);
            }
            let last = index + 1 == total;
            if page.entries.len() >= limit.max(1) || last {
                if !(last && exhausted) {
                    page.resume = Some(key);
                }
                break;
            }
        }
        Ok(page)
    }

    /// Removes entries that ended at or before `horizon`; returns how many.
    pub fn vacuum(&self, horizon: CommitTs, lsn: Lsn) -> Result<u64, IndexError> {
        const PAGE: usize = 1024;
        let mut removed = 0;
        let mut after: Option<Vec<u8>> = None;
        loop {
            let lower = after.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
            let page = self.tree.range(lower, Bound::Unbounded, PAGE)?;
            let Some((last, _)) = page.last() else {
                return Ok(removed);
            };
            after = Some(last.clone());
            for (key, value) in page {
                if end_of(&value)? <= horizon {
                    self.tree.remove(&key, lsn)?;
                    removed += 1;
                }
            }
        }
    }

    /// Every open (current) entry, in index order (checks and statistics).
    pub fn open_entries(&self) -> Result<Vec<IndexEntry>, IndexError> {
        let mut out = Vec::new();
        self.for_each_entry(|entry| {
            if entry.end_ts == OPEN {
                out.push(entry);
            }
            Ok(())
        })?;
        Ok(out)
    }

    /// Calls `visit` for every entry in index order.
    pub fn for_each_entry(
        &self,
        mut visit: impl FnMut(IndexEntry) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        const PAGE: usize = 1024;
        let mut after: Option<Vec<u8>> = None;
        loop {
            let lower = after.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
            let page = self.tree.range(lower, Bound::Unbounded, PAGE)?;
            let Some((last, _)) = page.last() else {
                return Ok(());
            };
            after = Some(last.clone());
            for (key, value) in page {
                visit(decode_entry(&self.definition, &key, &value)?)?;
            }
        }
    }

    /// Structural check: every entry decodes, `begin < end`, and every row has at most one
    /// open entry. Returns the number of entries.
    pub fn verify_structure(&self) -> Result<u64, IndexError> {
        let mut open_rows = std::collections::HashSet::new();
        let mut count = 0;
        self.for_each_entry(|entry| {
            count += 1;
            if entry.begin_ts >= entry.end_ts {
                return Err(IndexError::Corrupt(format!(
                    "index {}: entry of {:?} ends before it begins",
                    self.definition.name, entry.row_id
                )));
            }
            if entry.end_ts == OPEN && !open_rows.insert(entry.row_id) {
                return Err(IndexError::Corrupt(format!(
                    "index {}: {:?} has two current entries",
                    self.definition.name, entry.row_id
                )));
            }
            Ok(())
        })?;
        Ok(count)
    }

    /// Height, pages and entries of the tree.
    pub fn shape(&self) -> Result<TreeShape, IndexError> {
        Ok(self.tree.shape()?)
    }

    /// Pages of the tree file.
    pub fn page_count(&self) -> u64 {
        self.tree.page_count()
    }

    /// Pages changed since the last checkpoint.
    pub fn dirty_pages(&self) -> usize {
        self.tree.dirty_page_count()
    }

    /// Dirty pages and the root pointer, for the checkpoint journal.
    pub fn journal_writes(&self) -> Result<Vec<FileWrite>, IndexError> {
        Ok(self.tree.journal_writes()?)
    }

    /// Marks the state captured by [`journal_writes`](Self::journal_writes) as persisted.
    pub fn mark_clean(&self) {
        self.tree.mark_clean();
    }

    /// Persists everything directly (standalone use and tests; the engine checkpoints through
    /// the journal).
    pub fn flush(&self) -> Result<(), IndexError> {
        Ok(self.tree.flush()?)
    }

    // ----- internals -------------------------------------------------------------------------

    /// Encoded key and covered payload of `row`.
    fn encode(&self, row: &Row) -> Result<Encoded, IndexError> {
        Ok(Encoded {
            key: encode_index_key(&self.definition, &self.definition.key_values(row))?,
            included: encode_included(&self.definition, &self.definition.included_values(row))?,
        })
    }

    /// Ends the open entry of `row_id` under `key` at `commit_ts`.
    fn end_open_entry(
        &self,
        key: &[u8],
        row_id: RowId,
        commit_ts: CommitTs,
        lsn: Lsn,
    ) -> Result<(), IndexError> {
        let prefix = row_prefix(key, row_id);
        let upper = prefix_successor(&prefix).map_or(Bound::Unbounded, Bound::Excluded);
        // Versions of one row under one key ascend by begin_ts: the open one is the last.
        let versions = self
            .tree
            .range_rev(Bound::Included(&prefix), as_ref(&upper), 1)?;
        match versions.into_iter().next() {
            Some((tree_key, value)) if end_of(&value)? == OPEN => {
                let mut ended = value;
                ended[..8].copy_from_slice(&commit_ts.0.to_be_bytes());
                self.tree.insert(&tree_key, &ended, lsn)?;
                Ok(())
            }
            _ => Err(IndexError::Corrupt(format!(
                "index {}: no current entry for {row_id:?}; rebuild the projections",
                self.definition.name
            ))),
        }
    }
}

/// Borrows an owned bound.
fn as_ref(bound: &Bound<Vec<u8>>) -> Bound<&[u8]> {
    match bound {
        Bound::Included(key) => Bound::Included(key),
        Bound::Excluded(key) => Bound::Excluded(key),
        Bound::Unbounded => Bound::Unbounded,
    }
}
