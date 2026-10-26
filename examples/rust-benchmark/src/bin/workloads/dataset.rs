//! The benchmark dataset: every table, its entity id, its row formula and its size at a scale
//! factor. This is the only definition of the data; the JVM benchmark opens a database
//! prepared from it (`workloads --prepare`) and creates the matching catalog, in the same
//! order, so entity and field ids line up.

use std::time::Instant;

use adb_core::{Direction, Row, RowId, Value};
use adb_engine::{AnalyzeOptions, Database, IndexColumn, IndexSpec};

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

/// Every table, in catalog order (entity ids 1..=11).
pub const TABLES: [Table; 11] = [
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
    table("ix_account", 11, 100_000, true),
];

/// Multiplier of the `ix_account.balance` permutation.
const BALANCE_MULTIPLIER: u64 = 48_271;
/// Modulus of the `ix_account.balance` permutation (prime, so balances are unique for ids
/// below it, and spread over `0..BALANCE_MODULUS` like random values).
pub const BALANCE_MODULUS: u64 = 1_000_000_007;
/// Distinct owners of `ix_account` (`owner = 'owner-42'` selects 1 / 10,000 of the rows).
pub const OWNERS: u64 = 10_000;

/// One secondary index of the 2.3.0 workloads, as `CREATE INDEX` declares it.
pub struct Index {
    /// Index name (the JVM creates the same index through SQL).
    pub name: &'static str,
    /// Indexed table.
    pub table: &'static str,
    /// Key column field ids, ascending.
    pub columns: &'static [u32],
    /// Covered (`INCLUDE`) field ids.
    pub include: &'static [u32],
    /// The equivalent SQL statement.
    pub sql: &'static str,
}

impl Index {
    /// The engine's specification of the index.
    pub fn spec(&self) -> IndexSpec {
        IndexSpec {
            name: self.name.into(),
            entity_id: by_name(self.table).entity,
            columns: self
                .columns
                .iter()
                .map(|field| IndexColumn {
                    field_id: *field,
                    direction: Direction::Asc,
                })
                .collect(),
            include: self.include.to_vec(),
        }
    }
}

/// The indexes of `ix_account` (fields: 1 id, 2 region, 3 balance, 4 owner, 5 note).
pub const INDEXES: [Index; 2] = [
    Index {
        name: "idx_ix_owner",
        table: "ix_account",
        columns: &[4],
        include: &[3],
        sql: "CREATE INDEX idx_ix_owner ON ix_account (owner) INCLUDE (balance)",
    },
    Index {
        name: "idx_ix_balance",
        table: "ix_account",
        columns: &[3],
        include: &[],
        sql: "CREATE INDEX idx_ix_balance ON ix_account (balance)",
    },
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
        // id, region (100 values), balance (unique, spread like random values), owner
        // (10,000 values), note (a payload column no index covers)
        "ix_account" => vec![
            int(id),
            int(id % 100),
            int(balance(id)),
            Value::String(format!("owner-{}", id % OWNERS)),
            Value::String(format!("note-{id:08}")),
        ],
        other => unreachable!("unknown table {other}"),
    };
    fields
        .into_iter()
        .zip(1..)
        .fold(Row::new(), |row, (value, field)| {
            row.with_field(field, value)
        })
}

/// `ix_account.balance` of row `id`: `id * 48,271 mod 1,000,000,007`.
pub fn balance(id: u64) -> u64 {
    (id * BALANCE_MULTIPLIER) % BALANCE_MODULUS
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
                tx.put(
                    RowId::from_primary_key(table.entity, id as i64),
                    row(table, id, scale),
                );
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
            tx.put(RowId::from_primary_key(table.entity, id as i64), changed);
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

    /// Entity ids are 1..=11 in table order; scale multiplies only scaled tables.
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

    /// Balances are unique (ORDER BY balance has no ties) and spread over the whole range.
    #[test]
    fn balances_are_unique_and_spread() {
        let n = 100_000;
        let mut balances: Vec<u64> = (1..=n).map(balance).collect();
        let below_tenth = balances
            .iter()
            .filter(|b| **b < BALANCE_MODULUS / 10)
            .count();
        assert!((9_000..11_000).contains(&below_tenth), "{below_tenth}");
        balances.sort_unstable();
        balances.dedup();
        assert_eq!(balances.len() as u64, n);
    }
}
