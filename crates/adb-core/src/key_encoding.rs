//! Order-preserving byte encoding of key values ("memcomparable" keys).
//!
//! [`encode_key`] turns a sequence of values, each with a sort [`Direction`], into a byte string
//! whose lexicographic (`memcmp`) order is the SQL order of the values: the first column decides,
//! ties are broken by the next one, and so on. Secondary indexes store these strings in a
//! B+Tree over plain byte keys, so a range of SQL values becomes a range of bytes.
//!
//! # Format of one column
//!
//! A type tag followed by a type-specific body. In ascending order:
//!
//! | Value | Tag | Body |
//! |---|---|---|
//! | `Bool` | `0x10` | `0x00` (false) or `0x01` (true) |
//! | `Int64` | `0x20` | 8 bytes big-endian of `value XOR 2^63` (sign bit flipped) |
//! | `Float64` | `0x30` | 8 bytes big-endian: positive numbers with the sign bit set, negative numbers with every bit inverted |
//! | `String` | `0x40` | UTF-8 bytes, `0x00` escaped as `0x00 0xFF`, terminated by `0x00 0x01` |
//! | `Bytes` | `0x50` | same escaping and terminator as strings |
//! | `Null` | `0xF0` | none |
//!
//! * NULL sorts **after** every value in ascending order and before every value in descending
//!   order, matching the engine's Sort and TopK (`NULLS LAST` ascending, `NULLS FIRST`
//!   descending), so an index scan can replace a sort.
//! * Floats: `-0.0` is stored as `0.0` and every NaN as one canonical NaN that sorts after
//!   `+inf` (SQL comparisons treat NaN as incomparable; an index never returns it for a range).
//! * A column in **descending** order is the ascending encoding with every byte inverted. The
//!   encoding of each column is prefix-free (fixed width or terminated), so inverting it reverses
//!   its order without disturbing the columns after it.
//! * Values of different types in one column order by type tag. Columns hold one type in
//!   practice (the JVM catalog types them); the tag only keeps the order total.
use serde::{Deserialize, Serialize};

use crate::{CoreError, Value};

/// Tag of a boolean value.
const TAG_BOOL: u8 = 0x10;
/// Tag of a 64-bit integer.
const TAG_INT64: u8 = 0x20;
/// Tag of a double.
const TAG_FLOAT64: u8 = 0x30;
/// Tag of a UTF-8 string.
const TAG_STRING: u8 = 0x40;
/// Tag of a byte string.
const TAG_BYTES: u8 = 0x50;
/// Tag of SQL NULL; the largest tag, so NULL sorts last in ascending order.
pub const TAG_NULL: u8 = 0xF0;
/// Sign bit of a 64-bit word.
const SIGN: u64 = 1 << 63;

/// Sort direction of one key column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Smallest value first, NULL last.
    #[default]
    Asc,
    /// Largest value first, NULL first.
    Desc,
}

/// Appends the encoding of one column value to `out`.
pub fn encode_value(value: &Value, direction: Direction, out: &mut Vec<u8>) {
    let start = out.len();
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(flag) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*flag));
        }
        Value::Int64(number) => {
            out.push(TAG_INT64);
            out.extend_from_slice(&((*number as u64) ^ SIGN).to_be_bytes());
        }
        Value::Float64(number) => {
            out.push(TAG_FLOAT64);
            out.extend_from_slice(&float_bits(*number).to_be_bytes());
        }
        Value::String(text) => {
            out.push(TAG_STRING);
            escape(text.as_bytes(), out);
        }
        Value::Bytes(bytes) => {
            out.push(TAG_BYTES);
            escape(bytes, out);
        }
    }
    if direction == Direction::Desc {
        for byte in &mut out[start..] {
            *byte = !*byte;
        }
    }
}

/// Encodes a composite key: every value in its own direction, in order.
pub fn encode_key<'a>(columns: impl IntoIterator<Item = (&'a Value, Direction)>) -> Vec<u8> {
    let mut out = Vec::new();
    for (value, direction) in columns {
        encode_value(value, direction, &mut out);
    }
    out
}

/// Decodes the values of a key encoded with `directions`; returns them and the number of bytes
/// read (the key may be followed by other data, e.g. an index entry's row id).
///
/// `-0.0` decodes as `0.0` and every NaN as the canonical NaN (see the module documentation).
pub fn decode_key(
    bytes: &[u8],
    directions: &[Direction],
) -> Result<(Vec<Value>, usize), CoreError> {
    let mut values = Vec::with_capacity(directions.len());
    let mut at = 0;
    for direction in directions {
        let (value, used) = decode_value(&bytes[at..], *direction)?;
        values.push(value);
        at += used;
    }
    Ok((values, at))
}

/// The smallest byte string that is greater than every string starting with `prefix`, or
/// `None` if there is none (`prefix` is empty or all `0xFF`).
///
/// Used as the exclusive upper bound of a prefix scan: `[prefix, prefix_successor(prefix))`
/// holds exactly the keys that start with `prefix`.
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut out = prefix.to_vec();
    while let Some(last) = out.pop() {
        if last != 0xFF {
            out.push(last + 1);
            return Some(out);
        }
    }
    None
}

/// Decodes one column value; returns it and the number of bytes it occupied.
fn decode_value(bytes: &[u8], direction: Direction) -> Result<(Value, usize), CoreError> {
    let read = |at: usize| -> Result<u8, CoreError> {
        let byte = *bytes
            .get(at)
            .ok_or_else(|| CoreError::InvalidData("truncated key".into()))?;
        Ok(match direction {
            Direction::Asc => byte,
            Direction::Desc => !byte,
        })
    };
    let word = |at: usize| -> Result<u64, CoreError> {
        let mut array = [0u8; 8];
        for (i, slot) in array.iter_mut().enumerate() {
            *slot = read(at + i)?;
        }
        Ok(u64::from_be_bytes(array))
    };
    match read(0)? {
        TAG_NULL => Ok((Value::Null, 1)),
        TAG_BOOL => match read(1)? {
            0 => Ok((Value::Bool(false), 2)),
            1 => Ok((Value::Bool(true), 2)),
            other => Err(CoreError::InvalidData(format!(
                "invalid boolean byte {other}"
            ))),
        },
        TAG_INT64 => Ok((Value::Int64((word(1)? ^ SIGN) as i64), 9)),
        TAG_FLOAT64 => {
            let bits = word(1)?;
            let raw = if bits & SIGN != 0 { bits ^ SIGN } else { !bits };
            Ok((Value::Float64(f64::from_bits(raw)), 9))
        }
        tag @ (TAG_STRING | TAG_BYTES) => {
            let mut body = Vec::new();
            let mut at = 1;
            loop {
                let byte = read(at)?;
                if byte != 0x00 {
                    body.push(byte);
                    at += 1;
                    continue;
                }
                match read(at + 1)? {
                    0xFF => body.push(0x00),
                    0x01 => break,
                    other => {
                        return Err(CoreError::InvalidData(format!(
                            "invalid escape byte {other}"
                        )))
                    }
                }
                at += 2;
            }
            let value = if tag == TAG_STRING {
                Value::String(
                    String::from_utf8(body)
                        .map_err(|_| CoreError::InvalidData("key string is not UTF-8".into()))?,
                )
            } else {
                Value::Bytes(body)
            };
            Ok((value, at + 2))
        }
        other => Err(CoreError::InvalidData(format!(
            "unknown key tag {other:#x}"
        ))),
    }
}

/// Order-preserving bit pattern of a double (see the module documentation).
fn float_bits(number: f64) -> u64 {
    let number = if number == 0.0 {
        0.0 // folds -0.0 into 0.0
    } else if number.is_nan() {
        f64::NAN
    } else {
        number
    };
    let bits = number.to_bits();
    if bits & SIGN == 0 {
        bits | SIGN
    } else {
        !bits
    }
}

/// Appends `bytes` with `0x00` escaped as `0x00 0xFF`, then the terminator `0x00 0x01`.
fn escape(bytes: &[u8], out: &mut Vec<u8>) {
    for byte in bytes {
        out.push(*byte);
        if *byte == 0x00 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0x00, 0x01]);
}

/// Unit tests of the key encoding; the order properties are checked against a reference
/// comparison over many generated values.
#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    /// SQL order of two values of one column (NULLS LAST), the reference for the encoding.
    fn reference(a: &Value, b: &Value) -> Ordering {
        fn rank(value: &Value) -> u8 {
            match value {
                Value::Bool(_) => 1,
                Value::Int64(_) => 2,
                Value::Float64(_) => 3,
                Value::String(_) => 4,
                Value::Bytes(_) => 5,
                Value::Null => 9,
            }
        }
        match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
            (Value::Float64(x), Value::Float64(y)) => normalize(*x).total_cmp(&normalize(*y)),
            (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
            (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
            _ => rank(a).cmp(&rank(b)),
        }
    }

    /// `-0.0 -> 0.0`, NaN -> canonical positive NaN (sorts after +inf under `total_cmp`).
    fn normalize(x: f64) -> f64 {
        if x == 0.0 {
            0.0
        } else if x.is_nan() {
            f64::NAN
        } else {
            x
        }
    }

    /// Deterministic generator of varied values (xorshift).
    struct Values(u64);

    impl Values {
        /// Next pseudo-random word.
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A value of the given kind (0 bool, 1 int, 2 float, 3 string, 4 bytes, 5 null).
        fn value(&mut self, kind: u64) -> Value {
            let word = self.next();
            match kind {
                0 => Value::Bool(word & 1 == 1),
                1 => Value::Int64(match word % 6 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    2 => -1,
                    3 => 0,
                    _ => word as i64 >> (word % 60),
                }),
                2 => Value::Float64(match word % 10 {
                    9 => -f64::NAN,
                    0 => f64::NEG_INFINITY,
                    1 => f64::INFINITY,
                    2 => -0.0,
                    3 => 0.0,
                    4 => f64::MIN_POSITIVE,
                    5 => -f64::MIN_POSITIVE,
                    _ => (word as i64 as f64) / ((word % 1000) as f64 + 1.0),
                }),
                3 | 4 => {
                    let len = (word % 6) as usize;
                    let bytes: Vec<u8> = (0..len)
                        .map(|_| [0u8, 1, 0xFF, b'a', b'b'][(self.next() % 5) as usize])
                        .collect();
                    if kind == 3 {
                        Value::String(bytes.iter().map(|b| char::from(*b % 128)).collect())
                    } else {
                        Value::Bytes(bytes)
                    }
                }
                _ => Value::Null,
            }
        }
    }

    /// For every pair of generated values of one kind (plus NULL), the byte order of the
    /// encodings equals the reference order, ascending and reversed for descending.
    #[test]
    fn byte_order_matches_sql_order() {
        let mut generator = Values(0x9E37_79B9_7F4A_7C15);
        for kind in 0..5 {
            let mut values: Vec<Value> = (0..60).map(|_| generator.value(kind)).collect();
            values.push(Value::Null);
            for a in &values {
                for b in &values {
                    let expected = reference(a, b);
                    let asc =
                        encode_key([(a, Direction::Asc)]).cmp(&encode_key([(b, Direction::Asc)]));
                    let desc =
                        encode_key([(a, Direction::Desc)]).cmp(&encode_key([(b, Direction::Desc)]));
                    assert_eq!(asc, expected, "{a:?} vs {b:?} ascending");
                    assert_eq!(desc, expected.reverse(), "{a:?} vs {b:?} descending");
                }
            }
        }
    }

    /// Composite keys order by the first column, then the second, in their own directions.
    #[test]
    fn composite_keys_order_column_by_column() {
        let rows = [
            (Value::String("a".into()), Value::Int64(3)),
            (Value::String("a".into()), Value::Int64(1)),
            (Value::String("a\0".into()), Value::Int64(9)),
            (Value::String("ab".into()), Value::Null),
            (Value::Null, Value::Int64(0)),
        ];
        let key = |row: &(Value, Value)| {
            encode_key([(&row.0, Direction::Asc), (&row.1, Direction::Desc)])
        };
        let mut sorted: Vec<&(Value, Value)> = rows.iter().collect();
        sorted.sort_by_key(|row| key(row));
        let expected = [&rows[0], &rows[1], &rows[2], &rows[3], &rows[4]];
        assert_eq!(sorted, expected);
    }

    /// Every value decodes back (floats normalized), and trailing bytes are left alone.
    #[test]
    fn keys_round_trip() {
        let mut generator = Values(42);
        for kind in 0..6 {
            for _ in 0..50 {
                let value = generator.value(kind);
                for direction in [Direction::Asc, Direction::Desc] {
                    let mut bytes =
                        encode_key([(&value, direction), (&Value::Int64(7), Direction::Asc)]);
                    bytes.extend_from_slice(b"rowid");
                    let (decoded, used) = decode_key(&bytes, &[direction, Direction::Asc]).unwrap();
                    let expected = match &value {
                        Value::Float64(x) => Value::Float64(normalize(*x)),
                        other => other.clone(),
                    };
                    match (&decoded[0], &expected) {
                        (Value::Float64(x), Value::Float64(y)) if x.is_nan() => assert!(y.is_nan()),
                        _ => assert_eq!(decoded[0], expected),
                    }
                    assert_eq!(decoded[1], Value::Int64(7));
                    assert_eq!(&bytes[used..], b"rowid");
                }
            }
        }
    }

    /// Damaged keys are rejected instead of misread.
    #[test]
    fn malformed_keys_are_rejected() {
        assert!(decode_key(&[], &[Direction::Asc]).is_err());
        assert!(decode_key(&[0x20, 1, 2], &[Direction::Asc]).is_err());
        assert!(decode_key(&[0x40, b'a', 0x00, 0x07], &[Direction::Asc]).is_err());
        assert!(decode_key(&[0x10, 0x05], &[Direction::Asc]).is_err());
        assert!(decode_key(&[0x77], &[Direction::Asc]).is_err());
    }

    /// The prefix successor bounds exactly the keys with the prefix.
    #[test]
    fn prefix_successor_bounds_prefix_scans() {
        assert_eq!(prefix_successor(&[1, 2, 3]), Some(vec![1, 2, 4]));
        assert_eq!(prefix_successor(&[1, 0xFF, 0xFF]), Some(vec![2]));
        assert_eq!(prefix_successor(&[0xFF]), None);
        assert_eq!(prefix_successor(&[]), None);
        let prefix = encode_key([(&Value::Int64(5), Direction::Asc)]);
        let upper = prefix_successor(&prefix).unwrap();
        let inside = encode_key([
            (&Value::Int64(5), Direction::Asc),
            (&Value::Null, Direction::Asc),
        ]);
        let next = encode_key([(&Value::Int64(6), Direction::Asc)]);
        assert!(prefix <= inside && inside < upper && upper <= next);
    }
}
