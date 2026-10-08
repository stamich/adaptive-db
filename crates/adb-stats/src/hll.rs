//! HyperLogLog distinct-value estimator.
//!
//! 4096 one-byte registers (precision 12): a standard error of about 1.6% in 4 KiB, whatever
//! the number of distinct values. Small cardinalities use linear counting.

/// Register-index bits.
const PRECISION: u32 = 12;
/// Number of registers.
const REGISTERS: usize = 1 << PRECISION;

/// A HyperLogLog sketch over 64-bit hashes.
#[derive(Debug, Clone)]
pub struct HyperLogLog {
    /// Highest observed rank per register.
    registers: Box<[u8; REGISTERS]>,
}

impl Default for HyperLogLog {
    /// An empty sketch.
    fn default() -> Self {
        Self {
            registers: Box::new([0; REGISTERS]),
        }
    }
}

impl HyperLogLog {
    /// Bytes held by one sketch (for working-set accounting).
    pub const BYTES: usize = REGISTERS;

    /// Adds one well-mixed 64-bit hash.
    pub fn add(&mut self, hash: u64) {
        let index = (hash >> (64 - PRECISION)) as usize;
        // The remaining bits with a sentinel, so the rank is at most 64 - PRECISION + 1.
        let rest = (hash << PRECISION) | (1 << (PRECISION - 1));
        let rank = rest.leading_zeros() as u8 + 1;
        if rank > self.registers[index] {
            self.registers[index] = rank;
        }
    }

    /// Estimated number of distinct hashes added.
    pub fn estimate(&self) -> f64 {
        let m = REGISTERS as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self
            .registers
            .iter()
            .map(|rank| 2f64.powi(-i32::from(*rank)))
            .sum();
        let raw = alpha * m * m / sum;
        let zeros = self.registers.iter().filter(|rank| **rank == 0).count();
        if raw <= 2.5 * m && zeros > 0 {
            // Linear counting is more accurate for small cardinalities.
            m * (m / zeros as f64).ln()
        } else {
            raw
        }
    }
}

/// Unit tests of the sketch.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::hash;
    use adb_core::Value;

    /// Estimates stay within 3% from a few hundred to a million distinct values.
    #[test]
    fn estimates_are_accurate() {
        for n in [300u64, 20_000, 1_000_000] {
            let mut sketch = HyperLogLog::default();
            for i in 0..n {
                sketch.add(hash(&Value::Int64(i as i64)));
                sketch.add(hash(&Value::Int64(i as i64))); // duplicates do not count
            }
            let error = (sketch.estimate() - n as f64).abs() / n as f64;
            assert!(error < 0.03, "n = {n}: error {error}");
        }
    }
}
