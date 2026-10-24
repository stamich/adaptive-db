//! The index projection against a reference model of every row version.
use std::cmp::Ordering;

use adb_core::{CommitTs, Direction, IndexId, Lsn, Row, RowId, Value};
use adb_index::{
    IndexBounds, IndexColumn, IndexDefinition, IndexEntry, IndexError, IndexProjection, IndexSpec,
    IndexState, ScanDirection, ValueBound,
};
use tempfile::tempdir;

/// Entity of the rows.
const ENTITY: u64 = 9;

/// Index on (field 1 `direction_1`, field 2 ascending) INCLUDE (field 3).
fn definition(direction_1: Direction) -> IndexDefinition {
    IndexDefinition::new(
        IndexId(1),
        IndexSpec {
            name: "idx".into(),
            entity_id: ENTITY,
            columns: vec![
                IndexColumn {
                    field_id: 1,
                    direction: direction_1,
                },
                IndexColumn {
                    field_id: 2,
                    direction: Direction::Asc,
                },
            ],
            include: vec![3],
        },
        IndexState::Active,
        CommitTs(0),
    )
}

/// A row with fields 1 (small int or NULL), 2 (small int) and 3 (payload).
fn row(a: Option<i64>, b: i64, payload: i64) -> Row {
    let row = Row::new()
        .with_field(2, Value::Int64(b))
        .with_field(3, Value::Int64(payload));
    match a {
        Some(a) => row.with_field(1, Value::Int64(a)),
        None => row,
    }
}

/// One version of one row in the model.
#[derive(Debug, Clone)]
struct Version {
    /// Row.
    row_id: RowId,
    /// Content.
    row: Row,
    /// First commit.
    begin: u64,
    /// End commit (`u64::MAX` while current).
    end: u64,
}

/// Deterministic pseudo-random words.
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// SQL order of two values of one key column (NULLs last ascending, first descending).
fn compare(a: &Value, b: &Value, direction: Direction) -> Ordering {
    let ordering = match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        _ => unreachable!(),
    };
    match direction {
        Direction::Asc => ordering,
        Direction::Desc => ordering.reverse(),
    }
}

/// Whether the model version matches `bounds` (prefix equality, then the range).
fn matches(definition: &IndexDefinition, version: &Version, bounds: &IndexBounds) -> bool {
    let key = definition.key_values(&version.row);
    for (value, wanted) in key.iter().zip(&bounds.prefix) {
        if value != wanted {
            return false;
        }
    }
    let column = bounds.prefix.len();
    if bounds.lower.is_none() && bounds.upper.is_none() {
        return true;
    }
    let value = &key[column];
    if matches!(value, Value::Null) {
        return false;
    }
    let lower_ok = bounds.lower.as_ref().is_none_or(|b| {
        let ord = compare(value, &b.value, Direction::Asc);
        ord == Ordering::Greater || (b.inclusive && ord == Ordering::Equal)
    });
    let upper_ok = bounds.upper.as_ref().is_none_or(|b| {
        let ord = compare(value, &b.value, Direction::Asc);
        ord == Ordering::Less || (b.inclusive && ord == Ordering::Equal)
    });
    lower_ok && upper_ok
}

/// Reads every page of a scan, checking that each page respects the limits.
fn scan_all(
    projection: &IndexProjection,
    bounds: &IndexBounds,
    snapshot: u64,
    direction: ScanDirection,
) -> Vec<IndexEntry> {
    let mut out = Vec::new();
    let mut resume: Option<Vec<u8>> = None;
    for _ in 0..100_000 {
        let page = projection
            .scan(
                bounds,
                CommitTs(snapshot),
                direction,
                resume.as_deref(),
                7,
                20,
            )
            .unwrap();
        assert!(page.entries.len() <= 7 && page.examined <= 20);
        out.extend(page.entries);
        match page.resume {
            Some(key) => resume = Some(key),
            None => return out,
        }
    }
    panic!("scan did not terminate");
}

/// Random inserts, updates (some leaving the key unchanged) and deletes; then scans at random
/// snapshots with random bounds in both directions equal the model, in index order.
#[test]
fn scans_at_any_snapshot_match_the_model() {
    for direction_1 in [Direction::Asc, Direction::Desc] {
        let dir = tempdir().unwrap();
        let definition = definition(direction_1);
        let projection =
            IndexProjection::open(dir.path(), definition.clone(), 16, CommitTs(0)).unwrap();
        let mut model: Vec<Version> = Vec::new();
        let mut words = Words(0xDEAD_BEEF_0042 + direction_1 as u64);
        for ts in 1..=1500u64 {
            let pk = (words.next() % 120) as i64 - 60;
            let row_id = RowId::from_primary_key(ENTITY, pk);
            let current = model
                .iter()
                .position(|v| v.row_id == row_id && v.end == u64::MAX);
            let before = current.map(|i| model[i].row.clone());
            let after = if words.next().is_multiple_of(6) {
                None
            } else {
                let a = (!words.next().is_multiple_of(8)).then(|| (words.next() % 10) as i64 - 5);
                let b = (words.next() % 4) as i64;
                Some(row(a, b, (words.next() % 3) as i64))
            };
            projection
                .apply_row(
                    row_id,
                    before.as_ref(),
                    after.as_ref(),
                    CommitTs(ts),
                    Lsn(ts),
                )
                .unwrap();
            let same = before.as_ref().zip(after.as_ref()).is_some_and(|(b, a)| {
                definition.key_values(b) == definition.key_values(a)
                    && definition.included_values(b) == definition.included_values(a)
            });
            if same {
                let index = current.unwrap();
                model[index].row = after.unwrap();
                continue;
            }
            if let Some(index) = current {
                model[index].end = ts;
            }
            if let Some(row) = after {
                model.push(Version {
                    row_id,
                    row,
                    begin: ts,
                    end: u64::MAX,
                });
            }
        }
        projection.verify_structure().unwrap();
        assert_eq!(projection.watermark(), CommitTs(1500));

        for _ in 0..120 {
            let snapshot = 1 + words.next() % 1500;
            let mut bounds = IndexBounds::all();
            match words.next() % 4 {
                0 => {}
                1 => bounds.prefix = vec![Value::Int64((words.next() % 10) as i64 - 5)],
                2 => {
                    bounds.lower = (words.next().is_multiple_of(2)).then(|| ValueBound {
                        value: Value::Int64((words.next() % 10) as i64 - 5),
                        inclusive: words.next().is_multiple_of(2),
                    });
                    bounds.upper = Some(ValueBound {
                        value: Value::Int64((words.next() % 10) as i64 - 2),
                        inclusive: words.next().is_multiple_of(2),
                    });
                }
                _ => {
                    bounds.prefix = vec![Value::Int64((words.next() % 10) as i64 - 5)];
                    bounds.lower = Some(ValueBound {
                        value: Value::Int64((words.next() % 4) as i64),
                        inclusive: words.next().is_multiple_of(2),
                    });
                }
            }
            let mut expected: Vec<&Version> = model
                .iter()
                .filter(|v| {
                    v.begin <= snapshot && snapshot < v.end && matches(&definition, v, &bounds)
                })
                .collect();
            expected.sort_by(|x, y| {
                let (kx, ky) = (definition.key_values(&x.row), definition.key_values(&y.row));
                compare(&kx[0], &ky[0], direction_1)
                    .then(compare(&kx[1], &ky[1], Direction::Asc))
                    .then(x.row_id.cmp(&y.row_id))
            });
            let expected: Vec<(RowId, Vec<Value>, Vec<Value>)> = expected
                .iter()
                .map(|v| {
                    (
                        v.row_id,
                        definition.key_values(&v.row),
                        definition.included_values(&v.row),
                    )
                })
                .collect();
            let forward: Vec<(RowId, Vec<Value>, Vec<Value>)> =
                scan_all(&projection, &bounds, snapshot, ScanDirection::Forward)
                    .into_iter()
                    .map(|e| (e.row_id, e.key, e.included))
                    .collect();
            assert_eq!(forward, expected, "{bounds:?} at {snapshot}");
            let mut backward: Vec<(RowId, Vec<Value>, Vec<Value>)> =
                scan_all(&projection, &bounds, snapshot, ScanDirection::Backward)
                    .into_iter()
                    .map(|e| (e.row_id, e.key, e.included))
                    .collect();
            backward.reverse();
            assert_eq!(backward, expected, "reversed {bounds:?} at {snapshot}");
        }

        // Vacuum removes exactly the versions that ended at or before the horizon.
        let ended = model.iter().filter(|v| v.end <= 700).count() as u64;
        assert_eq!(projection.vacuum(CommitTs(700), Lsn(2000)).unwrap(), ended);
        let current = model.iter().filter(|v| v.end == u64::MAX).count();
        assert_eq!(projection.open_entries().unwrap().len(), current);
        projection.verify_structure().unwrap();
    }
}

/// Unchanged keys and payloads write nothing; a missing current entry is corruption.
#[test]
fn unchanged_rows_cost_no_write_and_lost_entries_are_detected() {
    let dir = tempdir().unwrap();
    let projection =
        IndexProjection::open(dir.path(), definition(Direction::Asc), 16, CommitTs(0)).unwrap();
    let id = RowId::from_primary_key(ENTITY, 1);
    projection
        .apply_row(id, None, Some(&row(Some(1), 1, 1)), CommitTs(1), Lsn(1))
        .unwrap();
    projection.flush().unwrap();
    let unrelated = row(Some(1), 1, 1).with_field(7, Value::String("not indexed".into()));
    projection
        .apply_row(
            id,
            Some(&row(Some(1), 1, 1)),
            Some(&unrelated),
            CommitTs(2),
            Lsn(2),
        )
        .unwrap();
    assert_eq!(
        projection.dirty_pages(),
        0,
        "an unindexed column changed nothing"
    );
    assert_eq!(projection.watermark(), CommitTs(2));
    let other = RowId::from_primary_key(ENTITY, 2);
    let error = projection
        .apply_row(other, Some(&row(Some(5), 5, 5)), None, CommitTs(3), Lsn(3))
        .unwrap_err();
    assert!(error.is_corruption(), "{error:?}");
}

/// Rows that cannot be indexed are refused by the pre-commit check; bad bounds are refused.
#[test]
fn oversized_rows_and_bad_bounds_are_refused() {
    let dir = tempdir().unwrap();
    let projection =
        IndexProjection::open(dir.path(), definition(Direction::Asc), 16, CommitTs(0)).unwrap();
    let huge = Row::new().with_field(1, Value::String("x".repeat(600)));
    assert!(matches!(
        projection.check_row(&huge),
        Err(IndexError::KeyTooLarge { .. })
    ));
    let null_bound = IndexBounds {
        prefix: vec![Value::Null],
        ..IndexBounds::all()
    };
    assert!(projection
        .scan(
            &null_bound,
            CommitTs(1),
            ScanDirection::Forward,
            None,
            10,
            10
        )
        .is_err());
    let too_long = IndexBounds {
        prefix: vec![Value::Int64(1), Value::Int64(2)],
        lower: Some(ValueBound {
            value: Value::Int64(3),
            inclusive: true,
        }),
        upper: None,
    };
    assert!(too_long.byte_range(projection.definition()).is_err());
}
