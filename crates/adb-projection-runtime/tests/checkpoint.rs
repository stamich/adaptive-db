//! Durable projection checkpoints: round trip, atomic replacement and damage detection.
use adb_core::{CommitSeq, ProjectionEpoch, ProjectionId};
use adb_projection_runtime::{
    CheckpointEntry, ProjectionCheckpoint, ProjectionCheckpointStore, ProjectionDescriptor,
    ProjectionFrontier, ProjectionKind, ProjectionMode, ProjectionState,
};
use tempfile::tempdir;

/// A checkpoint with one synchronous and one asynchronous projection.
fn sample() -> ProjectionCheckpoint {
    ProjectionCheckpoint {
        epoch: ProjectionEpoch(7),
        entries: vec![
            CheckpointEntry {
                descriptor: ProjectionDescriptor::new(
                    ProjectionId(1),
                    "__current_row",
                    ProjectionKind::CurrentRow,
                    ProjectionMode::Synchronous,
                    None,
                ),
                state: ProjectionState::Ready,
                frontier: ProjectionFrontier::at(CommitSeq(42)),
            },
            CheckpointEntry {
                descriptor: ProjectionDescriptor::new(
                    ProjectionId(2),
                    "feed",
                    ProjectionKind::TestAsync,
                    ProjectionMode::Asynchronous,
                    Some(3),
                ),
                state: ProjectionState::NeedsRebuild,
                frontier: ProjectionFrontier::at(CommitSeq(10)),
            },
        ],
    }
}

/// A missing file is an empty checkpoint; a saved one loads back unchanged.
#[test]
fn save_and_load_round_trip() {
    let dir = tempdir().unwrap();
    let store = ProjectionCheckpointStore::new(dir.path());
    assert_eq!(store.load().unwrap(), ProjectionCheckpoint::default());
    store.save(&sample()).unwrap();
    assert_eq!(store.load().unwrap(), sample());
    let mut newer = sample();
    newer.epoch = ProjectionEpoch(8);
    store.save(&newer).unwrap();
    assert_eq!(store.load().unwrap(), newer);
}

/// A save that died before the rename leaves the previous checkpoint intact and the stray
/// temporary file is ignored (and removed).
#[test]
fn an_interrupted_save_keeps_the_old_checkpoint() {
    let dir = tempdir().unwrap();
    let store = ProjectionCheckpointStore::new(dir.path());
    store.save(&sample()).unwrap();
    std::fs::write(dir.path().join("projections.meta.tmp"), b"half a checkpoi").unwrap();
    assert_eq!(store.load().unwrap(), sample());
    assert!(!dir.path().join("projections.meta.tmp").exists());
}

/// Every flipped byte and every truncation is an error, never a silently altered checkpoint.
#[test]
fn damage_is_detected() {
    let dir = tempdir().unwrap();
    let store = ProjectionCheckpointStore::new(dir.path());
    store.save(&sample()).unwrap();
    let path = store.path();
    let original = std::fs::read(&path).unwrap();
    for at in 0..original.len() {
        let mut damaged = original.clone();
        damaged[at] ^= 0x40;
        std::fs::write(&path, &damaged).unwrap();
        assert!(store.load().is_err(), "flip at {at}");
    }
    for cut in 0..original.len() {
        std::fs::write(&path, &original[..cut]).unwrap();
        assert!(store.load().is_err(), "cut at {cut}");
    }
    std::fs::write(&path, &original).unwrap();
    assert_eq!(store.load().unwrap(), sample());
}

/// A checkpoint whose frontier has the delta behind the base is refused.
#[test]
fn inconsistent_frontiers_are_refused() {
    let dir = tempdir().unwrap();
    let store = ProjectionCheckpointStore::new(dir.path());
    let mut bad = sample();
    bad.entries[0].frontier = ProjectionFrontier {
        base_commit_seq: CommitSeq(5),
        delta_commit_seq: CommitSeq(1),
    };
    store.save(&bad).unwrap();
    assert!(store.load().is_err());
}
