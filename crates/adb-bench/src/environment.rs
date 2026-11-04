//! The state of the machine around a benchmark.
//!
//! The 2.5.x results taught the lesson this module exists for: the same engine code wrote at 5.1k
//! and at 42.0k rows/s, and the only difference was the CPU governor. A benchmark report must say
//! what the machine was doing.
//!
//! Every value is best effort. A missing `/proc` or `/sys` entry (a VM, another OS) becomes `None`
//! and `null` in the report, never an error, so the harness runs anywhere while a Linux reference
//! machine gets a complete record. All readers take a *root* directory (`/` in production) so tests
//! can build a fake tree.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

/// `scaling_governor` of CPU 0, relative to the root.
const GOVERNOR: &str = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
/// Current frequency of CPU 0 in kHz.
const FREQUENCY: &str = "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq";
/// Maximum frequency of CPU 0 in kHz.
const MAX_FREQUENCY: &str = "sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq";
/// Directory of the thermal zones.
const THERMAL: &str = "sys/class/thermal";
/// Load averages.
const LOADAVG: &str = "proc/loadavg";
/// CPU description.
const CPUINFO: &str = "proc/cpuinfo";
/// Kernel release.
const KERNEL: &str = "proc/sys/kernel/osrelease";

/// The CPU frequency policy of the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernorClass {
    /// The governor is `performance`: the only one whose results can qualify.
    Performance,
    /// Another governor (`powersave`, `schedutil`, ...). Rates measured under it are diagnostic.
    Other(String),
    /// No cpufreq interface (most VMs and CI runners): judged by its measurements alone.
    Unknown,
}

/// Inherent methods of [`GovernorClass`].
impl GovernorClass {
    /// Classifies the text of `scaling_governor` (`None` when it could not be read).
    pub fn from_governor(governor: Option<&str>) -> Self {
        match governor.map(str::trim) {
            None | Some("") => GovernorClass::Unknown,
            Some("performance") => GovernorClass::Performance,
            Some(other) => GovernorClass::Other(other.to_string()),
        }
    }
}

/// Volatile machine state, captured before and after each family.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSnapshot {
    /// Current frequency of CPU 0 in kHz.
    pub cpu_frequency_khz: Option<u64>,
    /// Temperature of the hottest thermal zone in degrees Celsius.
    pub max_thermal_c: Option<f64>,
    /// Load averages over 1, 5 and 15 minutes.
    pub loadavg: Option<[f64; 3]>,
}

/// Inherent methods of [`RuntimeSnapshot`].
impl RuntimeSnapshot {
    /// Reads the live machine.
    pub fn capture() -> Self {
        Self::capture_from(Path::new("/"))
    }

    /// Reads the tree below `root` (a fake one in tests).
    pub fn capture_from(root: &Path) -> Self {
        Self {
            cpu_frequency_khz: read_u64(root, FREQUENCY),
            max_thermal_c: max_thermal_c(root),
            loadavg: loadavg(root),
        }
    }

    /// The snapshot as a JSON object; missing values are `null`.
    pub fn to_json(&self) -> Value {
        json!({
            "cpu_frequency_khz": self.cpu_frequency_khz,
            "max_thermal_c": self.max_thermal_c,
            "loadavg": self.loadavg,
        })
    }
}

/// What does not change during a run: the machine, the operating system and the toolchain.
#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// Operating system (`std::env::consts::OS`).
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Kernel release, when readable.
    pub kernel: Option<String>,
    /// Model name of the CPU.
    pub cpu_model: Option<String>,
    /// Logical CPUs available to the process.
    pub logical_cpus: usize,
    /// Text of `scaling_governor` of CPU 0.
    pub governor: Option<String>,
    /// Maximum frequency of CPU 0 in kHz.
    pub max_frequency_khz: Option<u64>,
    /// `rustc --version` of the build that produced this harness.
    pub rustc: String,
    /// `release` or `debug`: numbers from a debug build are never comparable.
    pub profile: &'static str,
}

/// Inherent methods of [`Environment`].
impl Environment {
    /// Reads the live machine.
    pub fn capture() -> Self {
        Self::capture_from(Path::new("/"))
    }

    /// Reads the tree below `root`.
    pub fn capture_from(root: &Path) -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            kernel: read_trimmed(root, KERNEL),
            cpu_model: cpu_model(root),
            logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
            governor: read_trimmed(root, GOVERNOR),
            max_frequency_khz: read_u64(root, MAX_FREQUENCY),
            rustc: env!("ADB_BENCH_RUSTC").to_string(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        }
    }

    /// The class of the CPU governor.
    pub fn governor_class(&self) -> GovernorClass {
        GovernorClass::from_governor(self.governor.as_deref())
    }

    /// The environment as a JSON object; missing values are `null`.
    pub fn to_json(&self) -> Value {
        json!({
            "os": self.os,
            "arch": self.arch,
            "kernel": self.kernel,
            "cpu_model": self.cpu_model,
            "logical_cpus": self.logical_cpus,
            "governor": self.governor,
            "max_frequency_khz": self.max_frequency_khz,
            "rustc": self.rustc,
            "build": self.profile,
        })
    }
}

/// Reads a file below `root` and trims it; `None` when it is missing or empty.
fn read_trimmed(root: &Path, relative: &str) -> Option<String> {
    let text = fs::read_to_string(root.join(relative)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Reads an unsigned integer from a file below `root`.
fn read_u64(root: &Path, relative: &str) -> Option<u64> {
    read_trimmed(root, relative)?.parse().ok()
}

/// The `model name` line of `/proc/cpuinfo`.
fn cpu_model(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(CPUINFO)).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, name)| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// The hottest thermal zone in degrees Celsius (the kernel reports millidegrees).
fn max_thermal_c(root: &Path) -> Option<f64> {
    let entries = fs::read_dir(root.join(THERMAL)).ok()?;
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone"))
        .filter_map(|e| fs::read_to_string(e.path().join("temp")).ok())
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .map(|milli| milli / 1000.0)
        .max_by(f64::total_cmp)
}

/// The 1, 5 and 15 minute load averages.
fn loadavg(root: &Path) -> Option<[f64; 3]> {
    let text = read_trimmed(root, LOADAVG)?;
    let mut fields = text.split_whitespace().map(str::parse::<f64>);
    Some([
        fields.next()?.ok()?,
        fields.next()?.ok()?,
        fields.next()?.ok()?,
    ])
}
