//! Segmented WAL with bounded frames, crash-tail normalization, and segment-gap detection.

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
/// Number of low LSN bits reserved for an offset within a WAL segment.
const OFFSET_BITS: u32 = 32;
/// Largest segment id or offset representable by the packed Milestone 1.6 LSN.
const OFFSET_MASK: u64 = (1u64 << OFFSET_BITS) - 1;

/// Packs a segment id and offset into one LSN after validating both components.
pub fn make_lsn(segment_id: u64, offset: u64) -> Result<Lsn, WalError> {
    if segment_id > OFFSET_MASK || offset > OFFSET_MASK {
        return Err(WalError::InvalidConfiguration(format!(
            "LSN components out of range: segment={segment_id}, offset={offset}"
        )));
    }
    Ok(Lsn((segment_id << OFFSET_BITS) | offset))
}

/// Extracts the physical WAL segment id from a packed LSN.
pub fn lsn_segment(lsn: Lsn) -> u64 {
    lsn.0 >> OFFSET_BITS
}

/// Extracts the byte offset within a WAL segment from a packed LSN.
pub fn lsn_offset(lsn: Lsn) -> u64 {
    lsn.0 & OFFSET_MASK
}

/// Returns the canonical fixed-width filename for a segment id.
fn segment_name(segment_id: u64) -> String {
    format!("{segment_id:016x}.wal")
}

/// Returns the filesystem path for one segment id.
fn segment_path(dir: &Path, segment_id: u64) -> PathBuf {
    dir.join(segment_name(segment_id))
}

/// Lists recognized WAL segments in ascending order and validates id range and contiguity.
fn list_segments(dir: &Path) -> Result<Vec<u64>, WalError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("wal") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(id) = u64::from_str_radix(stem, 16) else {
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
    for pair in ids.windows(2) {
        if pair[1] != pair[0] + 1 {
            return Err(WalError::Corrupt(format!(
                "missing WAL segment between {} and {}",
                pair[0], pair[1]
            )));
        }
    }
    Ok(ids)
}

/// Appends records to a sequence of fixed-size WAL segments.
pub struct SegmentedWalWriter {
    dir: PathBuf,
    segment_size: u64,
    segment_id: u64,
    file: File,
    offset: u64,
}

/// Implements safe segmented append, rotation, crash-tail truncation, and retention.
impl SegmentedWalWriter {
    /// Opens the newest segment and truncates an incomplete final crash suffix before appending.
    pub fn open(dir: impl AsRef<Path>, segment_size: u64) -> Result<Self, WalError> {
        if segment_size <= HEADER_LEN as u64 || segment_size > OFFSET_MASK {
            return Err(WalError::InvalidConfiguration(format!(
                "segment_size must be in {}..={OFFSET_MASK}, got {segment_size}",
                HEADER_LEN + 1
            )));
        }

        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let segments = list_segments(&dir)?;
        let segment_id = segments.last().copied().unwrap_or(0);
        let path = segment_path(&dir, segment_id);
        let newly_created = !path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)?;
        if newly_created {
            sync_dir(&dir)?;
        }

        let physical_end = file.seek(SeekFrom::End(0))?;
        let valid_end = read_segment(&path, segment_id, true, None)?;
        if valid_end < physical_end {
            file.set_len(valid_end)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;

        Ok(Self {
            dir,
            segment_size,
            segment_id,
            file,
            offset: valid_end,
        })
    }

    /// Appends one bounded CRC-protected record, rotating first when the record will not fit.
    pub fn append(&mut self, record: &WalRecord) -> Result<Lsn, WalError> {
        let payload = bincode::serialize(record)?;
        if payload.len() > MAX_WAL_RECORD_BYTES {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        let payload_len =
            u32::try_from(payload.len()).map_err(|_| WalError::RecordTooLarge(payload.len()))?;
        let record_len = (HEADER_LEN as u64)
            .checked_add(payload.len() as u64)
            .ok_or_else(|| WalError::Corrupt("WAL record length overflow".into()))?;
        if record_len > self.segment_size {
            return Err(WalError::RecordTooLarge(payload.len()));
        }
        if self.offset > 0
            && self
            .offset
            .checked_add(record_len)
            .map_or(true, |end| end > self.segment_size)
        {
            self.rotate()?;
        }

        let lsn = make_lsn(self.segment_id, self.offset)?;
        let mut hasher = Hasher::new();
        hasher.update(&payload);
        let checksum = hasher.finalize();

        self.file.write_all(&WAL_MAGIC.to_le_bytes())?;
        self.file.write_all(&WAL_VERSION.to_le_bytes())?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.offset = self
            .offset
            .checked_add(record_len)
            .ok_or_else(|| WalError::Corrupt("WAL offset overflow".into()))?;
        Ok(lsn)
    }

    /// Flushes process buffers and synchronizes the active WAL segment.
    pub fn sync(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Returns the packed LSN at which the next record will be appended.
    pub fn position(&self) -> Result<Lsn, WalError> {
        make_lsn(self.segment_id, self.offset)
    }

    /// Deletes complete segments strictly older than the segment containing `retain_lsn`.
    pub fn prune_segments_before(&self, retain_lsn: Lsn) -> Result<usize, WalError> {
        let retain_segment = lsn_segment(retain_lsn);
        let mut removed = 0;
        for segment_id in list_segments(&self.dir)? {
            if segment_id < retain_segment && segment_id < self.segment_id {
                fs::remove_file(segment_path(&self.dir, segment_id))?;
                removed += 1;
            }
        }
        if removed > 0 {
            sync_dir(&self.dir)?;
        }
        Ok(removed)
    }

    /// Synchronizes the active segment and advances to a newly created next segment.
    fn rotate(&mut self) -> Result<(), WalError> {
        self.sync()?;
        let next = self
            .segment_id
            .checked_add(1)
            .ok_or_else(|| WalError::InvalidConfiguration("segment id overflow".into()))?;
        if next > OFFSET_MASK {
            return Err(WalError::InvalidConfiguration(
                "segment id exceeds LSN range".into(),
            ));
        }

        let path = segment_path(&self.dir, next);
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        sync_dir(&self.dir)?;
        self.segment_id = next;
        self.offset = 0;
        self.file = file;
        Ok(())
    }
}

/// Sequential reader across a contiguous set of WAL segments.
pub struct SegmentedWalReader;

/// Implements bounded, checksummed recovery over segmented WAL files.
impl SegmentedWalReader {
    /// Reads all complete records and tolerates an incomplete suffix only in the newest segment.
    pub fn read_all(dir: impl AsRef<Path>) -> Result<Vec<(Lsn, WalRecord)>, WalError> {
        let dir = dir.as_ref();
        let segments = list_segments(dir)?;
        let mut out = Vec::new();
        for (index, segment_id) in segments.iter().copied().enumerate() {
            let is_last = index + 1 == segments.len();
            read_segment(
                &segment_path(dir, segment_id),
                segment_id,
                is_last,
                Some(&mut out),
            )?;
        }
        Ok(out)
    }
}

/// Reads one segment and returns the offset after its last complete valid record.
fn read_segment(
    path: &Path,
    segment_id: u64,
    allow_truncated_tail: bool,
    mut out: Option<&mut Vec<(Lsn, WalRecord)>>,
) -> Result<u64, WalError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut offset = 0u64;
    if file_len > OFFSET_MASK {
        return Err(WalError::Corrupt(format!(
            "segment {segment_id} exceeds LSN offset range"
        )));
    }

    loop {
        if offset == file_len {
            return Ok(offset);
        }
        let remaining = file_len - offset;
        if remaining < HEADER_LEN as u64 {
            return if allow_truncated_tail {
                Ok(offset)
            } else {
                Err(WalError::Corrupt(format!(
                    "truncated non-tail WAL segment {segment_id} at {offset}"
                )))
            };
        }

        file.seek(SeekFrom::Start(offset))?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if u32::from_le_bytes(magic) != WAL_MAGIC {
            return Err(WalError::Corrupt(format!(
                "bad WAL magic in segment {segment_id} at {offset}"
            )));
        }

        let mut version = [0u8; 2];
        file.read_exact(&mut version)?;
        if u16::from_le_bytes(version) != WAL_VERSION {
            return Err(WalError::Corrupt(format!(
                "unsupported WAL version in segment {segment_id}"
            )));
        }

        let mut len = [0u8; 4];
        file.read_exact(&mut len)?;
        let payload_len = u32::from_le_bytes(len) as usize;
        if payload_len > MAX_WAL_RECORD_BYTES {
            return Err(WalError::Corrupt(format!(
                "payload length {payload_len} exceeds hardened limit in segment {segment_id}"
            )));
        }

        let mut crc = [0u8; 4];
        file.read_exact(&mut crc)?;
        let expected_crc = u32::from_le_bytes(crc);
        let frame_len = (HEADER_LEN as u64)
            .checked_add(payload_len as u64)
            .ok_or_else(|| WalError::Corrupt("frame length overflow".into()))?;
        if frame_len > remaining {
            return if allow_truncated_tail {
                Ok(offset)
            } else {
                Err(WalError::Corrupt(format!(
                    "truncated non-tail WAL segment {segment_id} at {offset}"
                )))
            };
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

        let record: WalRecord = bincode::deserialize(&payload)?;
        if let Some(records) = out.as_mut() {
            (**records).push((make_lsn(segment_id, offset)?, record));
        }
        offset = offset
            .checked_add(frame_len)
            .ok_or_else(|| WalError::Corrupt("segment offset overflow".into()))?;
    }
}

/// Synchronizes a directory entry update so created/removed segment names survive power loss.
fn sync_dir(dir: &Path) -> Result<(), WalError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}
