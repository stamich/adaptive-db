//! Tests of the storage record on canned `mountinfo` and `df` text.
use adb_bench::storage::{mount_of, parse_df_free_bytes};
use adb_bench::StorageInfo;

/// A trimmed `/proc/self/mountinfo`.
const MOUNTINFO: &str = "\
24 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
30 24 0:25 / /tmp rw,nosuid,nodev shared:5 - tmpfs tmpfs rw,size=65536k
41 24 259:5 / /data rw,relatime shared:9 - xfs /dev/nvme1n1p1 rw
42 41 259:6 / /data/bench\\040disk rw,relatime shared:10 - btrfs /dev/nvme2n1 rw
43 24 0:31 / /tmpfiles rw master:11 optional:x - ext4 /dev/sda1 rw";

/// Output of `df -Pk /data`.
const DF: &str = "\
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/nvme1n1p1   976762584 100000000 876762584      11% /data
";

#[test]
fn the_longest_mount_point_wins() {
    let m = mount_of("/data/work/adb", MOUNTINFO).unwrap();
    assert_eq!(
        (m.mount_point.as_str(), m.filesystem.as_str()),
        ("/data", "xfs")
    );
    let root = mount_of("/home/user", MOUNTINFO).unwrap();
    assert_eq!(
        (root.mount_point.as_str(), root.filesystem.as_str()),
        ("/", "ext4")
    );
}

#[test]
fn a_mount_point_matches_whole_components_only() {
    // /tmpfiles is a different mount from /tmp, and /tmpx belongs to neither.
    assert_eq!(mount_of("/tmp/x", MOUNTINFO).unwrap().filesystem, "tmpfs");
    assert_eq!(
        mount_of("/tmpfiles/x", MOUNTINFO).unwrap().device,
        "/dev/sda1"
    );
    assert_eq!(mount_of("/tmpx/y", MOUNTINFO).unwrap().mount_point, "/");
}

#[test]
fn octal_escapes_in_mount_points_are_undone() {
    let m = mount_of("/data/bench disk/run", MOUNTINFO).unwrap();
    assert_eq!(m.mount_point, "/data/bench disk");
    assert_eq!(m.filesystem, "btrfs");
}

#[test]
fn non_ascii_mount_points_do_not_panic() {
    let line = "50 24 0:40 / /dane/zażółć rw - ext4 /dev/sdb1 rw\\\\";
    let m = mount_of("/dane/zażółć/x", line).unwrap();
    assert_eq!(m.mount_point, "/dane/zażółć");
    // A lone backslash at the end of a field is kept as it is.
    let odd = "51 24 0:41 / /odd\\ rw - ext4 /dev/sdc1 rw";
    assert_eq!(mount_of("/odd\\/x", odd).unwrap().mount_point, "/odd\\");
}

#[test]
fn optional_fields_before_the_hyphen_are_skipped() {
    let m = mount_of("/tmpfiles", MOUNTINFO).unwrap();
    assert_eq!(
        m.filesystem, "ext4",
        "two optional fields precede the hyphen on that line"
    );
}

#[test]
fn free_space_comes_from_the_df_available_column() {
    assert_eq!(parse_df_free_bytes(DF), Some(876_762_584 * 1024));
    assert_eq!(parse_df_free_bytes(""), None);
    assert_eq!(parse_df_free_bytes("header only\n"), None);
    assert_eq!(parse_df_free_bytes("h\nshort line\n"), None);
}

#[test]
fn a_ram_disk_is_flagged() {
    let tmp = StorageInfo::from_texts("/tmp/adb-bench", MOUNTINFO, "");
    assert!(tmp.is_memory_backed());
    assert_eq!(tmp.to_json()["memory_backed"], true);
    let data = StorageInfo::from_texts("/data/x", MOUNTINFO, DF);
    assert!(!data.is_memory_backed());
    assert_eq!(data.free_bytes, Some(876_762_584 * 1024));
    assert_eq!(data.to_json()["device"], "/dev/nvme1n1p1");
}

#[test]
fn unknown_storage_is_null_not_an_error() {
    let info = StorageInfo::from_texts("/somewhere", "", "");
    assert!(info.mount.is_none() && !info.is_memory_backed());
    let json = info.to_json();
    assert!(json["filesystem"].is_null() && json["free_bytes"].is_null());
}

#[test]
fn the_live_machine_can_be_inspected() {
    let dir = tempfile::tempdir().unwrap();
    let info = StorageInfo::capture(dir.path());
    assert!(!info.path.is_empty());
    let _ = info.to_json();
}
