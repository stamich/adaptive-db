//! On-disk layout of the change log: segment files of CRC-framed envelopes.
//!
//! A segment is named after the sequence number of its first envelope (`{seq:020}.clog`), starts
//! with a 12-byte header (magic and format version) and continues with frames
//! `length: u32 | crc32: u32 | payload`. Frames are appended; a segment is never rewritten.
//!
//! A crash can leave one torn frame at the end of the *last* segment; [`scan_segment`] reports
//! the length of the intact prefix so the writer can cut it off. The same damage anywhere else
//! is corruption.
use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use adb_core::CommitSeq;

use crate::{error::ChangeLogError, model::MAX_ENVELOPE_BYTES};

/// Magic bytes at the start of every segment.
pub(crate) const MAGIC: &[u8; 8] = b"ADBCLOG1";
/// Format version stored after the magic.
pub(crate) const FORMAT_VERSION: u32 = 1;
/// Size of the segment header.
pub(crate) const HEADER_LEN: u64 = 12;
/// Size of a frame header (length and checksum).
pub(crate) const FRAME_HEADER_LEN: u64 = 8;
/// File extension of segments.
pub(crate) const EXTENSION: &str = "clog";

/// The header bytes of a new segment.
pub(crate) fn header_bytes() -> [u8; HEADER_LEN as usize] {
    let mut bytes = [0u8; HEADER_LEN as usize];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes
}

/// Name of the segment whose first envelope has sequence number `first`.
pub(crate) fn segment_name(first: CommitSeq) -> String {
    format!("{:020}.{EXTENSION}", first.0)
}

/// Parses a segment file name back into its first sequence number.
pub(crate) fn parse_segment_name(name: &str) -> Option<CommitSeq> {
    let stem = name.strip_suffix(&format!(".{EXTENSION}"))?;
    (stem.len() == 20 && stem.bytes().all(|b| b.is_ascii_digit()))
        .then(|| stem.parse().ok().map(CommitSeq))
        .flatten()
}

/// Frames a payload: length, checksum, payload.
pub(crate) fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + FRAME_HEADER_LEN as usize);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// What a segment contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentScan {
    /// Sequence number of the first frame, if any.
    pub first_seq: Option<CommitSeq>,
    /// Sequence number of the last intact frame, if any.
    pub last_seq: Option<CommitSeq>,
    /// Number of intact frames.
    pub frames: u64,
    /// Length in bytes of the intact prefix (header included).
    pub valid_len: u64,
    /// Length of the file on disk; larger than `valid_len` when a torn tail was found.
    pub file_len: u64,
}

/// Outcome of reading one frame.
pub(crate) enum FrameRead {
    /// An intact frame: its payload.
    Frame(Vec<u8>),
    /// Clean end of file at a frame boundary.
    End,
    /// The frame is incomplete or fails its checksum. `reaches_end` tells whether nothing
    /// follows it, i.e. whether it can be a torn write.
    Damaged {
        /// What is wrong with the frame.
        reason: String,
        /// Whether nothing follows the frame.
        reaches_end: bool,
    },
}

/// Reads one frame from `reader`, which is positioned at `offset` of a file of `file_len` bytes.
pub(crate) fn read_frame(
    reader: &mut impl Read,
    offset: u64,
    file_len: u64,
) -> Result<FrameRead, ChangeLogError> {
    let remaining = file_len.saturating_sub(offset);
    if remaining == 0 {
        return Ok(FrameRead::End);
    }
    if remaining < FRAME_HEADER_LEN {
        return Ok(FrameRead::Damaged {
            reason: "incomplete frame header".into(),
            reaches_end: true,
        });
    }
    let mut header = [0u8; FRAME_HEADER_LEN as usize];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header[..4].try_into().expect("4 bytes")) as u64;
    let crc = u32::from_le_bytes(header[4..].try_into().expect("4 bytes"));
    if length > MAX_ENVELOPE_BYTES {
        return Ok(FrameRead::Damaged {
            reason: format!("frame length {length} exceeds the limit"),
            reaches_end: false,
        });
    }
    if remaining - FRAME_HEADER_LEN < length {
        return Ok(FrameRead::Damaged {
            reason: "incomplete frame payload".into(),
            reaches_end: true,
        });
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    if crc32fast::hash(&payload) != crc {
        let reaches_end = remaining == FRAME_HEADER_LEN + length;
        return Ok(FrameRead::Damaged {
            reason: "checksum mismatch".into(),
            reaches_end,
        });
    }
    Ok(FrameRead::Frame(payload))
}

/// The sequence number at the start of a payload (see the model module).
pub(crate) fn payload_seq(payload: &[u8]) -> Option<CommitSeq> {
    payload
        .get(..8)
        .map(|bytes| CommitSeq(u64::from_le_bytes(bytes.try_into().expect("8 bytes"))))
}

/// Validates the header and every frame of the segment at `path`.
///
/// `named_first` is the sequence number from the file name; the first frame must match it.
/// With `is_last` set, a torn tail is tolerated and reported through `valid_len < file_len`;
/// otherwise any damage is [`ChangeLogError::Corrupt`].
pub(crate) fn scan_segment(
    path: &Path,
    named_first: CommitSeq,
    is_last: bool,
) -> Result<SegmentScan, ChangeLogError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let corrupt = |offset: u64, reason: String| ChangeLogError::Corrupt {
        segment: name.clone(),
        offset,
        reason,
    };
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    if file_len < HEADER_LEN {
        if is_last {
            // A crash while creating the segment: nothing was ever committed to it.
            return Ok(SegmentScan {
                first_seq: None,
                last_seq: None,
                frames: 0,
                valid_len: 0,
                file_len,
            });
        }
        return Err(corrupt(0, "segment shorter than its header".into()));
    }
    let mut header = [0u8; HEADER_LEN as usize];
    reader.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(corrupt(0, "bad magic".into()));
    }
    let version = u32::from_le_bytes(header[8..].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(corrupt(8, format!("unsupported format version {version}")));
    }
    let mut scan = SegmentScan {
        first_seq: None,
        last_seq: None,
        frames: 0,
        valid_len: HEADER_LEN,
        file_len,
    };
    loop {
        match read_frame(&mut reader, scan.valid_len, file_len)? {
            FrameRead::End => return Ok(scan),
            FrameRead::Damaged {
                reason,
                reaches_end,
            } => {
                if is_last && reaches_end {
                    return Ok(scan);
                }
                return Err(corrupt(scan.valid_len, reason));
            }
            FrameRead::Frame(payload) => {
                let seq = payload_seq(&payload)
                    .ok_or_else(|| corrupt(scan.valid_len, "payload without sequence".into()))?;
                match scan.last_seq {
                    None if seq != named_first => {
                        return Err(corrupt(
                            scan.valid_len,
                            format!("first sequence {} differs from the file name", seq.0),
                        ));
                    }
                    Some(previous) if seq <= previous => {
                        return Err(corrupt(
                            scan.valid_len,
                            format!("sequence {} does not follow {}", seq.0, previous.0),
                        ));
                    }
                    _ => {}
                }
                scan.first_seq.get_or_insert(seq);
                scan.last_seq = Some(seq);
                scan.frames += 1;
                scan.valid_len += FRAME_HEADER_LEN + payload.len() as u64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names round-trip and foreign files are ignored.
    #[test]
    fn segment_names() {
        let name = segment_name(CommitSeq(42));
        assert_eq!(name, "00000000000000000042.clog");
        assert_eq!(parse_segment_name(&name), Some(CommitSeq(42)));
        assert_eq!(parse_segment_name("42.clog"), None);
        assert_eq!(parse_segment_name("00000000000000000042.tmp"), None);
        assert_eq!(parse_segment_name("0000000000000000004x.clog"), None);
    }

    /// A framed payload reads back; a flipped payload byte is a checksum mismatch.
    #[test]
    fn frames_are_checksummed() {
        let framed = frame(b"hello world, 8+ bytes");
        let len = framed.len() as u64;
        match read_frame(&mut &framed[..], 0, len).unwrap() {
            FrameRead::Frame(payload) => assert_eq!(payload, b"hello world, 8+ bytes"),
            _ => panic!("expected a frame"),
        }
        let mut bad = framed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(matches!(
            read_frame(&mut &bad[..], 0, len).unwrap(),
            FrameRead::Damaged {
                reaches_end: true,
                ..
            }
        ));
    }
}
