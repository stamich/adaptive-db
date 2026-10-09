//! Statistics document: JSON round trip, validation and option bounds.
use adb_core::Value;
use adb_stats::{
    AnalyzeOptions, ColumnStatistics, HistogramBucket, MostCommonValue, StatsError,
    TableStatistics, STATISTICS_FORMAT_VERSION,
};

/// A small valid document.
fn sample() -> TableStatistics {
    TableStatistics {
        format_version: STATISTICS_FORMAT_VERSION,
        entity_id: 7,
        row_count: 100,
        avg_row_bytes: 24.5,
        analyzed_at_ts: 42,
        modifications_at_analyze: 3,
        sampled_rows: 100,
        exact: true,
        columns: vec![
            ColumnStatistics {
                field_id: 1,
                null_count: 0,
                distinct_count: 100,
                distinct_exact: true,
                min: Some(Value::Int64(1)),
                max: Some(Value::Int64(100)),
                avg_width_bytes: 8.0,
                histogram: vec![HistogramBucket {
                    lower: Value::Int64(1),
                    upper: Value::Int64(100),
                    rows: 100,
                    distinct: 100,
                }],
                most_common: Vec::new(),
            },
            ColumnStatistics {
                field_id: 2,
                null_count: 10,
                distinct_count: 2,
                distinct_exact: true,
                min: Some(Value::String("DE".into())),
                max: Some(Value::String("PL".into())),
                avg_width_bytes: 2.0,
                histogram: Vec::new(),
                most_common: vec![MostCommonValue {
                    value: Value::String("PL".into()),
                    rows: 60,
                }],
            },
        ],
    }
}

/// A document survives JSON encoding unchanged, and columns are found by field id.
#[test]
fn json_round_trip() {
    let statistics = sample();
    let json = statistics.to_json().unwrap();
    assert!(std::str::from_utf8(&json).unwrap().contains("\"Int64\":1"));
    let decoded = TableStatistics::from_json(&json).unwrap();
    assert_eq!(decoded, statistics);
    assert_eq!(decoded.column(2).unwrap().null_count, 10);
    assert!(decoded.column(3).is_none());
}

/// Inconsistent documents are rejected.
#[test]
fn invalid_documents_are_rejected() {
    let mut wrong_version = sample();
    wrong_version.format_version = 99;
    assert!(matches!(
        wrong_version.to_json(),
        Err(StatsError::Format(_))
    ));

    let mut unordered = sample();
    unordered.columns.reverse();
    assert!(unordered.validate().is_err());

    let mut too_many_nulls = sample();
    too_many_nulls.columns[1].null_count = 101;
    assert!(too_many_nulls.validate().is_err());

    assert!(TableStatistics::from_json(br#"{"format_version":1}"#).is_err());
}

/// Options decode from JSON with defaults, and out-of-range values are refused.
#[test]
fn options_decode_and_validate() {
    assert_eq!(
        AnalyzeOptions::from_json(b"").unwrap(),
        AnalyzeOptions::default()
    );
    let options = AnalyzeOptions::from_json(br#"{"fields":[1,2],"sample_rows":500}"#).unwrap();
    assert_eq!(options.fields, Some(vec![1, 2]));
    assert_eq!(options.sample_rows, 500);
    assert_eq!(options.histogram_buckets, 64);
    for bad in [
        r#"{"sample_rows":0}"#,
        r#"{"histogram_buckets":1000}"#,
        r#"{"max_rows":0}"#,
        r#"{"unknown":1}"#,
    ] {
        assert!(
            matches!(
                AnalyzeOptions::from_json(bad.as_bytes()),
                Err(StatsError::InvalidOptions(_))
            ),
            "{bad}"
        );
    }
}
