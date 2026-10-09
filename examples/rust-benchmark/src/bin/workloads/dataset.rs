//! The benchmark dataset: every table, its entity id, its row formula and its size at a scale
//! factor. This is the only definition of the data; the JVM benchmark opens a database
//! prepared from it (`workloads --prepare`) and creates the matching catalog, in the same
//! order, so entity and field ids line up.

use std::time::Instant;

use adb_core::{Row, RowId, Value};
use adb_engine::{AnalyzeOptions, Database};

use crate::Result;

/// One table of the dataset.
pub struct Table {
    /// SQL name (the JVM catalog uses the same name).
    pub name: &'static str,
    /// Entity id: the position of the table's `CREATE TABLE` in the JVM catalog, from 1.
    pub entity: u64,
    /// Rows at scale factor 1.
    pub base_rows: u64,
    /// Whether the row count grows with the scale factor.
    pub scaled: bool,
}

impl Table {
    /// Rows at scale factor `scale`.
    pub fn rows(&self, scale: u64) -> u64 {
        if self.scaled {
            self.base_rows * scale
        } else {
            self.base_rows
        }
    }
}

/// Every table, in catalog order (entity ids 1..=10).
pub const TABLES: [Table; 10] = [
    table("bench_customer", 1, 100, true),
    table("bench_orders", 2, 1_000, true),
    table("jo_c", 3, 20, false),
    table("jo_b", 4, 2_000, true),
    table("jo_a", 5, 20_000, true),
    table("st_region", 6, 100, false),
    table("st_dim", 7, 1_000, true),
    table("st_fact", 8, 100_000, true),
    table("sk_events", 9, 100_000, true),
    table("sk_stale", 10, 100_000, true),
];

/// `const` constructor of [`Table`].
const fn table(name: &'static str, entity: u64, base_rows: u64, scaled: bool) -> Table {
    Table {
        name,
        entity,
        base_rows,
        scaled,
    }
}

/// The table named `name`.
pub fn by_name(name: &str) -> &'static Table {
    TABLES
        .iter()
        .find(|table| table.name == name)
        .expect("known table")
}

/// Rows of table `name` at `scale`.
pub fn rows(name: &str, scale: u64) -> u64 {
    by_name(name).rows(scale)
}

/// Distinct `kind` values of the skewed tables.
const KINDS: u64 = 50;

/// Row `id` (1-based) of `table` at `scale`. Field ids follow the column order of the JVM DDL.
pub fn row(table: &Table, id: u64, scale: u64) -> Row {
    let int = |v: u64| Value::Int64(v as i64);
    let fields: Vec<Value> = match table.name {
        // id, name, segment
        "bench_customer" => vec![
            int(id),
            Value::String(format!("customer-{id}")),
            int(id % 5),
        ],
        // id, customer_id -> bench_customer, amount
        "bench_orders" => vec![
            int(id),
            int(1 + (id * 7) % rows("bench_customer", scale)),
            int((id * 37) % 1000 + 1),
        ],
        // id, tag
        "jo_c" => vec![int(id), int(id % 10)],
        // id, c_id -> jo_c
        "jo_b" => vec![int(id), int(1 + (id * 7) % rows("jo_c", scale))],
        // id, b_id -> jo_b, v
        "jo_a" => vec![
            int(id),
            int(1 + (id * 13) % rows("jo_b", scale)),
            int(id % 1000),
        ],
        // id, tag (tag = 7 selects 1% of the regions)
        "st_region" => vec![int(id), int(id % 100)],
        // id, region_id -> st_region, name
        "st_dim" => vec![
            int(id),
            int(1 + (id * 31) % rows("st_region", scale)),
            Value::String(format!("dim-{id}")),
        ],
        // id, dim_id -> st_dim, v
        "st_fact" => vec![
            int(id),
            int(1 + (id * 17) % rows("st_dim", scale)),
            int(id % 100),
        ],
        // id, kind (Zipf over 50 values), region (= kind + 100: perfectly correlated), v
        "sk_events" => {
            let kind = zipf_kind(id);
            vec![int(id), int(kind), int(kind + 100), int(id % 10)]
        }
        // id, kind (uniform when analyzed; see `make_stale`), v
        "sk_stale" => vec![int(id), int(id % KINDS), int(id % 10)],
        other => unreachable!("unknown table {other}"),
    };
    fields
        .into_iter()
        .zip(1..)
        .fold(Row::new(), |row, (value, field)| {
            row.with_field(field, value)
        })
}

/// A Zipf-distributed kind (exponent 1.1) chosen by a hash of `id`: kind 0 holds about 26% of
/// the rows, kind 7 about 2.7%, kind 49 about 0.4%.
fn zipf_kind(id: u64) -> u64 {
    let weights: Vec<f64> = (1..=KINDS).map(|k| 1.0 / (k as f64).powf(1.1)).collect();
    let total: f64 = weights.iter().sum();
    let u = (split_mix(id) >> 11) as f64 / (1u64 << 53) as f64 * total;
    let mut cumulative = 0.0;
    for (kind, weight) in weights.iter().enumerate() {
        cumulative += weight;
        if u < cumulative {
            return kind as u64;
        }
    }
    KINDS - 1
}

/// SplitMix64 finalizer: a well-mixed hash of `x`.
fn split_mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Rows committed per load transaction.
const LOAD_BATCH: u64 = 5_000;

/// Loads every table at `scale`; returns the rows loaded and the elapsed seconds.
pub fn load(db: &Database, scale: u64) -> Result<(u64, f64)> {
    let start = Instant::now();
    let mut total = 0;
    for table in &TABLES {
        let rows = table.rows(scale);
        let mut first = 1;
        while first <= rows {
            let last = (first + LOAD_BATCH - 1).min(rows);
            let mut tx = db.begin();
            for id in first..=last {
                tx.put(RowId::compose(table.entity, id), row(table, id, scale));
            }
            db.commit(tx)?;
            first = last + 1;
        }
        total += rows;
    }
    Ok((total, start.elapsed().as_secs_f64()))
}

/// Analyzes `sk_stale`, then moves every even row to kind 0 (50% of the rows): its statistics
/// still describe a uniform column, so estimates of `kind = 0` are off by about 25× and the
/// planner reports them as stale.
pub fn make_stale(db: &Database, scale: u64) -> Result<()> {
    let table = by_name("sk_stale");
    db.analyze(table.entity, &AnalyzeOptions::default())?;
    let rows = table.rows(scale);
    let mut first = 2;
    while first <= rows {
        let mut tx = db.begin();
        let mut id = first;
        while id <= rows && id < first + 2 * LOAD_BATCH {
            let changed = row(table, id, scale).with_field(2, Value::Int64(0));
            tx.put(RowId::compose(table.entity, id), changed);
            id += 2;
        }
        db.commit(tx)?;
        first = id;
    }
    Ok(())
}

/// Unit tests of the dataset formulas.
#[cfg(test)]
mod tests {
    use super::*;

    /// Entity ids are 1..=10 in table order; scale multiplies only scaled tables.
    #[test]
    fn tables_are_ordered_and_scaled() {
        for (index, table) in TABLES.iter().enumerate() {
            assert_eq!(table.entity, index as u64 + 1);
        }
        assert_eq!(rows("bench_orders", 10), 10_000);
        assert_eq!(rows("jo_c", 10), 20);
    }

    /// The Zipf kinds are skewed as documented.
    #[test]
    fn zipf_is_skewed() {
        let n = 100_000;
        let zero = (1..=n).filter(|id| zipf_kind(*id) == 0).count() as f64 / n as f64;
        assert!((0.24..0.28).contains(&zero), "{zero}");
        assert!((1..=n).all(|id| zipf_kind(id) < KINDS));
    }
}
