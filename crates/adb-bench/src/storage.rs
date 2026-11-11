//! Where the data of a benchmark lives.
//!
//! Durable-write cases are limited by `fsync`, so the same engine on another filesystem, another
//! device or a nearly full disk gives another result. The benchmarks put their data in a temporary
//! directory, which may be a RAM disk (`tmpfs`) on one machine and an NVMe drive on another; this
//! module records which, so a report cannot be read without it.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

/// Filesystem types that live in memory: an `fsync` there measures nothing.
const MEMORY_BACKED: [&str; 3] = ["tmpfs", "ramfs", "devtmpfs"];

/// One line of `/proc/self/mountinfo`, reduced to what a report needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Where the filesystem is mounted.
    pub mount_point: String,
    /// Filesystem type (`ext4`, `xfs`, `btrfs`, `tmpfs`, ...).
    pub filesystem: String,
    /// Backing device or source (`/dev/nvme0n1p2`, `tmpfs`, ...).
    pub device: String,
}

/// The storage under a data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageInfo {
    /// The directory that was inspected.
    pub path: String,
    /// The mount that holds it, when it could be determined.
    pub mount: Option<MountEntry>,
    /// Free space in bytes, when `df` could tell.
    pub free_bytes: Option<u64>,
}

/// Inherent methods of [`StorageInfo`].
impl StorageInfo {
    /// Inspects the live machine for `path` (which must exist for `df` to answer).
    pub fn capture(path: &Path) -> Self {
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let df = Command::new("df")
            .arg("-Pk")
            .arg(path)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        Self::from_texts(&canonical.to_string_lossy(), &mountinfo, &df)
    }

    /// Builds the record from the text of `mountinfo` and of `df -Pk` (so tests need no machine).
    pub fn from_texts(path: &str, mountinfo: &str, df: &str) -> Self {
        Self {
            path: path.to_string(),
            mount: mount_of(path, mountinfo),
            free_bytes: parse_df_free_bytes(df),
        }
    }

    /// Whether the data directory is in memory, which makes durable-write numbers meaningless.
    pub fn is_memory_backed(&self) -> bool {
        self.mount
            .as_ref()
            .is_some_and(|m| MEMORY_BACKED.contains(&m.filesystem.as_str()))
    }

    /// The record as a JSON object; unknown values are `null`.
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "filesystem": self.mount.as_ref().map(|m| m.filesystem.clone()),
            "mount_point": self.mount.as_ref().map(|m| m.mount_point.clone()),
            "device": self.mount.as_ref().map(|m| m.device.clone()),
            "free_bytes": self.free_bytes,
            "memory_backed": self.is_memory_backed(),
        })
    }
}

/// The mount of `mountinfo` that holds `path`: the one with the longest mount point that is a
/// whole-component prefix of it.
pub fn mount_of(path: &str, mountinfo: &str) -> Option<MountEntry> {
    mountinfo
        .lines()
        .filter_map(parse_mountinfo_line)
        .filter(|m| is_prefix_of(&m.mount_point, path))
        .max_by_key(|m| m.mount_point.len())
}

/// Whether `mount` is `path` or one of its parent directories.
fn is_prefix_of(mount: &str, path: &str) -> bool {
    mount == "/"
        || path == mount
        || path
            .strip_prefix(mount)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Parses one `mountinfo` line:
/// `36 35 98:0 /root /mnt rw,noatime master:1 - ext3 /dev/root rw`. The fields before the single
/// hyphen vary in number; the filesystem type and source follow it.
fn parse_mountinfo_line(line: &str) -> Option<MountEntry> {
    let fields: Vec<&str> = line.split(' ').collect();
    let dash = fields.iter().position(|f| *f == "-")?;
    Some(MountEntry {
        mount_point: unescape(fields.get(4)?),
        filesystem: (*fields.get(dash + 1)?).to_string(),
        device: unescape(fields.get(dash + 2)?),
    })
}

/// Undoes the octal escapes of `mountinfo` (a space is `\040`). Works on bytes, so a mount point
/// with non-ASCII characters cannot be cut in the middle of one.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = bytes[i] == b'\\'
            && bytes.len() >= i + 4
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b));
        if escape {
            let value = bytes[i + 1..i + 4]
                .iter()
                .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Free bytes from the output of `df -Pk` (POSIX format, 1024-byte blocks): the fourth column of
/// the second line.
pub fn parse_df_free_bytes(df: &str) -> Option<u64> {
    let line = df.lines().nth(1)?;
    let kib: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    kib.checked_mul(1024)
}
