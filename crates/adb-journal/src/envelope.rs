//! Checksummed framing for small metadata files (B+Tree roots, checkpoints, CDC offsets).
//!
//! ```text
//! magic[8] | u16 version | u32 crc32(payload) | u32 payload_len | payload
//! ```

use crc32fast::Hasher;

/// Bytes before the payload.
const HEADER_LEN: usize = 18;

/// Frames `payload` with `magic`, `version`, length and CRC.
pub fn seal(magic: &[u8; 8], version: u16, payload: &[u8]) -> Vec<u8> {
    let mut hasher = Hasher::new();
    hasher.update(payload);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&hasher.finalize().to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Validates a framed file and returns its payload, or a human-readable reason.
pub fn open<'a>(
    magic: &[u8; 8],
    version: u16,
    bytes: &'a [u8],
    max_len: usize,
) -> Result<&'a [u8], String> {
    if bytes.len() > max_len {
        return Err("metadata file too large".into());
    }
    if bytes.len() < HEADER_LEN || &bytes[..8] != magic {
        return Err("unknown metadata format".into());
    }
    let found = u16::from_le_bytes([bytes[8], bytes[9]]);
    if found != version {
        return Err(format!("unsupported metadata version {found}"));
    }
    let crc = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
    let len = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]) as usize;
    if bytes.len() != HEADER_LEN + len {
        return Err("metadata length mismatch".into());
    }
    let payload = &bytes[HEADER_LEN..];
    let mut hasher = Hasher::new();
    hasher.update(payload);
    if hasher.finalize() != crc {
        return Err("metadata checksum mismatch".into());
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let sealed = seal(b"TESTMAGI", 3, b"payload");
        assert_eq!(open(b"TESTMAGI", 3, &sealed, 1024).unwrap(), b"payload");
        assert!(open(b"TESTMAGI", 4, &sealed, 1024).is_err());
        assert!(open(b"OTHERMAG", 3, &sealed, 1024).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(b"TESTMAGI", 3, &tampered, 1024).is_err());
        assert!(open(b"TESTMAGI", 3, &sealed[..sealed.len() - 1], 1024).is_err());
    }
}
