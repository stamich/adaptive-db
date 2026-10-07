//! Hash equi-join (INNER / LEFT).
use std::collections::HashMap;

use crate::{
    key::{join_key, KeyPart},
    memory::MemoryReservation,
    operator::{
        collect::collect_all_bounded,
        join::{check_fanout, combine, fill_batch, finish_left_row, LeftStream},
        Operator, RowBatch,
    },
    ExecRow, ExecutionContext, ExecutionError, Expr, JoinType, SlotId,
};

/// Approximate bytes of one hash-table entry besides its key (bucket, vector header).
const ENTRY_OVERHEAD_BYTES: usize = 48;

/// Materialized build side.
struct BuildSide {
    /// Every build row, addressed by position.
    rows: Vec<ExecRow>,
    /// Join key -> positions of the build rows with that key.
    index: HashMap<Vec<KeyPart>, Vec<u32>>,
    /// Memory held by `rows` and `index`; released with the build side.
    _reservation: MemoryReservation,
}

/// Builds a hash table over the right input on the first pull, then streams the left input
/// through it. Output order follows the left input.
pub struct HashJoinOperator {
    /// Probe side.
    left: LeftStream,
    /// Build side input, consumed by the first pull.
    right: Box<dyn Operator>,
    /// The hash table, once built.
    build: Option<BuildSide>,
    /// INNER or LEFT.
    join_type: JoinType,
    /// Key slots on the left input.
    left_keys: Vec<SlotId>,
    /// Key slots on the right input, aligned with `left_keys`.
    right_keys: Vec<SlotId>,
    /// Slots produced by the right input (copied into joined rows).
    right_slots: Vec<SlotId>,
    /// Extra condition checked on each key match.
    residual: Option<Expr>,
}

impl HashJoinOperator {
    /// Joins `left` and `right` on `left_keys[i] = right_keys[i]`.
    pub fn new(
        left: Box<dyn Operator>,
        right: Box<dyn Operator>,
        join_type: JoinType,
        (left_keys, right_keys): (Vec<SlotId>, Vec<SlotId>),
        right_slots: Vec<SlotId>,
        residual: Option<Expr>,
    ) -> Self {
        Self {
            left: LeftStream::new(left),
            right,
            build: None,
            join_type,
            left_keys,
            right_keys,
            right_slots,
            residual,
        }
    }

    /// Materializes the right input and indexes it by join key, reserving memory for both.
    fn build(&mut self, context: &ExecutionContext) -> Result<BuildSide, ExecutionError> {
        let mut reservation = context.memory.reservation("hash join build side");
        let rows = collect_all_bounded(
            self.right.as_mut(),
            context,
            &mut reservation,
            "hash join build side",
        )?;
        let mut index: HashMap<Vec<KeyPart>, Vec<u32>> = HashMap::new();
        for (position, row) in rows.iter().enumerate() {
            // NULL keys never match, so they are not indexed at all.
            let Some(key) = join_key(row, &self.right_keys) else {
                continue;
            };
            let key_bytes: usize = key.iter().map(KeyPart::estimated_bytes).sum();
            reservation.grow(key_bytes + ENTRY_OVERHEAD_BYTES)?;
            // Positions fit in u32: collect_all_bounded caps rows far below u32::MAX.
            index.entry(key).or_default().push(position as u32);
        }
        Ok(BuildSide {
            rows,
            index,
            _reservation: reservation,
        })
    }
}

impl Operator for HashJoinOperator {
    /// Builds on the first call, then probes left rows until a batch is full.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.build.is_none() {
            self.build = Some(self.build(context)?);
        }
        let Self {
            left,
            build,
            join_type,
            left_keys,
            right_slots,
            residual,
            ..
        } = self;
        let build = build.as_ref().expect("built above");

        fill_batch(left, context, |left_row, out| {
            let mut matches = 0;
            let candidates = join_key(&left_row, left_keys)
                .and_then(|key| build.index.get(&key))
                .map_or(&[][..], Vec::as_slice);
            for position in candidates {
                let joined = combine(&left_row, &build.rows[*position as usize], right_slots);
                if let Some(residual) = residual {
                    if !residual.evaluate_bool(&joined)? {
                        continue;
                    }
                }
                matches += 1;
                check_fanout(matches, context)?;
                out.push(joined);
            }
            finish_left_row(left_row, matches, *join_type, right_slots, out);
            Ok(())
        })
    }
}
