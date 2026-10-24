//! Deterministic fuzzing of the order-preserving key encoding: arbitrary and corrupted bytes
//! never panic the decoder, decoding is stable under re-encoding, and adversarial values
//! (embedded escape bytes, long strings, extreme numbers) keep the byte order equal to the
//! value order.
use std::cmp::Ordering;

use adb_core::{
    key_encoding::{decode_key, encode_key, prefix_successor, Direction},
    Value,
};

/// Deterministic pseudo-random words (xorshift64).
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// A byte string of up to `max` bytes biased towards the encoding's special bytes.
    fn bytes(&mut self, max: u64) -> Vec<u8> {
        let len = self.below(max + 1) as usize;
        (0..len)
            .map(|_| match self.below(6) {
                0 => 0x00,
                1 => 0xFF,
                2 => 0x01,
                _ => self.next() as u8,
            })
            .collect()
    }

    /// A value of a random type, favouring edge cases.
    fn value(&mut self) -> Value {
        match self.below(7) {
            0 => Value::Null,
            1 => Value::Bool(self.below(2) == 1),
            2 => Value::Int64(match self.below(4) {
                0 => i64::MIN,
                1 => i64::MAX,
                2 => self.below(5) as i64 - 2,
                _ => self.next() as i64,
            }),
            3 => Value::Float64(match self.below(6) {
                0 => f64::NEG_INFINITY,
                1 => f64::INFINITY,
                2 => f64::MIN_POSITIVE,
                3 => -f64::MIN_POSITIVE,
                // NaN orders only in its canonical form; the unit tests cover it.
                _ => Some(f64::from_bits(self.next()))
                    .filter(|v| !v.is_nan())
                    .unwrap_or(0.5),
            }),
            4 => Value::String(
                String::from_utf8_lossy(&self.bytes(40))
                    .chars()
                    .filter(|c| *c != char::REPLACEMENT_CHARACTER)
                    .collect(),
            ),
            _ => Value::Bytes(self.bytes(40)),
        }
    }

    /// A direction.
    fn direction(&mut self) -> Direction {
        if self.below(2) == 0 {
            Direction::Asc
        } else {
            Direction::Desc
        }
    }
}

/// SQL order of two values of one column, ascending: by type tag, NULL last.
fn reference(a: &Value, b: &Value) -> Ordering {
    /// Type rank in tag order.
    fn rank(value: &Value) -> u8 {
        match value {
            Value::Bool(_) => 1,
            Value::Int64(_) => 2,
            Value::Float64(_) => 3,
            Value::String(_) => 4,
            Value::Bytes(_) => 5,
            Value::Null => 6,
        }
    }
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => {
            let normalize = |v: f64| if v == 0.0 { 0.0 } else { v };
            normalize(*x).total_cmp(&normalize(*y))
        }
        (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)),
    }
}

/// Random bytes never panic the decoder; whatever decodes is stable: re-encoding the values
/// and decoding again yields the same values.
#[test]
fn arbitrary_bytes_never_panic() {
    let mut words = Words(0x9E37_79B9_7F4A_7C15);
    let mut decoded = 0;
    for _ in 0..50_000 {
        let bytes = words.bytes(48);
        let directions: Vec<Direction> =
            (0..words.below(4) + 1).map(|_| words.direction()).collect();
        if let Ok((values, used)) = decode_key(&bytes, &directions) {
            decoded += 1;
            assert!(used <= bytes.len());
            let again = encode_key(values.iter().zip(directions.iter().copied()));
            let (stable, used_again) = decode_key(&again, &directions).unwrap();
            assert_eq!(used_again, again.len());
            assert_eq!(format!("{stable:?}"), format!("{values:?}"));
        }
    }
    assert!(decoded > 0, "some random inputs are valid keys");
}

/// Every truncation of a valid key is rejected or decodes fewer bytes; flipping any byte never
/// panics.
#[test]
fn corrupted_keys_are_rejected_without_panics() {
    let mut words = Words(42);
    for _ in 0..2_000 {
        let columns: Vec<(Value, Direction)> = (0..words.below(4) + 1)
            .map(|_| (words.value(), words.direction()))
            .collect();
        let directions: Vec<Direction> = columns.iter().map(|(_, d)| *d).collect();
        let key = encode_key(columns.iter().map(|(v, d)| (v, *d)));
        let (values, used) = decode_key(&key, &directions).unwrap();
        assert_eq!(used, key.len());
        assert_eq!(values.len(), columns.len());
        for cut in 0..key.len() {
            assert!(
                decode_key(&key[..cut], &directions).is_err(),
                "a truncated key must not decode: {columns:?} cut at {cut}"
            );
        }
        let mut flipped = key.clone();
        let at = words.below(key.len() as u64) as usize;
        flipped[at] ^= 1 << words.below(8);
        let _ = decode_key(&flipped, &directions);
    }
}

/// Adversarial composite keys order exactly as their values, column by column, in both
/// directions; prefix successors bound every key that starts with the prefix.
#[test]
fn adversarial_values_keep_their_order() {
    let mut words = Words(7);
    for _ in 0..20_000 {
        let directions: Vec<Direction> =
            (0..words.below(3) + 1).map(|_| words.direction()).collect();
        let a: Vec<Value> = directions.iter().map(|_| words.value()).collect();
        let b: Vec<Value> = directions
            .iter()
            .zip(&a)
            .map(|(_, v)| {
                if words.below(3) == 0 {
                    v.clone()
                } else {
                    words.value()
                }
            })
            .collect();
        let expected = a
            .iter()
            .zip(&b)
            .zip(&directions)
            .map(|((x, y), d)| match d {
                Direction::Asc => reference(x, y),
                Direction::Desc => reference(y, x),
            })
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal);
        let ka = encode_key(a.iter().zip(directions.iter().copied()));
        let kb = encode_key(b.iter().zip(directions.iter().copied()));
        assert_eq!(ka.cmp(&kb), expected, "{a:?} vs {b:?} ({directions:?})");
        if let Some(successor) = prefix_successor(&ka) {
            assert!(successor > ka);
            let mut extended = ka.clone();
            extended.extend_from_slice(&words.bytes(8));
            assert!(extended < successor);
        }
    }
}
