//! Sequential readers over the canonical log.
//!
//! [`WalCursor`] reads forward from any record boundary up to a caller-supplied limit (normally
//! the durable end of the log). Recovery uses it from the checkpoint position; change-data
//! capture uses it from a consumer cursor. Both therefore cost O(new log), not O(whole log).

use std::{fs::File, path::PathBuf};

use adb_core::Lsn;

use crate::{
    error::WalError,
    record::WalRecord,
    segmented::{
        list_segments, lsn_offset, lsn_segment, make_lsn, read_frame, segment_path, Frame,
    },
};

/// One record read from the log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Position of the record.
    pub lsn: Lsn,
    /// Position right after the record (where the next one starts).
    pub next: Lsn,
    /// The record.
    pub record: WalRecord,
}

/// Forward cursor over the segmented log.
pub struct WalCursor {
    dir: PathBuf,
    segment_id: u64,
    offset: u64,
    file: Option<(File, u64)>,
}

impl WalCursor {
    /// Positions a cursor at `from`, which must be a record boundary.
    ///
    /// Fails with [`WalError::Truncated`] when `from` precedes the oldest retained segment.
    pub fn open(dir: impl Into<PathBuf>, from: Lsn) -> Result<Self, WalError> {
        let dir = dir.into();
        let segments = list_segments(&dir)?;
        let segment_id = lsn_segment(from);
        if let Some(first) = segments.first() {
            if segment_id < *first {
                return Err(WalError::Truncated {
                    requested: from,
                    earliest: make_lsn(*first, 0)?,
                });
            }
        } else if from.0 != 0 {
            return Err(WalError::Corrupt(format!(
                "log is empty but position {from:?} was requested"
            )));
        }
        Ok(Self {
            dir,
            segment_id,
            offset: lsn_offset(from),
            file: None,
        })
    }

    /// Current position.
    pub fn position(&self) -> Result<Lsn, WalError> {
        make_lsn(self.segment_id, self.offset)
    }

    /// Next record strictly before `until`, or `None` at the limit or end of the log.
    pub fn next_before(&mut self, until: Lsn) -> Result<Option<LogEntry>, WalError> {
        loop {
            let lsn = self.position()?;
            if lsn >= until {
                return Ok(None);
            }
            if self.file.is_none() {
                let path = segment_path(&self.dir, self.segment_id);
                if !path.exists() {
                    return Ok(None);
                }
                let file = File::open(&path)?;
                let len = file.metadata()?.len();
                self.file = Some((file, len));
            }
            let Some((file, len)) = self.file.as_mut() else {
                return Ok(None);
            };
            match read_frame(file, self.segment_id, self.offset, *len)? {
                Frame::Record(record, frame_len) => {
                    self.offset += frame_len;
                    return Ok(Some(LogEntry {
                        lsn,
                        next: self.position()?,
                        record,
                    }));
                }
                Frame::End | Frame::TruncatedTail => {
                    // The writer may have appended since the length was cached.
                    let current_len = file.metadata()?.len();
                    if current_len > *len {
                        *len = current_len;
                        continue;
                    }
                    let rotated = segment_path(&self.dir, self.segment_id + 1).exists();
                    if !rotated {
                        return Ok(None);
                    }
                    if self.offset != *len {
                        return Err(WalError::Corrupt(format!(
                            "truncated non-tail WAL segment {} at {}",
                            self.segment_id, self.offset
                        )));
                    }
                    self.segment_id += 1;
                    self.offset = 0;
                    self.file = None;
                }
            }
        }
    }
}

/// Reads every complete record of the log (tests and diagnostics).
pub fn read_all(dir: impl Into<PathBuf>) -> Result<Vec<LogEntry>, WalError> {
    let dir = dir.into();
    let start = match list_segments(&dir)?.first() {
        Some(first) => make_lsn(*first, 0)?,
        None => return Ok(Vec::new()),
    };
    let mut cursor = WalCursor::open(dir, start)?;
    let mut out = Vec::new();
    while let Some(entry) = cursor.next_before(Lsn(u64::MAX))? {
        out.push(entry);
    }
    Ok(out)
}
