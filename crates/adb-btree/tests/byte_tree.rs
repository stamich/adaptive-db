//! Behavioural tests of the byte-keyed B+Tree used by secondary indexes.

use std::{collections::BTreeMap, ops::Bound};

use adb_btree::{BTree, BTreeError, ByteTree, MAX_KEY_BYTES, MAX_VALUE_BYTES};
use adb_core::{Lsn, PageId, RowId, RowLocation};
use tempfile::tempdir;

/// Opens the tree in `dir` with a small cache (forcing evictions of clean pages).
fn open(dir: &std::path::Path) -> ByteTree {
    ByteTree::open(dir.join("tree.dat"), dir.join("tree.meta"), 8).unwrap()
}

/// Deterministic pseudo-random words (xorshift).
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A key of 1..=40 bytes with a skewed alphabet (shared prefixes, zero bytes).
    fn key(&mut self) -> Vec<u8> {
        let len = 1 + (self.next() % 40) as usize;
        (0..len)
            .map(|_| [0u8, 1, 7, 0xFF, b'k'][(self.next() % 5) as usize])
            .collect()
    }
}

/// Bound over an owned key.
fn bound(kind: u64, key: &[u8]) -> Bound<&[u8]> {
    match kind % 3 {
        0 => Bound::Included(key),
        1 => Bound::Excluded(key),
        _ => Bound::Unbounded,
    }
}

/// Random inserts, replacements and removes agree with an ordered map; so do forward and
/// reverse range scans with random bounds and limits, before and after a reopen.
#[test]
fn matches_an_ordered_map_model() {
    let dir = tempdir().unwrap();
    let tree = open(dir.path());
    let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut words = Words(0xC0FF_EE00_1234_5678);
    for step in 0..6000u64 {
        let key = words.key();
        if step % 5 == 4 {
            assert_eq!(tree.remove(&key, Lsn(step)).unwrap(), model.remove(&key));
        } else {
            let value = vec![(step % 251) as u8; (step % 90) as usize];
            tree.insert(&key, &value, Lsn(step)).unwrap();
            model.insert(key, value);
        }
    }
    let check = |tree: &ByteTree, words: &mut Words| {
        for _ in 0..300 {
            let (a, b) = (words.key(), words.key());
            let (low, high) = if a <= b { (a, b) } else { (b, a) };
            let (lower, upper) = (bound(words.next(), &low), bound(words.next(), &high));
            let limit = (words.next() % 50) as usize;
            let expected: Vec<(Vec<u8>, Vec<u8>)> = model
                .range::<[u8], _>((lower, upper))
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            assert_eq!(tree.range(lower, upper, limit).unwrap(), expected);
            let reversed: Vec<(Vec<u8>, Vec<u8>)> = model
                .range::<[u8], _>((lower, upper))
                .rev()
                .take(limit)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            assert_eq!(tree.range_rev(lower, upper, limit).unwrap(), reversed);
        }
        for (key, value) in &model {
            assert_eq!(tree.get(key).unwrap().as_ref(), Some(value));
        }
    };
    check(&tree, &mut words);
    assert_eq!(tree.shape().unwrap().entries, model.len() as u64);
    tree.flush().unwrap();
    drop(tree);
    check(&open(dir.path()), &mut words);
}

/// A keyset scan resumed after the last key returned visits every entry exactly once.
#[test]
fn keyset_scans_resume_after_the_last_key() {
    let dir = tempdir().unwrap();
    let tree = open(dir.path());
    for i in 0..2000u32 {
        tree.insert(&i.to_be_bytes(), &[1], Lsn(1)).unwrap();
    }
    let mut seen = Vec::new();
    let mut after: Option<Vec<u8>> = None;
    loop {
        let lower = after.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
        let page = tree.range(lower, Bound::Unbounded, 333).unwrap();
        let Some((last, _)) = page.last() else { break };
        after = Some(last.clone());
        seen.extend(page.into_iter().map(|(k, _)| k));
    }
    assert_eq!(seen.len(), 2000);
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
    let shape = tree.shape().unwrap();
    assert!(shape.height >= 2 && shape.leaf_pages > 1, "{shape:?}");
}

/// Maximum-size keys and values split correctly; oversized ones are refused unchanged.
#[test]
fn large_entries_split_and_limits_are_enforced() {
    let dir = tempdir().unwrap();
    let tree = open(dir.path());
    for i in 0..200u16 {
        let mut key = vec![b'x'; MAX_KEY_BYTES];
        key[..2].copy_from_slice(&i.to_be_bytes());
        tree.insert(&key, &vec![i as u8; MAX_VALUE_BYTES], Lsn(1))
            .unwrap();
    }
    assert_eq!(
        tree.range(Bound::Unbounded, Bound::Unbounded, usize::MAX)
            .unwrap()
            .len(),
        200
    );
    let too_long = vec![0u8; MAX_KEY_BYTES + 1];
    assert!(matches!(
        tree.insert(&too_long, &[], Lsn(2)),
        Err(BTreeError::Limit(_))
    ));
    assert!(matches!(
        tree.insert(&[], &[], Lsn(2)),
        Err(BTreeError::Limit(_))
    ));
    assert!(matches!(
        tree.insert(b"k", &vec![0; MAX_VALUE_BYTES + 1], Lsn(2)),
        Err(BTreeError::Limit(_))
    ));
    assert_eq!(
        tree.get(b"k").unwrap(),
        None,
        "refused inserts change nothing"
    );
    tree.insert(b"k", &[], Lsn(2)).unwrap();
    assert_eq!(tree.get(b"k").unwrap(), Some(Vec::new()));
}

/// A damaged page is reported as corruption, never misread.
#[test]
fn corrupted_pages_are_detected() {
    let dir = tempdir().unwrap();
    let tree = open(dir.path());
    for i in 0..500u32 {
        tree.insert(&i.to_be_bytes(), b"value", Lsn(1)).unwrap();
    }
    tree.flush().unwrap();
    drop(tree);
    let path = dir.path().join("tree.dat");
    let mut bytes = std::fs::read(&path).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x5A;
    std::fs::write(&path, bytes).unwrap();
    let tree = open(dir.path());
    let error = (0..500u32)
        .find_map(|i| tree.get(&i.to_be_bytes()).err())
        .or_else(|| {
            tree.range(Bound::Unbounded, Bound::Unbounded, usize::MAX)
                .err()
        })
        .expect("the damage is noticed");
    assert!(error.is_corruption(), "{error:?}");
}

/// Batched lookups on the fixed-width tree equal single lookups, and reject unsorted input.
#[test]
fn get_many_equals_single_lookups() {
    let dir = tempdir().unwrap();
    let tree = BTree::open(dir.path().join("p.dat"), dir.path().join("p.meta"), 8).unwrap();
    for i in (0..3000u128).step_by(3) {
        let location = RowLocation {
            page_id: PageId(i as u64),
            slot_id: 1,
        };
        tree.insert(RowId(i), location).unwrap();
    }
    let keys: Vec<RowId> = (0..3000u128).step_by(7).map(RowId).collect();
    let batched = tree.get_many(&keys).unwrap();
    for (key, found) in keys.iter().zip(batched) {
        assert_eq!(found, tree.get(*key).unwrap());
    }
    assert!(tree.get_many(&[RowId(5), RowId(1)]).is_err());
}
