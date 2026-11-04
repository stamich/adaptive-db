//! What every benchmark binary needs to know about where and how it runs.
//!
//! A durable benchmark writes to a directory; which filesystem holds that directory decides what
//! the numbers mean. [`Context`] picks the directory (the system temporary directory unless the
//! caller names one — `--data-dir` or `ADB_BENCH_DATA_DIR`), hands out fresh temporary
//! subdirectories in it, and captures the environment, the storage record and the fsync probe in one
//! place, so every family records them the same way.

use std::env;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::environment::Environment;
use crate::plan::MeasurementPlan;
use crate::probe::FsyncProbe;
use crate::storage::StorageInfo;

/// Environment variable naming the directory durable families write to.
pub const DATA_DIR_ENV: &str = "ADB_BENCH_DATA_DIR";

/// Samples of one fsync probe (a quick run takes fewer).
const PROBE_SAMPLES: usize = 50;

/// Samples of one fsync probe in a quick run.
const PROBE_SAMPLES_QUICK: usize = 10;

/// Where and on what a benchmark runs.
#[derive(Debug, Clone)]
pub struct Context {
    /// The machine, captured once at start.
    pub environment: Environment,
    /// Directory in which temporary data directories are created.
    pub data_root: PathBuf,
    /// Whether the run uses the quick plans.
    pub quick: bool,
}

/// Construction and helpers of [`Context`].
impl Context {
    /// A context for `data_dir`, or — when `None` — for [`DATA_DIR_ENV`] and then the system
    /// temporary directory. The quick flag comes from `ADB_BENCH_QUICK=1`.
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let data_root = data_dir
            .or_else(|| env::var_os(DATA_DIR_ENV).map(PathBuf::from))
            .unwrap_or_else(env::temp_dir);
        let quick = MeasurementPlan::from_env(false).is_quick();
        Self {
            environment: Environment::capture(),
            data_root,
            quick,
        }
    }

    /// A fresh temporary directory below [`Context::data_root`]; it is removed when dropped.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the root does not exist or cannot be written.
    pub fn tempdir(&self) -> io::Result<TempDir> {
        TempDir::new_in(&self.data_root)
    }

    /// The plan for CPU-bound (`durable == false`) or fsync-bound (`true`) cases, honouring
    /// `ADB_BENCH_QUICK` and `ADB_BENCH_MIN_SAMPLE_MS`.
    pub fn plan(&self, durable: bool) -> MeasurementPlan {
        MeasurementPlan::from_env(durable)
    }

    /// The storage record of [`Context::data_root`]; unknown values in it are `None`.
    pub fn storage(&self) -> StorageInfo {
        StorageInfo::capture(&self.data_root)
    }

    /// Runs the fsync probe in `dir`; `None` when the directory cannot be written.
    pub fn probe(&self, dir: &Path) -> Option<FsyncProbe> {
        let samples = if self.quick {
            PROBE_SAMPLES_QUICK
        } else {
            PROBE_SAMPLES
        };
        FsyncProbe::run(dir, samples).ok()
    }
}
