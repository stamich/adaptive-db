//! The common contract of derived projections, and its implementation for secondary indexes.
//!
//! Everything the engine stores besides the canonical log is a projection of it: the current
//! state, the version store, and since 2.3.0 one projection per secondary index. A projection
//! is fed committed transactions in log order, can always be rebuilt from the log, and serves
//! a snapshot only within the range of commits it reflects:
//!
//! ```text
//! serves(s)  <=>  usable_from() <= s <= watermark()
//! ```
//!
//! * `watermark` is the latest commit the projection reflects. Indexes are maintained
//!   synchronously inside the commit, so their watermark is always the latest applied commit;
//!   a future asynchronous projection (columnar, graph) fed by the change feed would lag
//!   behind it, and the same rule keeps the planner from using it for newer snapshots.
//! * `usable_from` is the oldest snapshot it can answer: an index holds no entries for history
//!   before it was built, and vacuum removes entries that ended before the vacuum horizon.

use std::collections::HashMap;

use adb_core::{CommitTs, Lsn, Row, RowId};
use adb_index::{IndexDefinition, IndexProjection};
use adb_tx::Mutation;
use parking_lot::Mutex;

use crate::{committed::CommittedTx, DbError};

/// A projection of the canonical log.
pub trait Projection: Send + Sync {
    /// Name for messages and EXPLAIN.
    fn name(&self) -> String;

    /// Applies one committed transaction whose `Commit` record is at `lsn`.
    fn apply(&self, tx: &CommittedTx, lsn: Lsn) -> Result<(), DbError>;

    /// Latest commit the projection reflects.
    fn watermark(&self) -> CommitTs;

    /// Oldest snapshot the projection can serve.
    fn usable_from(&self) -> CommitTs;

    /// Whether the projection can answer a query at `snapshot`.
    fn serves(&self, snapshot: CommitTs) -> bool {
        self.usable_from() <= snapshot && snapshot <= self.watermark()
    }

    /// Structural self-check; returns the number of entries examined.
    fn verify(&self) -> Result<u64, DbError>;
}

/// One index projection inside the engine: the persistent [`IndexProjection`] plus the vacuum
/// horizon it was trimmed to.
pub struct IndexHandle {
    /// The persistent projection.
    pub projection: IndexProjection,
    /// Entries that ended at or before this commit were vacuumed.
    vacuumed_through: Mutex<CommitTs>,
}

impl IndexHandle {
    /// Wraps an opened projection that was vacuumed through `vacuumed_through`.
    pub fn new(projection: IndexProjection, vacuumed_through: CommitTs) -> Self {
        Self {
            projection,
            vacuumed_through: Mutex::new(vacuumed_through),
        }
    }

    /// The index definition.
    pub fn definition(&self) -> &IndexDefinition {
        self.projection.definition()
    }

    /// Checks that every row written into this index's entity fits the index limits.
    pub fn check<'a>(
        &self,
        mutations: impl IntoIterator<Item = (&'a RowId, &'a Mutation)>,
    ) -> Result<(), DbError> {
        let entity = self.definition().entity_id;
        for (row_id, mutation) in mutations {
            if let (true, Mutation::Put(row)) = (row_id.entity_id() == entity, mutation) {
                self.projection.check_row(row)?;
            }
        }
        Ok(())
    }

    /// Records that the persisted entries were vacuumed through `horizon` (on open).
    pub fn restore_vacuum_horizon(&self, horizon: CommitTs) {
        let mut through = self.vacuumed_through.lock();
        *through = (*through).max(horizon);
    }

    /// Removes entries that ended at or before `horizon` and records the horizon.
    pub fn vacuum(&self, horizon: CommitTs, lsn: Lsn) -> Result<u64, DbError> {
        let removed = self.projection.vacuum(horizon, lsn)?;
        let mut through = self.vacuumed_through.lock();
        *through = (*through).max(horizon);
        Ok(removed)
    }
}

impl Projection for IndexHandle {
    /// `index <name>`.
    fn name(&self) -> String {
        format!("index {}", self.definition().name)
    }

    /// Applies the changes of the index's entity: each row's before-image and new state.
    fn apply(&self, tx: &CommittedTx, lsn: Lsn) -> Result<(), DbError> {
        let entity = self.definition().entity_id;
        let mut before: HashMap<RowId, Option<&Row>> = HashMap::new();
        for (row_id, version) in &tx.before_images {
            if row_id.entity_id() == entity {
                before.insert(*row_id, version.value.as_ref());
            }
        }
        for (row_id, mutation) in &tx.mutations {
            if row_id.entity_id() != entity {
                continue;
            }
            let after = match mutation {
                Mutation::Put(row) => Some(row),
                Mutation::Delete => None,
            };
            let old = before.get(row_id).copied().flatten();
            self.projection
                .apply_row(*row_id, old, after, tx.commit_ts, lsn)?;
        }
        self.projection.set_watermark(tx.commit_ts);
        Ok(())
    }

    /// Latest applied commit.
    fn watermark(&self) -> CommitTs {
        self.projection.watermark()
    }

    /// `max(created_at_ts, vacuum horizon)`.
    fn usable_from(&self) -> CommitTs {
        self.definition()
            .created_at_ts
            .max(*self.vacuumed_through.lock())
    }

    /// Decodes every entry and checks the entry invariants.
    fn verify(&self) -> Result<u64, DbError> {
        Ok(self.projection.verify_structure()?)
    }
}
