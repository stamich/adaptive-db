//! Statistics of a secondary index, accumulated over its entries in index order.
//!
//! The engine walks the entries visible at the analyzed snapshot and feeds each one's encoded
//! key prefixes and row key here; this module does not know the index format, only bytes.
use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
};

use crate::{collector::EXACT_DISTINCT_LIMIT, hll::HyperLogLog, model::IndexStatistics};

/// Two rows count as neighbours when their storage keys differ by at most this much (about the
/// rows of one primary-index leaf).
pub const LOCALITY_SPAN: u128 = 64;

/// Distinct counter: exact up to [`EXACT_DISTINCT_LIMIT`] hashes, HyperLogLog above.
#[derive(Debug, Clone, Default)]
struct Distinct {
    /// Hashes seen while exact.
    exact: HashSet<u64>,
    /// Sketch once the exact set overflowed.
    sketch: Option<HyperLogLog>,
}

impl Distinct {
    /// Adds one hashed value.
    fn add(&mut self, hash: u64) {
        match &mut self.sketch {
            Some(sketch) => sketch.add(hash),
            None => {
                self.exact.insert(hash);
                if self.exact.len() > EXACT_DISTINCT_LIMIT {
                    let mut sketch = HyperLogLog::default();
                    for seen in self.exact.drain() {
                        sketch.add(seen);
                    }
                    self.sketch = Some(sketch);
                }
            }
        }
    }

    /// Distinct values counted (estimated above the exact limit).
    fn count(&self) -> u64 {
        match &self.sketch {
            Some(sketch) => sketch.estimate().round() as u64,
            None => self.exact.len() as u64,
        }
    }
}

/// Accumulates [`IndexStatistics`] over the entries of one index in index order.
#[derive(Debug, Clone, Default)]
pub struct IndexCollector {
    /// One distinct counter per key prefix length.
    prefixes: Vec<Distinct>,
    /// Row key of the previous entry.
    previous_row: Option<u128>,
    /// Consecutive entry pairs whose rows are neighbours (see [`LOCALITY_SPAN`]).
    neighbours: u64,
    /// Consecutive entry pairs.
    pairs: u64,
    /// Entries added.
    entries: u64,
    /// Sum of the encoded key sizes.
    key_bytes: u64,
}

impl IndexCollector {
    /// Adds one visible entry: `prefixes[i]` is the encoding of its first `i + 1` key columns
    /// (so the last one is the whole key) and `row_key` its row's storage key.
    pub fn add(&mut self, prefixes: &[&[u8]], row_key: u128) {
        if self.prefixes.len() < prefixes.len() {
            self.prefixes.resize_with(prefixes.len(), Distinct::default);
        }
        for (counter, prefix) in self.prefixes.iter_mut().zip(prefixes) {
            let mut hasher = DefaultHasher::new();
            prefix.hash(&mut hasher);
            counter.add(hasher.finish());
        }
        if let Some(previous) = self.previous_row {
            self.pairs += 1;
            if row_key.abs_diff(previous) <= LOCALITY_SPAN {
                self.neighbours += 1;
            }
        }
        self.previous_row = Some(row_key);
        self.entries += 1;
        self.key_bytes += prefixes.last().map_or(0, |key| key.len() as u64);
    }

    /// The statistics of index `index_id`, given the shape of its tree.
    pub fn finish(
        self,
        index_id: u64,
        stored_entries: u64,
        height: u32,
        leaf_pages: u64,
    ) -> IndexStatistics {
        let locality = if self.pairs == 0 {
            1.0
        } else {
            self.neighbours as f64 / self.pairs as f64
        };
        let mut distinct: Vec<u64> = self
            .prefixes
            .iter()
            .map(|counter| counter.count().min(self.entries))
            .collect();
        // A longer prefix never has fewer distinct values; estimates may say otherwise.
        for at in 1..distinct.len() {
            distinct[at] = distinct[at].max(distinct[at - 1]);
        }
        IndexStatistics {
            index_id,
            entries: self.entries,
            stored_entries: stored_entries.max(self.entries),
            height,
            leaf_pages,
            avg_key_bytes: if self.entries == 0 {
                0.0
            } else {
                self.key_bytes as f64 / self.entries as f64
            },
            distinct_prefixes: distinct,
            pk_locality: locality,
        }
    }
}

/// Unit tests of the index collector.
#[cfg(test)]
mod tests {
    use super::*;

    /// Prefix counts, correlation and widths of a small index.
    #[test]
    fn counts_prefixes_and_order() {
        let mut collector = IndexCollector::default();
        // (a, b) keys in index order; one jump between far-apart rows.
        let rows = [
            (b"a", b"a1", 1u128),
            (b"a", b"a2", 2),
            (b"b", b"b1", 500),
            (b"b", b"b2", 499),
        ];
        for (first, whole, row) in rows {
            collector.add(&[first.as_slice(), whole.as_slice()], row);
        }
        let statistics = collector.finish(7, 6, 1, 1);
        assert_eq!(statistics.entries, 4);
        assert_eq!(statistics.stored_entries, 6);
        assert_eq!(statistics.distinct_prefixes, vec![2, 4]);
        assert!((statistics.pk_locality - (2.0 / 3.0)).abs() < 1e-9);
        assert_eq!(statistics.avg_key_bytes, 2.0);
    }

    /// Large cardinalities switch to the sketch and stay within its error.
    #[test]
    fn large_cardinalities_are_estimated() {
        let mut collector = IndexCollector::default();
        for i in 0..50_000u64 {
            let key = i.to_be_bytes();
            collector.add(&[&key[..7], &key], u128::from(i));
        }
        let statistics = collector.finish(1, 50_000, 3, 100);
        let estimate = statistics.distinct_prefixes[1] as f64;
        assert!((estimate - 50_000.0).abs() / 50_000.0 < 0.03, "{estimate}");
        assert_eq!(statistics.pk_locality, 1.0);
        assert!(statistics.distinct_prefixes[0] <= statistics.distinct_prefixes[1]);
    }
}
