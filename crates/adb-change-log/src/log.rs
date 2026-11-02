//! The change log store: an append-only, segmented, checksummed sequence of envelopes.
//!
//! [`ChangeLog`] is a cheap, cloneable handle (all clones share one state). Appends are
//! serialised by an internal mutex; readers take a snapshot of the segment table and read the
//! files without holding it, bounded by the length that was *published* when they started, so a
//! reader never sees a half-written frame.
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use adb_core::CommitSeq;
use parking_lot::Mutex;

use crate::{
    error::ChangeLogError,
    model::{decode_envelope, encode_envelope, ChangeEnvelope},
    segment::{
        frame, header_bytes, parse_segment_name, payload_seq, read_frame, scan_segment,
        segment_name, FrameRead, FRAME_HEADER_LEN, HEADER_LEN,
    },
};

/// Default size at which the active segment rolls over (16 MiB).
pub const DEFAULT_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Result of a successful [`ChangeLog::append`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The envelope was written.
    Appended,
    /// An identical envelope with the same sequence number was already stored; nothing changed.
    /// This makes replaying a recovered WAL tail over an existing change log safe.
    Duplicate,
}

/// What the log knows about one segment file.
#[derive(Debug, Clone)]
pub(crate) struct SegmentMeta {
    /// Sequence number of the first envelope (also the file name).
    pub first_seq: CommitSeq,
    /// Sequence number of the last envelope.
    pub last_seq: CommitSeq,
    /// Path of the file.
    pub path: PathBuf,
    /// Published length in bytes: everything below it is intact and complete.
    pub len: u64,
}

/// Mutable state behind the handle.
pub(crate) struct State {
    /// Directory holding the segments.
    pub dir: PathBuf,
    /// Rollover threshold.
    pub segment_bytes: u64,
    /// Segments in sequence order; the last one is active.
    pub segments: Vec<SegmentMeta>,
    /// Append handle of the last segment, opened lazily.
    pub active: Option<File>,
}

/// Handle to a change log directory.
#[derive(Clone)]
pub struct ChangeLog {
    /// Shared state.
    pub(crate) state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for ChangeLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("ChangeLog")
            .field("dir", &state.dir)
            .field("segments", &state.segments.len())
            .finish()
    }
}

impl ChangeLog {
    /// Opens (creating it when needed) the change log in `dir`.
    ///
    /// Every segment is validated. A torn tail of the last segment is cut off; any other damage
    /// is [`ChangeLogError::Corrupt`] and the caller is expected to rebuild the log from the
    /// canonical WAL.
    pub fn open(dir: &Path, segment_bytes: u64) -> Result<Self, ChangeLogError> {
        fs::create_dir_all(dir)?;
        let mut named: Vec<(CommitSeq, PathBuf)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(first) = parse_segment_name(&name) {
                named.push((first, entry.path()));
            }
        }
        named.sort();
        let mut segments = Vec::new();
        let count = named.len();
        for (index, (first, path)) in named.into_iter().enumerate() {
            let is_last = index + 1 == count;
            let scan = scan_segment(&path, first, is_last)?;
            if scan.valid_len < scan.file_len {
                truncate(&path, scan.valid_len)?;
            }
            match scan.last_seq {
                Some(last_seq) => segments.push(SegmentMeta {
                    first_seq: first,
                    last_seq,
                    path,
                    len: scan.valid_len,
                }),
                // An empty or header-less last segment holds nothing: drop it.
                None => fs::remove_file(&path)?,
            }
        }
        for pair in segments.windows(2) {
            if pair[1].first_seq <= pair[0].last_seq {
                return Err(ChangeLogError::Corrupt {
                    segment: pair[1]
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    offset: 0,
                    reason: format!(
                        "starts at {} but the previous segment ends at {}",
                        pair[1].first_seq.0, pair[0].last_seq.0
                    ),
                });
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                dir: dir.to_path_buf(),
                segment_bytes: segment_bytes.max(HEADER_LEN + FRAME_HEADER_LEN + 1),
                segments,
                active: None,
            })),
        })
    }

    /// Sequence number of the newest envelope, `None` for an empty log.
    pub fn head(&self) -> Option<CommitSeq> {
        self.state.lock().segments.last().map(|s| s.last_seq)
    }

    /// Sequence number of the oldest envelope still stored, `None` for an empty log.
    pub fn earliest(&self) -> Option<CommitSeq> {
        self.state.lock().segments.first().map(|s| s.first_seq)
    }

    /// Number of segment files.
    pub fn segment_count(&self) -> usize {
        self.state.lock().segments.len()
    }

    /// Appends `envelope`.
    ///
    /// * A sequence number above the head is written.
    /// * A sequence number at or below the head is accepted only as an exact repeat of the
    ///   stored envelope ([`AppendOutcome::Duplicate`], nothing written); different content is
    ///   [`ChangeLogError::Mismatch`], and a number that was pruned away (or never stored, in a
    ///   gap) is [`ChangeLogError::Regression`].
    ///
    /// The frame is written and flushed to the operating system but **not** fsynced; call
    /// [`ChangeLog::sync`] to make it durable. The envelope becomes visible to readers only
    /// when the write is complete.
    pub fn append(&self, envelope: &ChangeEnvelope) -> Result<AppendOutcome, ChangeLogError> {
        let payload = encode_envelope(envelope)?;
        let framed = frame(&payload);
        let mut state = self.state.lock();
        if let Some(head) = state.segments.last().map(|s| s.last_seq) {
            if envelope.commit_seq <= head {
                let stored = state.read_one(envelope.commit_seq)?;
                return match stored {
                    Some(existing) if existing == payload => Ok(AppendOutcome::Duplicate),
                    Some(_) => Err(ChangeLogError::Mismatch {
                        seq: envelope.commit_seq.0,
                    }),
                    None => Err(ChangeLogError::Regression {
                        head: head.0,
                        got: envelope.commit_seq.0,
                    }),
                };
            }
        }
        state.write_frame(envelope.commit_seq, &framed)?;
        Ok(AppendOutcome::Appended)
    }

    /// Makes everything appended so far durable (fsync of the active segment).
    pub fn sync(&self) -> Result<(), ChangeLogError> {
        let mut state = self.state.lock();
        if let Some(file) = state.active.as_mut() {
            file.sync_data()?;
        }
        Ok(())
    }

    /// Reads up to `max_commits` envelopes with sequence number above `after`, in order.
    pub fn read_after(
        &self,
        after: CommitSeq,
        max_commits: usize,
    ) -> Result<Vec<ChangeEnvelope>, ChangeLogError> {
        Ok(self.read_budgeted(after, max_commits, u64::MAX)?.0)
    }

    /// Like [`ChangeLog::read_after`] but also stops once the encoded size of the returned
    /// envelopes reaches `max_bytes`. At least one envelope is returned when one is available,
    /// so a single oversized commit cannot stall a consumer. The second value is the number of
    /// encoded bytes returned.
    pub(crate) fn read_budgeted(
        &self,
        after: CommitSeq,
        max_commits: usize,
        max_bytes: u64,
    ) -> Result<(Vec<ChangeEnvelope>, u64), ChangeLogError> {
        let snapshot: Vec<SegmentMeta> = {
            let state = self.state.lock();
            state
                .segments
                .iter()
                .filter(|s| s.last_seq > after)
                .cloned()
                .collect()
        };
        let mut out = Vec::new();
        let mut bytes = 0u64;
        for segment in snapshot {
            if out.len() >= max_commits || (!out.is_empty() && bytes >= max_bytes) {
                break;
            }
            read_segment(
                &segment,
                after,
                max_commits,
                max_bytes,
                &mut out,
                &mut bytes,
            )?;
        }
        Ok((out, bytes))
    }
}

impl State {
    /// The encoded payload stored under `seq`, if the log has it.
    fn read_one(&self, seq: CommitSeq) -> Result<Option<Vec<u8>>, ChangeLogError> {
        let Some(segment) = self
            .segments
            .iter()
            .find(|s| s.first_seq <= seq && seq <= s.last_seq)
        else {
            return Ok(None);
        };
        let mut file = File::open(&segment.path)?;
        file.seek(SeekFrom::Start(HEADER_LEN))?;
        let mut reader = BufReader::new(file);
        let mut offset = HEADER_LEN;
        while offset < segment.len {
            match read_frame(&mut reader, offset, segment.len)? {
                FrameRead::Frame(payload) => {
                    offset += FRAME_HEADER_LEN + payload.len() as u64;
                    if payload_seq(&payload) == Some(seq) {
                        return Ok(Some(payload));
                    }
                }
                FrameRead::End => break,
                FrameRead::Damaged { reason, .. } => {
                    return Err(ChangeLogError::Corrupt {
                        segment: segment_label(segment),
                        offset,
                        reason,
                    })
                }
            }
        }
        Ok(None)
    }

    /// Writes one framed envelope, rolling over to a new segment when the active one is full.
    fn write_frame(&mut self, seq: CommitSeq, framed: &[u8]) -> Result<(), ChangeLogError> {
        let needs_new = match self.segments.last() {
            None => true,
            Some(active) => {
                active.len > HEADER_LEN && active.len + framed.len() as u64 > self.segment_bytes
            }
        };
        if needs_new {
            if let Some(file) = self.active.take() {
                file.sync_data()?;
            }
            let path = self.dir.join(segment_name(seq));
            let mut file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(&path)?;
            file.write_all(&header_bytes())?;
            sync_dir(&self.dir)?;
            self.segments.push(SegmentMeta {
                first_seq: seq,
                last_seq: seq,
                path,
                len: HEADER_LEN,
            });
            self.active = Some(file);
        }
        if self.active.is_none() {
            let path = &self.segments.last().expect("a segment exists").path;
            self.active = Some(OpenOptions::new().append(true).open(path)?);
        }
        let file = self.active.as_mut().expect("opened above");
        file.write_all(framed)?;
        file.flush()?;
        let active = self.segments.last_mut().expect("a segment exists");
        active.len += framed.len() as u64;
        active.last_seq = seq;
        Ok(())
    }
}

/// Reads the envelopes of one segment above `after` into `out`, up to `max_commits` in total.
fn read_segment(
    segment: &SegmentMeta,
    after: CommitSeq,
    max_commits: usize,
    max_bytes: u64,
    out: &mut Vec<ChangeEnvelope>,
    bytes: &mut u64,
) -> Result<(), ChangeLogError> {
    let mut file = File::open(&segment.path)?;
    file.seek(SeekFrom::Start(HEADER_LEN))?;
    let mut reader = BufReader::new(file);
    let mut offset = HEADER_LEN;
    while offset < segment.len && out.len() < max_commits && (out.is_empty() || *bytes < max_bytes)
    {
        match read_frame(&mut reader, offset, segment.len)? {
            FrameRead::Frame(payload) => {
                offset += FRAME_HEADER_LEN + payload.len() as u64;
                let seq = payload_seq(&payload).unwrap_or(CommitSeq::ZERO);
                if seq > after {
                    *bytes += payload.len() as u64;
                    out.push(decode_envelope(&payload)?);
                }
            }
            FrameRead::End => break,
            FrameRead::Damaged { reason, .. } => {
                return Err(ChangeLogError::Corrupt {
                    segment: segment_label(segment),
                    offset,
                    reason,
                });
            }
        }
    }
    Ok(())
}

/// File name of a segment, for error messages.
fn segment_label(segment: &SegmentMeta) -> String {
    segment
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Cuts `path` to `len` bytes and makes the cut durable.
fn truncate(path: &Path, len: u64) -> Result<(), ChangeLogError> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(len)?;
    file.sync_all()?;
    Ok(())
}

/// Fsyncs a directory so a created or removed file survives a crash (best effort off Unix).
pub(crate) fn sync_dir(dir: &Path) -> Result<(), ChangeLogError> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}
