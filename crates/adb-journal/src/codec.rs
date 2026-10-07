//! Binary framing of a journal file.
//!
//! ```text
//! magic "ADBJRNL1" | u32 entry_count | entries... | u32 crc32(everything after magic)
//! entry: u16 path_len | path (UTF-8) | u8 op | op body
//! op 1 (Range):   u64 offset | u32 len | bytes
//! op 2 (Replace): u32 len | bytes
//! ```
//! All integers are little-endian. Decoding is bounds-checked and never panics.

use std::path::PathBuf;

use crc32fast::Hasher;

use crate::{FileWrite, JournalError, WriteOp};

const MAGIC: &[u8; 8] = b"ADBJRNL1";
const OP_RANGE: u8 = 1;
const OP_REPLACE: u8 = 2;

/// Encodes writes whose paths are already root-relative.
pub(crate) fn encode(writes: &[FileWrite]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(writes.len() as u32).to_le_bytes());
    for write in writes {
        let path = write.path.to_string_lossy();
        body.extend_from_slice(&(path.len() as u16).to_le_bytes());
        body.extend_from_slice(path.as_bytes());
        match &write.op {
            WriteOp::Range { offset, bytes } => {
                body.push(OP_RANGE);
                body.extend_from_slice(&offset.to_le_bytes());
                body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                body.extend_from_slice(bytes);
            }
            WriteOp::Replace { bytes } => {
                body.push(OP_REPLACE);
                body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                body.extend_from_slice(bytes);
            }
        }
    }
    let mut hasher = Hasher::new();
    hasher.update(&body);
    let mut out = Vec::with_capacity(MAGIC.len() + body.len() + 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&body);
    out.extend_from_slice(&hasher.finalize().to_le_bytes());
    out
}

/// Decodes and checksum-verifies a committed journal.
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<FileWrite>, JournalError> {
    if bytes.len() < MAGIC.len() + 8 || &bytes[..MAGIC.len()] != MAGIC {
        return Err(corrupt("bad magic or truncated header"));
    }
    let (body, crc) = bytes[MAGIC.len()..].split_at(bytes.len() - MAGIC.len() - 4);
    let mut hasher = Hasher::new();
    hasher.update(body);
    if hasher.finalize().to_le_bytes() != crc {
        return Err(corrupt("checksum mismatch"));
    }

    let mut reader = Reader { bytes: body, at: 0 };
    let count = reader.u32()? as usize;
    let mut writes = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let path_len = reader.u16()? as usize;
        let path = std::str::from_utf8(reader.take(path_len)?)
            .map_err(|_| corrupt("path is not UTF-8"))?;
        let op = match reader.u8()? {
            OP_RANGE => {
                let offset = reader.u64()?;
                let len = reader.u32()? as usize;
                WriteOp::Range {
                    offset,
                    bytes: reader.take(len)?.to_vec(),
                }
            }
            OP_REPLACE => {
                let len = reader.u32()? as usize;
                WriteOp::Replace {
                    bytes: reader.take(len)?.to_vec(),
                }
            }
            other => return Err(corrupt(&format!("unknown op {other}"))),
        };
        writes.push(FileWrite {
            path: PathBuf::from(path),
            op,
        });
    }
    if reader.at != body.len() {
        return Err(corrupt("trailing bytes after last entry"));
    }
    Ok(writes)
}

fn corrupt(message: &str) -> JournalError {
    JournalError::Corrupt(message.to_string())
}

/// Bounds-checked little-endian cursor.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], JournalError> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| corrupt("entry exceeds journal body"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, JournalError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, JournalError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, JournalError> {
        let mut array = [0u8; 4];
        array.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(array))
    }

    fn u64(&mut self) -> Result<u64, JournalError> {
        let mut array = [0u8; 8];
        array.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(array))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_both_operations() {
        let writes = vec![
            FileWrite::range("a/b.dat", 4096, vec![1, 2, 3]),
            FileWrite::replace("meta", vec![9; 10]),
        ];
        assert_eq!(decode(&encode(&writes)).unwrap(), writes);
    }

    #[test]
    fn rejects_any_flipped_bit() {
        let mut bytes = encode(&[FileWrite::range("x", 0, vec![7; 32])]);
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x01;
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn rejects_truncation() {
        let bytes = encode(&[FileWrite::range("x", 0, vec![7; 32])]);
        assert!(decode(&bytes[..bytes.len() - 5]).is_err());
    }
}
