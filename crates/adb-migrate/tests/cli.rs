//! The `adb-migrate` command line.
use std::process::Command;

use adb_core::{Row, RowId, Value};
use adb_engine::{format::FORMAT_FILE, Database};
use tempfile::tempdir;

/// Runs the binary with `args`; returns its exit code and output.
fn run(args: &[&std::path::Path]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_adb-migrate"))
        .args(args)
        .output()
        .expect("binary runs");
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.code().unwrap_or(-1), text)
}

/// A format-1 directory is migrated into a target and reports its rows; bad usage exits 2.
#[test]
fn migrates_into_a_target_directory() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    {
        let db = Database::open(&source).unwrap();
        let mut tx = db.begin();
        tx.put(
            RowId::compose(2, (-5i64) as u64),
            Row::new().with_field(1, Value::Int64(1)),
        );
        db.commit(tx).unwrap();
        db.close().unwrap();
    }
    std::fs::remove_file(source.join(FORMAT_FILE)).unwrap();
    let target = dir.path().join("target");
    let (code, text) = run(&[&source, &target]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("entity 2: 1 rows"), "{text}");
    let db = Database::open(&target).unwrap();
    assert!(db.get(RowId::from_primary_key(2, -5)).unwrap().is_some());

    let (code, text) = run(&[&source, &target]);
    assert_eq!(code, 1, "a used target is refused: {text}");
    assert_eq!(run(&[]).0, 2);
}
