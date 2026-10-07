//! GROUP BY with COUNT / SUM / MIN / MAX / AVG.
use std::{cmp::Ordering, collections::HashMap};

use adb_core::Value;

use crate::{
    expression::compare,
    key::{group_key, KeyPart},
    memory::MemoryReservation,
    operator::{output_buffer::OutputBuffer, Operator, RowBatch},
    AggregateFunction, AggregateSpec, ExecRow, ExecutionContext, ExecutionError, SlotId,
};

/// Approximate fixed bytes of one group besides its key and accumulators.
const GROUP_OVERHEAD_BYTES: usize = 64;

/// Running sum: exact `i128` for integers (so intermediate sums cannot overflow), `f64` for
/// doubles.
#[derive(Debug, Clone, Copy)]
enum Sum {
    /// No non-NULL value seen yet.
    Empty,
    /// Exact integer sum.
    Int(i128),
    /// Floating-point sum.
    Float(f64),
}

impl Sum {
    /// Adds one non-NULL value; the first value fixes the sum's type.
    fn add(&mut self, value: &Value) -> Result<(), ExecutionError> {
        *self = match (*self, value) {
            (Self::Empty, Value::Int64(v)) => Self::Int(i128::from(*v)),
            (Self::Empty, Value::Float64(v)) => Self::Float(*v),
            (Self::Int(acc), Value::Int64(v)) => {
                Self::Int(acc.checked_add(i128::from(*v)).ok_or_else(|| {
                    ExecutionError::ArithmeticOverflow("SUM exceeds the i128 range".into())
                })?)
            }
            (Self::Float(acc), Value::Float64(v)) => Self::Float(acc + v),
            (_, other) => {
                return Err(ExecutionError::Expression(format!(
                    "SUM/AVG cannot add {other:?} to a sum of another type"
                )))
            }
        };
        Ok(())
    }
}

/// State of one aggregate within one group.
#[derive(Debug, Clone)]
enum Accumulator {
    /// `COUNT(*)`: rows.
    CountRows(i64),
    /// `COUNT(x)`: non-NULL values.
    CountValues(i64),
    /// `SUM(x)`.
    Sum(Sum),
    /// `MIN(x)`.
    Min(Option<Value>),
    /// `MAX(x)`.
    Max(Option<Value>),
    /// `AVG(x)`: sum and count of non-NULL values.
    Avg(Sum, i64),
}

impl Accumulator {
    /// Initial state for `spec`.
    fn new(spec: &AggregateSpec) -> Self {
        match (spec.function, spec.input) {
            (AggregateFunction::Count, None) => Self::CountRows(0),
            (AggregateFunction::Count, Some(_)) => Self::CountValues(0),
            (AggregateFunction::Sum, _) => Self::Sum(Sum::Empty),
            (AggregateFunction::Min, _) => Self::Min(None),
            (AggregateFunction::Max, _) => Self::Max(None),
            (AggregateFunction::Avg, _) => Self::Avg(Sum::Empty, 0),
        }
    }

    /// Folds one input value in (`value` is NULL for `COUNT(*)`, which ignores it).
    ///
    /// Returns the change in heap bytes held (MIN/MAX of strings and byte strings keep a copy).
    fn update(&mut self, value: &Value) -> Result<isize, ExecutionError> {
        if let Self::CountRows(count) = self {
            *count += 1;
            return Ok(0);
        }
        if matches!(value, Value::Null) {
            return Ok(0);
        }
        match self {
            Self::CountRows(_) => unreachable!("handled above"),
            Self::CountValues(count) => *count += 1,
            Self::Sum(sum) => sum.add(value)?,
            Self::Avg(sum, count) => {
                sum.add(value)?;
                *count += 1;
            }
            Self::Min(current) => return replace_if(current, value, Ordering::Less),
            Self::Max(current) => return replace_if(current, value, Ordering::Greater),
        }
        Ok(0)
    }

    /// Final value of the aggregate.
    fn finish(&self) -> Result<Value, ExecutionError> {
        Ok(match self {
            Self::CountRows(count) | Self::CountValues(count) => Value::Int64(*count),
            Self::Sum(Sum::Empty) | Self::Avg(Sum::Empty, _) => Value::Null,
            Self::Sum(Sum::Int(acc)) => Value::Int64(i64::try_from(*acc).map_err(|_| {
                ExecutionError::ArithmeticOverflow(format!("SUM {acc} does not fit in INT64"))
            })?),
            Self::Sum(Sum::Float(acc)) => Value::Float64(*acc),
            Self::Avg(Sum::Int(acc), count) => Value::Float64(*acc as f64 / *count as f64),
            Self::Avg(Sum::Float(acc), count) => Value::Float64(*acc / *count as f64),
            Self::Min(value) | Self::Max(value) => value.clone().unwrap_or(Value::Null),
        })
    }
}

/// Replaces `current` with `candidate` if `candidate` compares as `wanted` (Less for MIN,
/// Greater for MAX). Returns the change in held heap bytes.
fn replace_if(
    current: &mut Option<Value>,
    candidate: &Value,
    wanted: Ordering,
) -> Result<isize, ExecutionError> {
    let replace = match current {
        None => true,
        Some(existing) => compare(candidate, existing)? == wanted,
    };
    if !replace {
        return Ok(0);
    }
    let old = current.as_ref().map_or(0, heap_bytes);
    *current = Some(candidate.clone());
    Ok(heap_bytes(candidate) as isize - old as isize)
}

/// Heap bytes owned by a value.
fn heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Bytes(bytes) => bytes.len(),
        _ => 0,
    }
}

/// Hash aggregation. Consumes its whole input on the first pull, keeping one entry per group
/// (memory-accounted, capped at `max_materialized_rows` groups), then emits one row per group
/// in first-seen order.
pub struct AggregateOperator {
    /// Upstream operator.
    input: Box<dyn Operator>,
    /// Grouping slots.
    group_by: Vec<SlotId>,
    /// Aggregates computed per group.
    aggregates: Vec<AggregateSpec>,
    /// Slot width of the output rows.
    width: usize,
    /// Result rows and the memory of the groups, once computed.
    output: Option<(OutputBuffer, MemoryReservation)>,
    /// Rows read from the input.
    rows_in: u64,
    /// Groups formed.
    groups: u64,
}

impl AggregateOperator {
    /// Groups `input` by `group_by` and computes `aggregates`.
    pub fn new(
        input: Box<dyn Operator>,
        group_by: Vec<SlotId>,
        aggregates: Vec<AggregateSpec>,
        width: usize,
    ) -> Self {
        Self {
            input,
            group_by,
            aggregates,
            width,
            output: None,
            rows_in: 0,
            groups: 0,
        }
    }

    /// Consumes the input and builds the result rows.
    fn compute(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<(Vec<ExecRow>, MemoryReservation), ExecutionError> {
        let mut reservation = context.memory.reservation("aggregate");
        let max_groups = context.limits.max_materialized_rows;
        let mut index: HashMap<Vec<KeyPart>, usize> = HashMap::new();
        let mut groups: Vec<(Vec<KeyPart>, Vec<Accumulator>)> = Vec::new();

        // A global aggregate has exactly one group, even for empty input.
        if self.group_by.is_empty() {
            groups.push((Vec::new(), self.fresh_accumulators()));
            index.insert(Vec::new(), 0);
        }

        while let Some(batch) = self.input.next_batch(context)? {
            context.check_running()?;
            self.rows_in += batch.len() as u64;
            for row in &batch {
                let key = group_key(row, &self.group_by);
                let position = match index.get(&key) {
                    Some(position) => *position,
                    None => {
                        if groups.len() >= max_groups {
                            return Err(ExecutionError::ResourceLimit(format!(
                                "aggregate would hold more than {max_groups} groups"
                            )));
                        }
                        let key_bytes: usize = key.iter().map(KeyPart::estimated_bytes).sum();
                        reservation.grow(
                            2 * key_bytes
                                + GROUP_OVERHEAD_BYTES
                                + self.aggregates.len() * std::mem::size_of::<Accumulator>(),
                        )?;
                        groups.push((key.clone(), self.fresh_accumulators()));
                        index.insert(key, groups.len() - 1);
                        groups.len() - 1
                    }
                };
                for (spec, accumulator) in self.aggregates.iter().zip(&mut groups[position].1) {
                    let value = spec.input.map_or(&Value::Null, |slot| row.get(slot));
                    let delta = accumulator.update(value)?;
                    if delta > 0 {
                        reservation.grow(delta as usize)?;
                    } else {
                        reservation.shrink(delta.unsigned_abs());
                    }
                }
            }
        }

        self.groups = groups.len() as u64;
        let mut rows = Vec::with_capacity(groups.len());
        for (key, accumulators) in &groups {
            let mut row = ExecRow::nulls(self.width);
            for (slot, part) in self.group_by.iter().zip(key) {
                row.set(*slot, part.to_value());
            }
            for (spec, accumulator) in self.aggregates.iter().zip(accumulators) {
                row.set(spec.output, accumulator.finish()?);
            }
            rows.push(row);
        }
        Ok((rows, reservation))
    }

    /// One fresh accumulator per aggregate.
    fn fresh_accumulators(&self) -> Vec<Accumulator> {
        self.aggregates.iter().map(Accumulator::new).collect()
    }
}

impl Operator for AggregateOperator {
    /// Aggregates everything on the first call, then emits the groups in batches.
    fn next_batch(
        &mut self,
        context: &ExecutionContext,
    ) -> Result<Option<RowBatch>, ExecutionError> {
        if self.output.is_none() {
            let (rows, reservation) = self.compute(context)?;
            self.output = Some((OutputBuffer::new(rows), reservation));
        }
        let (buffer, _) = self.output.as_mut().expect("computed above");
        Ok(buffer.next_batch(context.batch_size))
    }

    /// `aggregate`.
    fn name(&self) -> &'static str {
        "aggregate"
    }

    /// The input.
    fn children(&self) -> Vec<&dyn Operator> {
        vec![self.input.as_ref()]
    }

    /// Rows read, groups and memory.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        let peak = self
            .output
            .as_ref()
            .map_or(0, |(_, reservation)| reservation.peak() as u64);
        vec![
            ("rows_in", self.rows_in),
            ("groups", self.groups),
            ("peak_memory_bytes", peak),
        ]
    }
}
