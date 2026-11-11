//! Tests of the fsync control probe.
use adb_bench::probe::PROBE_BLOCK_BYTES;
use adb_bench::FsyncProbe;

#[test]
fn the_probe_times_every_sample_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let probe = FsyncProbe::run(dir.path(), 20).unwrap();
    assert_eq!(probe.samples_ns.len(), 20);
    assert_eq!(probe.block_bytes, PROBE_BLOCK_BYTES);
    assert!(probe.samples_ns.iter().all(|ns| *ns > 0.0));
    assert!(probe.p50_ns() > 0.0 && probe.p95_ns() >= probe.p50_ns());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "the probe file is removed"
    );
}

#[test]
fn the_probe_fails_cleanly_in_a_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no/such/dir");
    assert!(FsyncProbe::run(&missing, 3).is_err());
}

#[test]
fn drift_compares_the_medians() {
    let before = FsyncProbe {
        block_bytes: 4096,
        samples_ns: vec![100.0, 100.0, 100.0],
    };
    let slower = FsyncProbe {
        block_bytes: 4096,
        samples_ns: vec![150.0, 150.0, 150.0],
    };
    let same = FsyncProbe {
        block_bytes: 4096,
        samples_ns: vec![100.0, 100.0, 100.0],
    };
    assert!((before.drift_percent(&slower) - 50.0).abs() < 1e-9);
    assert_eq!(before.drift_percent(&same), 0.0);
    let empty = FsyncProbe {
        block_bytes: 4096,
        samples_ns: vec![],
    };
    assert_eq!(empty.drift_percent(&slower), 0.0, "no baseline, no drift");
    assert_eq!(empty.p50_ns(), 0.0);
    assert_eq!(empty.p95_ns(), 0.0);
}

#[test]
fn the_json_reports_microseconds() {
    let probe = FsyncProbe {
        block_bytes: 4096,
        samples_ns: vec![2000.0, 4000.0, 6000.0],
    };
    let json = probe.to_json();
    assert_eq!(json["samples"], 3);
    assert_eq!(json["p50_us"], 4.0);
    assert_eq!(json["p95_us"], 6.0);
    assert_eq!(json["block_bytes"], 4096);
}
