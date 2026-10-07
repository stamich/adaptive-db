//! Crash-protocol tests for the checkpoint journal.

use std::fs;

use adb_journal::{FileWrite, Journal, JournalError, JOURNAL_FILE};
use tempfile::tempdir;

/// A commit applies range and replace writes and removes the journal.
#[test]
fn commit_applies_writes_and_leaves_no_journal() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("data"), [0u8; 16]).unwrap();
    let journal = Journal::new(dir.path());

    journal
        .commit(&[
            FileWrite::range(dir.path().join("data"), 4, vec![1, 2]),
            FileWrite::replace(dir.path().join("meta"), b"root=7".to_vec()),
        ])
        .unwrap();

    let data = fs::read(dir.path().join("data")).unwrap();
    assert_eq!(&data[4..6], &[1, 2]);
    assert_eq!(fs::read(dir.path().join("meta")).unwrap(), b"root=7");
    assert!(!dir.path().join(JOURNAL_FILE).exists());
}

/// A range write past the end of a file extends it.
#[test]
fn range_write_extends_the_file() {
    let dir = tempdir().unwrap();
    let journal = Journal::new(dir.path());
    journal
        .commit(&[FileWrite::range(dir.path().join("pages"), 10, vec![5; 6])])
        .unwrap();
    assert_eq!(fs::read(dir.path().join("pages")).unwrap().len(), 16);
}

/// Simulates a crash after the commit point but before the in-place writes completed:
/// the apply step is forced to fail, leaving a committed journal and untouched targets.
#[test]
fn recover_reapplies_a_committed_journal() {
    let dir = tempdir().unwrap();
    // A directory where a file is expected makes the in-place write fail after the commit point.
    fs::create_dir(dir.path().join("data")).unwrap();
    let journal = Journal::new(dir.path());
    assert!(journal
        .commit(&[FileWrite::range("data", 0, vec![42; 8])])
        .is_err());
    assert!(dir.path().join(JOURNAL_FILE).exists());

    // "Restart": the obstacle is gone and the target holds its old content.
    fs::remove_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data"), [0u8; 8]).unwrap();

    assert!(journal.recover().unwrap());
    assert_eq!(fs::read(dir.path().join("data")).unwrap(), vec![42; 8]);
    assert!(!dir.path().join(JOURNAL_FILE).exists());
    assert!(!journal.recover().unwrap());
}

/// A `.tmp` journal never reached its commit point and is discarded.
#[test]
fn half_written_tmp_journal_is_discarded() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("data"), [0u8; 4]).unwrap();
    fs::write(dir.path().join("checkpoint.journal.tmp"), b"garbage").unwrap();

    assert!(!Journal::new(dir.path()).recover().unwrap());
    assert_eq!(fs::read(dir.path().join("data")).unwrap(), vec![0u8; 4]);
    assert!(!dir.path().join("checkpoint.journal.tmp").exists());
}

/// A corrupt committed journal is an error, not silently skipped.
#[test]
fn corrupt_committed_journal_is_reported_not_ignored() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(JOURNAL_FILE), b"ADBJRNL1 not a real body").unwrap();
    assert!(matches!(
        Journal::new(dir.path()).recover(),
        Err(JournalError::Corrupt(_))
    ));
}

/// Absolute paths outside the root and `..` components are rejected.
#[test]
fn writes_outside_root_are_rejected() {
    let dir = tempdir().unwrap();
    let other = tempdir().unwrap();
    let journal = Journal::new(dir.path());
    assert!(matches!(
        journal.commit(&[FileWrite::range(other.path().join("x"), 0, vec![1])]),
        Err(JournalError::OutsideRoot(_))
    ));
    assert!(matches!(
        journal.commit(&[FileWrite::range("../escape", 0, vec![1])]),
        Err(JournalError::OutsideRoot(_))
    ));
}
