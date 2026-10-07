//! Segmented canonical log: framing, segment files and the appender.
//!
//! The log is a sequence of segment files `0000000000000000.wal`, `...01.wal`, ... Each holds
//! CRC-protected frames:
//!
//! ```text
//! u32 magic | u16 version | u32 payload_len | u32 crc32(payload) | payload (bincode WalRecord)
//! ```
//!
//! A position (LSN) is `segment << 32 | offset`. Since Milestone 2.0.3 the log is the source of
//! truth and segments are never deleted by the engine.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use adb_core::Lsn;
use crc32fast::Hasher;

use crate::{
    error::WalError,
    format::{HEADER_LEN, MAX_WAL_RECORD_BYTES, WAL_MAGIC, WAL_VERSION},
    record::WalRecord,
};

/// Default maximum physical segment size.
pub const DEFAULT_SEGMENT_SIZE: u64 = 64 * 1024 * 1024;
const OFFSET_BITS: u32 = 32;
const OFFSET_MASK: u64 = (1u64 << OFFSET_BITS) - 1;

/// Packs a segment id and an offset into an LSN.
pub fn make_lsn(segment_id: u64, offset: u64) -> Result<Lsn, WalError> {
    if segment_id > OFFSET_MASK || offset > OFFSET_MASK {
        return Err(WalError::InvalidConfiguration(format!(
            "LSN components out of range: segment={segment_id}, offset={offset}"
        )));
    }
    Ok(Lsn((segment_id << OFFSET_BITS) | offset))
}

/// Segment component of an LSN.
pub fn lsn_segment(lsn: Lsn) -> u64 {
    lsn.0 >> OFFSET_BITS
}

/// Offset component of an LSN.
pub fn lsn_offset(lsn: Lsn) -> u64 {
    lsn.0 & OFFSET_MASK
}

pub(crate) fn segment_path(dir: &Path, segment_id: u64) -> PathBuf {
    dir.join(format!("{segment_id:016x}.wal"))
}

/// Lists segment ids in ascending order and rejects gaps.
pub(crate) fn list_segments(dir: &Path) -> Result<Vec<u64>, WalError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("wal") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|stem| u64::from_str_radix(stem, 16).ok())
        else {
            continue;
        };
        if id > OFFSET_MASK {
            return Err(WalError::Corrupt(format!(
                "segment id {id} exceeds LSN range"
            )));
        }
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();
    if let Some(pair) = ids.windows(2).find(|pair| pair[1] != pair[0] + 1) {
        return Err(WalError::Corrupt(format!(
            "missing WAL segment between {} and {}",
            pair[0], pair[1]
        )));
    }
    Ok(ids)
}

/// Position of the oldest retained record, or `None` for an empty log.
pub fn earliest_lsn(dir: impl AsRef<Path>) -> Result<Option<Lsn>, WalError> {
    match list_segments(dir.as_ref())?.first() {
        Some(first) => Ok(Some(make_lsn(*first, 0)?)),
        None => Ok(None),
    }
}

/// Outcome of reading one frame.
pub(crate) enum Frame {
    /// A complete, verified record and its encoded length.
    Record(WalRecord, u64),
    /// Clean end of the segment.
    End,
    /// An incomplete frame at the end of the segment (crash tail).
    TruncatedTail,
}

/// Reads the frame at `offset` of an open segment of length `file_len`.
pub(crate) fn read_frame(
    file: &mut File,
    segment_id: u64,
    offset: u64,
    file_len: u64,
) -> Result<Frame, WalError> {
    if offset == file_len {
        return Ok(Frame::End);
    }
    let remaining = file_len.saturating_sub(offset);
    if remaining < HEADER_LEN as u64 {
        return Ok(Frame::TruncatedTail);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header)?;
    if u32::from_le_bytes([header[0], header[1], header[2], header[3]]) != WAL_MAGIC {
        return Err(WalError::Corrupt(format!(
            "bad WAL magic in segment {segment_id} at {offset}"
        )));
    }
    if u16::from_le_bytes([header[4], header[5]]) != WAL_VERSION {
        return Err(WalError::Corrupt(format!(
            "unsupported WAL version in segment {segment_id}"
        )));
    }
    let payload_len = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;
    let expected_crc = u32::from_le_bytes([header[10], header[11], header[12], header[13]]);
    if payload_len > MAX_WAL_RECORD_BYTES {
        return Err(WalError::Corrupt(format!(
            "payload length {payload_len} exceeds limit in segment {segment_id}"
        )));
    }
    let frame_len = (HEADER_LEN + payload_len) as u64;
    if frame_len > remaining {
        return Ok(Frame::TruncatedTail);
    }
    let mut payload = vec![0u8; payload_len];
    file.read_exact(&mut payload)?;
    let mut hasher = Hasher::new();
    hasher.update(&payload);
    if hasher.finalize() != expected_crc {
        return Err(WalError::Corrupt(format!(
            "checksum mismatch in segment {segment_id} at {offset}"
        )));
    }
    Ok(Frame::Record(bincode::deserialize(&payload)?, frame_len))
}

/// Returns the offset after the last complete frame of a segment.
///
/// An incomplete frame is tolerated only when `is_last` (the crash tail of the newest segment).
pub(crate) fn valid_end(path: &Path, segment_id: u64, is_last: bool) -> Result<u64, WalError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut offset = 0;
    loop {
        match read_frame(&mut file, segment_id, offset, file_len)? {
            Frame::Record(_, len) => offset += len,
            Frame::End => return Ok(offset),
            Frame::TruncatedTail if is_last => return Ok(offset),
            Frame::TruncatedTail => {
                return Err(WalError::Corrupt(format!(
                    "truncated non-tail WAL segment {segment_id} at {offset}"
                )))
            }
        }
    }
}

/// Appends frames to the newest segment, rotating when it is full.
pub struct SegmentedWalWriter {
    dir: PathBuf,
    segment_size: u64,
    segment_id: u64,
    file: File,
    offset: u64,
}

impl SegmentedWalWriter {
    /// Opens the newest segment and truncates an incomplete crash tail before appending.
    pub fn open(dir: impl AsRef<Path>, segment_size: u64) -> Result<Self, WalError> {
        if segment_size <= HEADER_LEN as u64 || segment_size > OFFSET_MASK {
            return Err(WalError::InvalidConfiguration(format!(
                "segment_size must be in {}..={OFFSET_MASK}, got {segment_size}",
                HEADER_LEN + 1
            )));
        }
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let segment_id = list_segments(&dir)?.last().copied().unwrap_or(0);
        let path = segment_path(&dir, segment_id);
        let created = !path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        if created {
            sync_dir(&dir)?;
        }
        let end = valid_end(&path, segment_id, true)?;
        if end < file.metadata()?.len() {
            file.set_len(end)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::Start(end))?;
        Ok(Self {
            dir,
            segment_size,
            segment_id,
            file,
            offset: end,
        })
    }

    /// Appends one record and returns its position. Not durable until [`sync`](Self::sync).
    pub fn append(&mut self, record: &WalRecord) -> Result<Lsn, WalError> {
        let payload = bincode::serialize(record)?;
        if payload.len() > MAX_WAL_RECORD_BYTES {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        let frame_len = (HEADER_LEN + payload.len()) as u64;
        if frame_len > self.segment_size {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        if self.offset > 0 && self.offset + frame_len > self.segment_size {
            self.rotate()?;
        }
        let lsn = make_lsn(self.segment_id, self.offset)?;
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let mut frame = Vec::with_capacity(frame_len as usize);
        frame.extend_from_slice(&WAL_MAGIC.to_le_bytes());
        frame.extend_from_slice(&WAL_VERSION.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&hasher.finalize().to_le_bytes());
        frame.extend_from_slice(&payload);
        self.file.write_all(&frame)?;
        self.offset += frame_len;
        Ok(lsn)
    }

    /// Makes every appended record durable.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.sync_data()?;
        Ok(())
    }

    /// A handle that can make everything appended so far durable *without* holding the
    /// writer: syncing the returned file covers every record before the returned position.
    /// Earlier segments are already durable because rotation syncs them.
    pub fn sync_point(&self) -> Result<(File, Lsn), WalError> {
        Ok((self.file.try_clone()?, self.position()?))
    }

    /// Position at which the next record will be appended (end of the log).
    pub fn position(&self) -> Result<Lsn, WalError> {
        make_lsn(self.segment_id, self.offset)
    }

    fn rotate(&mut self) -> Result<(), WalError> {
        self.sync()?;
        let next = self.segment_id + 1;
        if next > OFFSET_MASK {
            return Err(WalError::InvalidConfiguration(
                "segment id exceeds LSN range".into(),
            ));
        }
        self.file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(segment_path(&self.dir, next))?;
        sync_dir(&self.dir)?;
        self.segment_id = next;
        self.offset = 0;
        Ok(())
    }
}

fn sync_dir(dir: &Path) -> Result<(), WalError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}
