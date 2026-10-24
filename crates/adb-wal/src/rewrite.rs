//! Offline, position-preserving rewrite of a whole log (used by data format migrations).
//!
//! [`rewrite_log`] reads every segment of a log, passes each record through a mapping and writes
//! it into a segment with the same id in another directory. A mapping must keep the encoded
//! size of every record (for example, changing bits of a fixed-width row id), so every record
//! keeps its position: LSNs stored elsewhere (checkpoints, change-feed cursors, consumer
//! offsets) stay valid. A mapping that changes a record's size fails the rewrite.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

use crc32fast::Hasher;

use crate::{
    error::WalError,
    format::{HEADER_LEN, WAL_MAGIC, WAL_VERSION},
    record::WalRecord,
    segmented::{list_segments, make_lsn, read_frame, segment_path, sync_dir, Frame},
};

/// What [`rewrite_log`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RewriteReport {
    /// Segments written.
    pub segments: u64,
    /// Records written.
    pub records: u64,
    /// Bytes of an incomplete crash tail at the end of the newest segment that were dropped.
    pub dropped_tail_bytes: u64,
}

/// Rewrites the log in `source` into `target` (created, must not contain segments), passing
/// every record through `map`; every record keeps its LSN.
///
/// Fails with [`WalError::InvalidConfiguration`] if `map` changes the encoded size of a record,
/// and with [`WalError::Corrupt`] if the source log is damaged (an incomplete frame is tolerated
/// only at the end of the newest segment, like recovery does).
pub fn rewrite_log(
    source: &Path,
    target: &Path,
    mut map: impl FnMut(WalRecord) -> WalRecord,
) -> Result<RewriteReport, WalError> {
    fs::create_dir_all(target)?;
    if !list_segments(target)?.is_empty() {
        return Err(WalError::InvalidConfiguration(format!(
            "target log {} already holds segments",
            target.display()
        )));
    }
    let segments = list_segments(source)?;
    let mut report = RewriteReport::default();
    for (index, segment_id) in segments.iter().copied().enumerate() {
        let is_last = index + 1 == segments.len();
        let mut input = File::open(segment_path(source, segment_id))?;
        let input_len = input.metadata()?.len();
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(segment_path(target, segment_id))?;
        let mut offset = 0u64;
        loop {
            match read_frame(&mut input, segment_id, offset, input_len)? {
                Frame::Record(record, frame_len) => {
                    let original = (frame_len as usize) - HEADER_LEN;
                    let payload = bincode::serialize(&map(record))?;
                    if payload.len() != original {
                        return Err(WalError::InvalidConfiguration(format!(
                            "rewrite changed the size of the record at {:?} ({original} -> {} bytes)",
                            make_lsn(segment_id, offset)?,
                            payload.len()
                        )));
                    }
                    output.write_all(&frame(&payload))?;
                    offset += frame_len;
                    report.records += 1;
                }
                Frame::End => break,
                Frame::TruncatedTail if is_last => {
                    report.dropped_tail_bytes = input_len - offset;
                    break;
                }
                Frame::TruncatedTail => {
                    return Err(WalError::Corrupt(format!(
                        "truncated non-tail WAL segment {segment_id} at {offset}"
                    )))
                }
            }
        }
        output.sync_all()?;
        report.segments += 1;
    }
    sync_dir(target)?;
    Ok(report)
}

/// One frame around an encoded record: header (magic, version, length, CRC) and payload.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut hasher = Hasher::new();
    hasher.update(payload);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    out.extend_from_slice(&WAL_VERSION.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&hasher.finalize().to_le_bytes());
    out.extend_from_slice(payload);
    out
}
