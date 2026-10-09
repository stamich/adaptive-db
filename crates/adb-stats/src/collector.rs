//! One `ANALYZE` run: a single snapshot scan of one entity.
//!
//! Exact figures (row count, NULLs, min/max, widths) are accumulated over every row. Distinct
//! values are counted exactly up to [`EXACT_DISTINCT_LIMIT`] per column and estimated with a
//! [`HyperLogLog`] sketch above it. Histograms and most common values are built from a
//! reservoir sample (Algorithm R with a seeded generator, so a run is reproducible); when the
//! sample holds every row they are exact.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, Instant},
};

use adb_core::{CommitTs, FieldId, KeyRange, Row, Value};
use adb_execution::{key::KeyPart, DataSource};

use crate::{
    hll::HyperLogLog,
    model::{
        ColumnStatistics, HistogramBucket, MostCommonValue, TableStatistics, MAX_COLUMNS,
        STATISTICS_FORMAT_VERSION,
    },
    values::{self, MAX_STORED_VALUE_BYTES},
    AnalyzeOptions, StatsError,
};

/// Distinct values counted exactly per column before switching to a sketch.
pub const EXACT_DISTINCT_LIMIT: usize = 10_000;
/// Rows requested per scan page.
const PAGE_ROWS: usize = 1024;
/// Bytes charged per field when computing the average row size (an estimate of the encoded size).
const FIELD_OVERHEAD_BYTES: usize = 8;
/// Bytes a sampled row holds besides its fields: the `Vec` header plus allocator overhead.
const SAMPLED_ROW_OVERHEAD_BYTES: usize = std::mem::size_of::<SampledRow>() + 16;
/// Bytes a sampled field holds besides heap payload: the `(FieldId, Value)` entry.
const SAMPLED_FIELD_BYTES: usize = std::mem::size_of::<(FieldId, Value)>();
/// Bytes charged per hash in an exact distinct set.
const DISTINCT_ENTRY_BYTES: usize = 16;
/// A common value must be at least this many times more frequent than the average value.
const COMMON_VALUE_FACTOR: f64 = 1.25;

/// One sampled row: its analyzed non-NULL fields, sorted by field id. A sorted vector holds a
/// row in one allocation, so the working-set accounting below is close to the real footprint.
type SampledRow = Vec<(FieldId, Value)>;

/// Analyzes `entity_id` as of `snapshot`.
///
/// `modifications_at_analyze` of the result is 0; the engine fills it in.
pub fn analyze(
    source: &dyn DataSource,
    entity_id: u64,
    snapshot: CommitTs,
    options: &AnalyzeOptions,
) -> Result<TableStatistics, StatsError> {
    options.validate()?;
    let mut run = Run::new(entity_id, options);
    let started = Instant::now();
    let deadline = options.deadline_ms.map(Duration::from_millis);
    let range = KeyRange::entity(entity_id);
    let mut after = None;
    loop {
        let page = source
            .scan_page(&range, after, PAGE_ROWS, snapshot)
            .map_err(|error| StatsError::Source(error.to_string()))?;
        let Some((last, _)) = page.last() else {
            break;
        };
        after = Some(*last);
        for (_, row) in &page {
            run.add(row)?;
        }
        run.check_working_set()?;
        if deadline.is_some_and(|limit| started.elapsed() >= limit) {
            return Err(StatsError::Limit(format!(
                "ANALYZE of entity {entity_id} exceeded its {} ms deadline",
                options.deadline_ms.unwrap_or_default()
            )));
        }
    }
    Ok(run.finish(snapshot))
}

/// Accumulators of one run.
struct Run<'a> {
    /// Entity analyzed.
    entity_id: u64,
    /// Options of the run.
    options: &'a AnalyzeOptions,
    /// Rows seen.
    rows: u64,
    /// Sum of the rows' encoded sizes.
    row_bytes: u64,
    /// Per-field accumulators.
    columns: BTreeMap<FieldId, ColumnAccumulator>,
    /// Reservoir sample of rows (restricted to the analyzed fields).
    sample: Vec<SampledRow>,
    /// Bytes held by `sample`.
    sample_bytes: usize,
    /// Deterministic generator for reservoir replacement.
    random: SplitMix64,
}

/// Exact figures and the distinct counter of one field.
#[derive(Default)]
struct ColumnAccumulator {
    /// Non-NULL values seen.
    non_null: u64,
    /// Non-NULL values that take part in ordering (all but non-finite floats).
    ordered: u64,
    /// Sum of the non-NULL values' widths.
    width_sum: u64,
    /// Smallest value seen.
    min: Option<Value>,
    /// Largest value seen.
    max: Option<Value>,
    /// Type tag of the first non-NULL value.
    type_tag: Option<u8>,
    /// Whether values of more than one type occurred.
    mixed_types: bool,
    /// Distinct values.
    distinct: Distinct,
}

/// Exact set of value hashes, replaced by a sketch once it grows too large.
enum Distinct {
    /// Hashes of every distinct value seen so far.
    Exact(HashSet<u64>),
    /// Sketch of the distinct values.
    Sketch(HyperLogLog),
}

impl Default for Distinct {
    /// An empty exact set.
    fn default() -> Self {
        Self::Exact(HashSet::new())
    }
}

impl Distinct {
    /// Records one value hash.
    fn add(&mut self, hash: u64) {
        match self {
            Self::Exact(set) => {
                set.insert(hash);
                if set.len() > EXACT_DISTINCT_LIMIT {
                    let mut sketch = HyperLogLog::default();
                    set.iter().for_each(|hash| sketch.add(*hash));
                    *self = Self::Sketch(sketch);
                }
            }
            Self::Sketch(sketch) => sketch.add(hash),
        }
    }

    /// Bytes held.
    fn bytes(&self) -> usize {
        match self {
            Self::Exact(set) => set.len() * DISTINCT_ENTRY_BYTES,
            Self::Sketch(_) => HyperLogLog::BYTES,
        }
    }

    /// The distinct count and whether it is exact, capped at `non_null`.
    fn count(&self, non_null: u64) -> (u64, bool) {
        match self {
            Self::Exact(set) => (set.len() as u64, true),
            Self::Sketch(sketch) => {
                let estimate = sketch.estimate().round() as u64;
                (estimate.clamp(u64::from(non_null > 0), non_null), false)
            }
        }
    }
}

impl<'a> Run<'a> {
    /// An empty run.
    fn new(entity_id: u64, options: &'a AnalyzeOptions) -> Self {
        let mut columns = BTreeMap::new();
        for field in options.fields.iter().flatten() {
            columns.insert(*field, ColumnAccumulator::default());
        }
        Self {
            entity_id,
            options,
            rows: 0,
            row_bytes: 0,
            columns,
            sample: Vec::new(),
            sample_bytes: 0,
            random: SplitMix64(entity_id ^ 0x9E37_79B9_7F4A_7C15),
        }
    }

    /// Folds one row into every accumulator and the reservoir.
    fn add(&mut self, row: &Row) -> Result<(), StatsError> {
        self.rows += 1;
        if self.rows > self.options.max_rows {
            return Err(StatsError::Limit(format!(
                "entity {} has more than {} rows",
                self.entity_id, self.options.max_rows
            )));
        }
        let restricted = self.options.fields.is_some();
        let mut kept = SampledRow::new();
        for (field, value) in &row.fields {
            self.row_bytes += (values::width(value) + FIELD_OVERHEAD_BYTES) as u64;
            if restricted && !self.columns.contains_key(field) {
                continue;
            }
            if !self.columns.contains_key(field) && self.columns.len() >= MAX_COLUMNS {
                return Err(StatsError::Limit(format!(
                    "entity {} has more than {MAX_COLUMNS} fields",
                    self.entity_id
                )));
            }
            let column = self.columns.entry(*field).or_default();
            if matches!(value, Value::Null) {
                continue;
            }
            column.add(value);
            kept.push((*field, value.clone()));
        }
        // `row.fields` iterates in field order, so `kept` is sorted.
        kept.shrink_to_fit();
        let bytes = sampled_row_bytes(&kept);
        self.sample_row(kept, bytes);
        Ok(())
    }

    /// Algorithm R: the first `sample_rows` rows, then each row replaces a random one with
    /// probability `sample_rows / rows`.
    fn sample_row(&mut self, row: SampledRow, bytes: usize) {
        let capacity = self.options.sample_rows;
        if self.sample.len() < capacity {
            self.sample.push(row);
            self.sample_bytes += bytes;
            return;
        }
        let slot = (self.random.next() % self.rows) as usize;
        if slot < capacity {
            let old = std::mem::replace(&mut self.sample[slot], row);
            self.sample_bytes = self.sample_bytes + bytes - sampled_row_bytes(&old);
        }
    }

    /// Fails once the sample and distinct sets hold more than the working-set budget.
    fn check_working_set(&self) -> Result<(), StatsError> {
        let distinct: usize = self.columns.values().map(|c| c.distinct.bytes()).sum();
        let held = self.sample_bytes + distinct;
        if held > self.options.max_working_set_bytes {
            return Err(StatsError::Limit(format!(
                "ANALYZE of entity {} needs more than {} bytes of working set",
                self.entity_id, self.options.max_working_set_bytes
            )));
        }
        Ok(())
    }

    /// Builds the statistics document.
    fn finish(self, snapshot: CommitTs) -> TableStatistics {
        let exact = self.sample.len() as u64 == self.rows;
        let columns = self
            .columns
            .iter()
            .map(|(field, column)| column.finish(*field, self.rows, &self.sample, self.options))
            .collect();
        TableStatistics {
            format_version: STATISTICS_FORMAT_VERSION,
            entity_id: self.entity_id,
            row_count: self.rows,
            avg_row_bytes: if self.rows == 0 {
                0.0
            } else {
                self.row_bytes as f64 / self.rows as f64
            },
            analyzed_at_ts: snapshot.0,
            modifications_at_analyze: 0,
            sampled_rows: self.sample.len() as u64,
            exact,
            columns,
        }
    }
}

impl ColumnAccumulator {
    /// Folds one non-NULL value in.
    fn add(&mut self, value: &Value) {
        self.non_null += 1;
        self.width_sum += values::width(value) as u64;
        let tag = values::type_tag(value);
        match self.type_tag {
            None => self.type_tag = Some(tag),
            Some(existing) if existing != tag => self.mixed_types = true,
            Some(_) => {}
        }
        self.distinct.add(values::hash(value));
        if !orderable(value) {
            return;
        }
        self.ordered += 1;
        if self.mixed_types {
            return;
        }
        if self
            .min
            .as_ref()
            .is_none_or(|min| values::compare(value, min) == Some(std::cmp::Ordering::Less))
        {
            self.min = Some(value.clone());
        }
        if self
            .max
            .as_ref()
            .is_none_or(|max| values::compare(value, max) == Some(std::cmp::Ordering::Greater))
        {
            self.max = Some(value.clone());
        }
    }

    /// The column's statistics; histogram and common values come from the sample.
    fn finish(
        &self,
        field: FieldId,
        rows: u64,
        sample: &[SampledRow],
        options: &AnalyzeOptions,
    ) -> ColumnStatistics {
        let (distinct_count, distinct_exact) = self.distinct.count(self.non_null);
        let mut statistics = ColumnStatistics {
            field_id: field,
            null_count: rows - self.non_null,
            distinct_count,
            distinct_exact,
            min: None,
            max: None,
            avg_width_bytes: if self.non_null == 0 {
                0.0
            } else {
                self.width_sum as f64 / self.non_null as f64
            },
            histogram: Vec::new(),
            most_common: Vec::new(),
        };
        if self.mixed_types {
            // No order and no meaningful frequencies across types.
            return statistics;
        }
        statistics.min = self.min.as_ref().map(values::stored);
        statistics.max = self.max.as_ref().map(values::stored);

        let mut sampled: Vec<&Value> = sample
            .iter()
            .filter_map(|row| {
                row.binary_search_by_key(&field, |(id, _)| *id)
                    .ok()
                    .map(|index| &row[index].1)
            })
            .filter(|value| orderable(value))
            .collect();
        if sampled.is_empty() {
            return statistics;
        }
        // Histogram and common values describe the orderable values only.
        let scale = self.ordered as f64 / sampled.len() as f64;
        statistics.most_common = most_common(&sampled, scale, options.most_common_values);
        sampled.sort_by(|a, b| values::compare(a, b).unwrap_or(std::cmp::Ordering::Equal));
        statistics.histogram = histogram(&sampled, scale, options.histogram_buckets);
        statistics
    }
}

/// Values markedly more frequent than average in the sample, most frequent first.
fn most_common(sampled: &[&Value], scale: f64, limit: usize) -> Vec<MostCommonValue> {
    let mut counts: HashMap<KeyPart, (u64, &Value)> = HashMap::new();
    for value in sampled {
        counts.entry(KeyPart::of(value)).or_insert((0, value)).0 += 1;
    }
    let average = sampled.len() as f64 / counts.len() as f64;
    let mut candidates: Vec<(u64, &Value)> = counts
        .into_values()
        .filter(|(count, value)| {
            *count >= 2
                && *count as f64 > COMMON_VALUE_FACTOR * average
                && values::width(value) <= MAX_STORED_VALUE_BYTES
        })
        .collect();
    // Most frequent first; ties in value order, so the result is deterministic.
    candidates.sort_by(|(ca, va), (cb, vb)| {
        cb.cmp(ca)
            .then_with(|| values::compare(va, vb).unwrap_or(std::cmp::Ordering::Equal))
    });
    candidates
        .into_iter()
        .take(limit)
        .map(|(count, value)| MostCommonValue {
            value: value.clone(),
            rows: (count as f64 * scale).round() as u64,
        })
        .collect()
}

/// Equi-depth histogram over sorted sample values.
///
/// Bucket row counts are scaled with cumulative rounding, so they sum exactly to
/// `round(sorted.len() * scale)` (the column's non-NULL count).
fn histogram(sorted: &[&Value], scale: f64, buckets: usize) -> Vec<HistogramBucket> {
    let n = sorted.len();
    let buckets = buckets.min(n);
    (0..buckets)
        .map(|i| {
            let (start, end) = (i * n / buckets, (i + 1) * n / buckets);
            let slice = &sorted[start..end];
            let distinct = 1 + slice
                .windows(2)
                .filter(|pair| values::compare(pair[0], pair[1]) != Some(std::cmp::Ordering::Equal))
                .count() as u64;
            HistogramBucket {
                lower: values::stored(slice[0]),
                upper: values::stored(slice[slice.len() - 1]),
                rows: (end as f64 * scale).round() as u64 - (start as f64 * scale).round() as u64,
                distinct,
            }
        })
        .collect()
}

/// Bytes a sampled row holds: entries, heap payload of strings and byte arrays, overhead.
fn sampled_row_bytes(row: &SampledRow) -> usize {
    SAMPLED_ROW_OVERHEAD_BYTES
        + row
            .iter()
            .map(|(_, value)| {
                SAMPLED_FIELD_BYTES
                    + match value {
                        Value::String(_) | Value::Bytes(_) => values::width(value),
                        _ => 0,
                    }
            })
            .sum::<usize>()
}

/// Whether a value can take part in min/max, histograms and common values: every value except
/// non-finite floats (NaN has no order; infinities have no JSON form).
fn orderable(value: &Value) -> bool {
    !matches!(value, Value::Float64(v) if !v.is_finite())
}

/// SplitMix64: a tiny, well-mixed, seedable generator.
struct SplitMix64(u64);

impl SplitMix64 {
    /// Next pseudo-random value.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
