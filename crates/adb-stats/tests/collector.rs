//! `ANALYZE` collector: exact figures, sketches, sampling, limits and determinism.
use adb_core::{CommitTs, KeyRange, Row, RowId, Value};
use adb_execution::{DataSource, ExecutionError};
use adb_stats::{analyze, AnalyzeOptions, StatsError, TableStatistics};

/// Entity analyzed by every test.
const ENTITY: u64 = 3;

/// A synthetic entity of `rows` rows whose primary keys are `0..rows`; row `i` is `generate(i)`.
struct Synthetic<F: Fn(u64) -> Row + Send + Sync> {
    /// Number of rows.
    rows: u64,
    /// Row generator.
    generate: F,
}

impl<F: Fn(u64) -> Row + Send + Sync> DataSource for Synthetic<F> {
    /// Always the first commit.
    fn latest_committed_ts(&self) -> CommitTs {
        CommitTs(1)
    }

    /// Generates the row when its key belongs to the entity.
    fn point_lookup(&self, row_id: RowId, _: CommitTs) -> Result<Option<Row>, ExecutionError> {
        let pk = row_id.0 as u64;
        Ok(
            (KeyRange::entity(ENTITY).contains(row_id) && pk < self.rows)
                .then(|| (self.generate)(pk)),
        )
    }

    /// Generates one page of rows in key order.
    fn scan_page(
        &self,
        range: &KeyRange,
        after: Option<RowId>,
        limit: usize,
        _: CommitTs,
    ) -> Result<Vec<(RowId, Row)>, ExecutionError> {
        if *range != KeyRange::entity(ENTITY) {
            return Ok(Vec::new());
        }
        let first = after.map_or(0, |after| after.0 as u64 + 1);
        Ok((first..self.rows.min(first + limit as u64))
            .map(|pk| (RowId::compose(ENTITY, pk), (self.generate)(pk)))
            .collect())
    }
}

/// Runs `ANALYZE` over a synthetic entity.
fn run(
    rows: u64,
    generate: impl Fn(u64) -> Row + Send + Sync,
    options: &AnalyzeOptions,
) -> Result<TableStatistics, StatsError> {
    analyze(&Synthetic { rows, generate }, ENTITY, CommitTs(1), options)
}

/// A small table yields exact counts, NULLs, bounds, NDV and a histogram covering every value.
#[test]
fn small_table_is_exact() {
    let statistics = run(
        500,
        |pk| {
            Row::new()
                .with_field(1, Value::Int64(pk as i64))
                .with_field(
                    2,
                    if pk % 5 == 0 {
                        Value::Null
                    } else {
                        Value::String(format!("name-{}", pk % 40))
                    },
                )
        },
        &AnalyzeOptions::default(),
    )
    .unwrap();
    statistics.validate().unwrap();
    assert_eq!(statistics.row_count, 500);
    assert!(statistics.exact);
    assert_eq!(statistics.sampled_rows, 500);
    assert_eq!(statistics.entity_id, ENTITY);
    assert_eq!(statistics.analyzed_at_ts, 1);

    let id = statistics.column(1).unwrap();
    assert_eq!(id.null_count, 0);
    assert_eq!(id.distinct_count, 500);
    assert!(id.distinct_exact);
    assert_eq!(id.min, Some(Value::Int64(0)));
    assert_eq!(id.max, Some(Value::Int64(499)));
    assert_eq!(id.histogram.iter().map(|b| b.rows).sum::<u64>(), 500);
    assert!(id.most_common.is_empty(), "unique values are never common");

    let name = statistics.column(2).unwrap();
    assert_eq!(name.null_count, 100);
    // pk % 40 for pk not divisible by 5 hits 32 residues (those not divisible by 5).
    assert_eq!(name.distinct_count, 32);
    assert_eq!(name.histogram.iter().map(|b| b.rows).sum::<u64>(), 400);
}

/// A skewed column exposes its dominant value as the first most common value.
#[test]
fn skewed_column_reports_most_common_values() {
    let countries = |pk: u64| match pk % 4 {
        0 | 1 => "PL",
        2 => "DE",
        _ => "US",
    };
    let rows = 100_000;
    let statistics = run(
        rows,
        |pk| Row::new().with_field(1, Value::String(countries(pk).into())),
        &AnalyzeOptions::default(),
    )
    .unwrap();
    assert!(!statistics.exact, "100k rows exceed the default sample");
    let column = statistics.column(1).unwrap();
    assert_eq!(column.distinct_count, 3);
    let first = &column.most_common[0];
    assert_eq!(first.value, Value::String("PL".into()));
    let error = (first.rows as f64 - rows as f64 / 2.0).abs() / (rows as f64 / 2.0);
    assert!(error < 0.05, "PL estimate {} is off by {error}", first.rows);
}

/// Above the exact limit, the distinct count comes from the sketch and stays within 3%.
#[test]
fn large_table_uses_sketch_within_error_bound() {
    let rows = 200_000;
    let statistics = run(
        rows,
        |pk| Row::new().with_field(1, Value::Int64((pk * 7919) as i64)),
        &AnalyzeOptions::default(),
    )
    .unwrap();
    let column = statistics.column(1).unwrap();
    assert!(!column.distinct_exact);
    let error = (column.distinct_count as f64 - rows as f64).abs() / rows as f64;
    assert!(
        error < 0.03,
        "NDV {} is off by {error}",
        column.distinct_count
    );
    assert_eq!(column.histogram.iter().map(|b| b.rows).sum::<u64>(), rows);
}

/// Restricting the analyzed fields skips the others.
#[test]
fn field_selection_is_respected() {
    let options = AnalyzeOptions {
        fields: Some(vec![2]),
        ..AnalyzeOptions::default()
    };
    let statistics = run(
        10,
        |pk| {
            Row::new()
                .with_field(1, Value::Int64(pk as i64))
                .with_field(2, Value::Bool(pk % 2 == 0))
        },
        &options,
    )
    .unwrap();
    assert!(statistics.column(1).is_none());
    assert_eq!(statistics.column(2).unwrap().distinct_count, 2);
}

/// A column whose values change type gets counts but no order-based statistics.
#[test]
fn mixed_types_have_no_bounds_or_histogram() {
    let statistics = run(
        100,
        |pk| {
            Row::new().with_field(
                1,
                if pk % 2 == 0 {
                    Value::Int64(pk as i64)
                } else {
                    Value::String(pk.to_string())
                },
            )
        },
        &AnalyzeOptions::default(),
    )
    .unwrap();
    let column = statistics.column(1).unwrap();
    assert_eq!(column.distinct_count, 100);
    assert!(column.min.is_none() && column.max.is_none());
    assert!(column.histogram.is_empty() && column.most_common.is_empty());
}

/// An empty entity produces an empty, valid document.
#[test]
fn empty_entity_is_valid() {
    let statistics = run(0, |_| Row::new(), &AnalyzeOptions::default()).unwrap();
    statistics.validate().unwrap();
    assert_eq!(statistics.row_count, 0);
    assert!(statistics.columns.is_empty());
    assert_eq!(statistics.avg_row_bytes, 0.0);
}

/// Every bound is enforced as a `Limit` error.
#[test]
fn limits_are_enforced() {
    let int_row = |pk: u64| Row::new().with_field(1, Value::Int64(pk as i64));

    let rows_limited = AnalyzeOptions {
        max_rows: 100,
        ..AnalyzeOptions::default()
    };
    assert!(matches!(
        run(101, int_row, &rows_limited),
        Err(StatsError::Limit(_))
    ));
    assert!(run(100, int_row, &rows_limited).is_ok());

    let memory_limited = AnalyzeOptions {
        max_working_set_bytes: 4096,
        ..AnalyzeOptions::default()
    };
    assert!(matches!(
        run(10_000, int_row, &memory_limited),
        Err(StatsError::Limit(_))
    ));

    let deadline = AnalyzeOptions {
        deadline_ms: Some(0),
        ..AnalyzeOptions::default()
    };
    assert!(matches!(
        run(5_000, int_row, &deadline),
        Err(StatsError::Limit(_))
    ));

    let wide = |_: u64| {
        (0..=adb_stats::model::MAX_COLUMNS as u32).fold(Row::new(), |row, field| {
            row.with_field(field, Value::Bool(true))
        })
    };
    assert!(matches!(
        run(1, wide, &AnalyzeOptions::default()),
        Err(StatsError::Limit(_))
    ));
}

/// Two runs over the same data produce identical documents (the sample is seeded).
#[test]
fn runs_are_deterministic() {
    let generate = |pk: u64| Row::new().with_field(1, Value::Int64((pk % 997) as i64));
    let first = run(80_000, generate, &AnalyzeOptions::default()).unwrap();
    let second = run(80_000, generate, &AnalyzeOptions::default()).unwrap();
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
}
