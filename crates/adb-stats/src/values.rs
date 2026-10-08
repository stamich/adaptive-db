//! Value helpers shared by the collector: ordering, hashing, width and truncation.
use std::{
    cmp::Ordering,
    hash::{DefaultHasher, Hash, Hasher},
};

use adb_core::Value;
use adb_execution::key::KeyPart;

/// Longest string or byte string stored as a bound or common value; longer ones are stored as
/// their prefix (a valid lower bound, an approximate upper bound).
pub const MAX_STORED_VALUE_BYTES: usize = 256;

/// Orders two values of the same type; `None` for different types or NaN.
pub fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Int64(a), Value::Int64(b)) => Some(a.cmp(b)),
        (Value::Float64(a), Value::Float64(b)) => a.partial_cmp(b),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Bytes(a), Value::Bytes(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Discriminant of a value's type, used to detect columns holding several types.
pub fn type_tag(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int64(_) => 2,
        Value::Float64(_) => 3,
        Value::String(_) => 4,
        Value::Bytes(_) => 5,
    }
}

/// 64-bit hash of a value for distinct counting (`-0.0 == 0.0`, every NaN equal).
///
/// The hasher uses fixed keys, so a value hashes the same in every run of the same build.
pub fn hash(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    KeyPart::of(value).hash(&mut hasher);
    hasher.finish()
}

/// Approximate encoded width of a value in bytes.
pub fn width(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int64(_) | Value::Float64(_) => 8,
        Value::String(text) => text.len(),
        Value::Bytes(bytes) => bytes.len(),
    }
}

/// The value as stored in statistics: strings and byte strings longer than
/// [`MAX_STORED_VALUE_BYTES`] are cut to that length (strings on a character boundary).
pub fn stored(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() > MAX_STORED_VALUE_BYTES => {
            let mut end = MAX_STORED_VALUE_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(text[..end].to_string())
        }
        Value::Bytes(bytes) if bytes.len() > MAX_STORED_VALUE_BYTES => {
            Value::Bytes(bytes[..MAX_STORED_VALUE_BYTES].to_vec())
        }
        other => other.clone(),
    }
}

/// Unit tests of the value helpers.
#[cfg(test)]
mod tests {
    use super::*;

    /// Equal numbers hash equally, including the two zeros; truncation respects UTF-8.
    #[test]
    fn hashing_and_truncation() {
        assert_eq!(hash(&Value::Float64(0.0)), hash(&Value::Float64(-0.0)));
        assert_ne!(hash(&Value::Int64(1)), hash(&Value::Int64(2)));
        let long = "ó".repeat(200);
        match stored(&Value::String(long)) {
            Value::String(text) => assert!(text.len() <= MAX_STORED_VALUE_BYTES),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(compare(&Value::Int64(1), &Value::String("a".into())), None);
    }
}
