//! Atomic multi-file write journal ("doublewrite") used to publish checkpoints.
//!
//! A checkpoint must move several files (heap pages, B+Tree pages, B+Tree root metadata and
//! the checkpoint record itself) from one consistent state to the next. Writing them in place
//! one by one is not atomic: a crash in the middle leaves torn pages or a tree whose root no
//! longer matches its pages.
//!
//! [`Journal::commit`] therefore follows the classic redo-journal protocol:
//!
//! 1. serialize every write into `checkpoint.journal.tmp`, `fsync`, rename to
//!    `checkpoint.journal`, `fsync` the directory — **this rename is the commit point**;
//! 2. apply the writes in place and `fsync` every touched file;
//! 3. delete the journal.
//!
//! If the process dies after step 1, [`Journal::recover`] re-applies the journal on the next
//! open (writes are idempotent page images and whole-file replacements). If it dies before
//! step 1, the target files were never touched. Either way the files end up in exactly one of
//! the two consistent states.

mod codec;
pub mod envelope;

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// File name of a committed journal inside the journal root.
pub const JOURNAL_FILE: &str = "checkpoint.journal";
/// File name of a journal that is still being written (never applied).
const JOURNAL_TMP_FILE: &str = "checkpoint.journal.tmp";

/// Errors raised while writing, reading or applying a journal.
#[derive(Debug, Error)]
pub enum JournalError {
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The committed journal is malformed (checksum or framing mismatch).
    #[error("corrupt journal: {0}")]
    Corrupt(String),
    /// A write targets a file outside the journal root.
    #[error("journal write target {0:?} is outside the journal root")]
    OutsideRoot(PathBuf),
}

/// What to do with one target file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Write `bytes` at `offset`, extending the file if needed.
    Range {
        /// Byte offset inside the target file.
        offset: u64,
        /// Bytes to write.
        bytes: Vec<u8>,
    },
    /// Atomically replace the whole file content.
    Replace {
        /// New file content.
        bytes: Vec<u8>,
    },
}

/// One write against one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    /// Target file; must live under the journal root.
    pub path: PathBuf,
    /// Operation to perform.
    pub op: WriteOp,
}

impl FileWrite {
    /// Convenience constructor for an in-place range write.
    pub fn range(path: impl Into<PathBuf>, offset: u64, bytes: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            op: WriteOp::Range { offset, bytes },
        }
    }

    /// Convenience constructor for a whole-file replacement.
    pub fn replace(path: impl Into<PathBuf>, bytes: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            op: WriteOp::Replace { bytes },
        }
    }
}

/// Journal bound to one root directory (the database directory).
#[derive(Debug, Clone)]
pub struct Journal {
    root: PathBuf,
}

impl Journal {
    /// Creates a journal whose files live directly in `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Durably commits `writes` and applies them in place.
    ///
    /// On success every write is applied and durable and no journal remains on disk.
    pub fn commit(&self, writes: &[FileWrite]) -> Result<(), JournalError> {
        if writes.is_empty() {
            return Ok(());
        }
        let relative = writes
            .iter()
            .map(|write| self.relative(write))
            .collect::<Result<Vec<_>, _>>()?;
        self.persist(&codec::encode(&relative))?;
        self.apply_and_clear(&relative)
    }

    /// Applies a journal left behind by a crash. Returns `true` when one was applied.
    ///
    /// A half-written `.tmp` journal is discarded: its commit point was never reached.
    pub fn recover(&self) -> Result<bool, JournalError> {
        let tmp = self.root.join(JOURNAL_TMP_FILE);
        if tmp.exists() {
            fs::remove_file(&tmp)?;
            sync_dir(&self.root)?;
        }
        let path = self.root.join(JOURNAL_FILE);
        if !path.exists() {
            return Ok(false);
        }
        let writes = codec::decode(&fs::read(&path)?)?;
        self.apply_and_clear(&writes)?;
        Ok(true)
    }

    /// Writes the encoded journal and publishes it with an atomic rename (the commit point).
    fn persist(&self, encoded: &[u8]) -> Result<(), JournalError> {
        let tmp = self.root.join(JOURNAL_TMP_FILE);
        {
            let mut file = File::create(&tmp)?;
            file.write_all(encoded)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, self.root.join(JOURNAL_FILE))?;
        sync_dir(&self.root)
    }

    /// Applies relative-path writes, syncs every touched file, then removes the journal.
    fn apply_and_clear(&self, writes: &[FileWrite]) -> Result<(), JournalError> {
        let mut touched_files = BTreeSet::new();
        let mut touched_dirs = BTreeSet::new();
        for write in writes {
            let target = self.root.join(&write.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
                touched_dirs.insert(parent.to_path_buf());
            }
            match &write.op {
                WriteOp::Range { offset, bytes } => {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .truncate(false)
                        .write(true)
                        .open(&target)?;
                    file.seek(SeekFrom::Start(*offset))?;
                    file.write_all(bytes)?;
                    touched_files.insert(target);
                }
                WriteOp::Replace { bytes } => replace_file(&target, bytes)?,
            }
        }
        for path in touched_files {
            OpenOptions::new().write(true).open(&path)?.sync_all()?;
        }
        for dir in touched_dirs {
            sync_dir(&dir)?;
        }
        fs::remove_file(self.root.join(JOURNAL_FILE))?;
        sync_dir(&self.root)
    }

    /// Converts an absolute or relative target path into a root-relative one.
    fn relative(&self, write: &FileWrite) -> Result<FileWrite, JournalError> {
        let path = if write.path.is_absolute() {
            write
                .path
                .strip_prefix(&self.root)
                .map_err(|_| JournalError::OutsideRoot(write.path.clone()))?
                .to_path_buf()
        } else {
            write.path.clone()
        };
        if path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(JournalError::OutsideRoot(write.path.clone()));
        }
        Ok(FileWrite {
            path,
            op: write.op.clone(),
        })
    }
}

/// Replaces a file atomically: temp file, fsync, rename.
pub fn replace_file(target: &Path, bytes: &[u8]) -> Result<(), JournalError> {
    let mut tmp_name = target.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, target)?;
    Ok(())
}

/// Makes directory-entry changes (create, rename, remove) durable.
pub fn sync_dir(dir: &Path) -> Result<(), JournalError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}
