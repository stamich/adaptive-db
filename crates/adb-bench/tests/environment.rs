//! Tests of the environment record on fake `/proc` and `/sys` trees.
use std::fs;
use std::path::Path;

use adb_bench::{Environment, GovernorClass, RuntimeSnapshot};

/// Writes `text` to `root/relative`, creating directories.
fn put(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A fake Ryzen with the `performance` governor.
fn fake_machine(root: &Path, governor: &str) {
    let cpufreq = "sys/devices/system/cpu/cpu0/cpufreq";
    put(
        root,
        &format!("{cpufreq}/scaling_governor"),
        &format!("{governor}\n"),
    );
    put(root, &format!("{cpufreq}/scaling_cur_freq"), "4900000\n");
    put(root, &format!("{cpufreq}/cpuinfo_max_freq"), "5083000\n");
    put(root, "proc/loadavg", "0.42 0.30 0.20 1/900 12345\n");
    put(
        root,
        "proc/cpuinfo",
        "processor\t: 0\nmodel name\t: AMD Ryzen 9 5950X 16-Core Processor\ncpu MHz\t\t: 4900.0\n",
    );
    put(root, "proc/sys/kernel/osrelease", "6.18.0-test\n");
    put(root, "sys/class/thermal/thermal_zone0/temp", "41000\n");
    put(root, "sys/class/thermal/thermal_zone1/temp", "55500\n");
}

#[test]
fn a_complete_machine_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    fake_machine(dir.path(), "performance");
    let env = Environment::capture_from(dir.path());
    assert_eq!(env.governor.as_deref(), Some("performance"));
    assert_eq!(env.governor_class(), GovernorClass::Performance);
    assert_eq!(
        env.cpu_model.as_deref(),
        Some("AMD Ryzen 9 5950X 16-Core Processor")
    );
    assert_eq!(env.kernel.as_deref(), Some("6.18.0-test"));
    assert_eq!(env.max_frequency_khz, Some(5_083_000));
    assert!(env.logical_cpus >= 1);
    assert!(env.rustc.starts_with("rustc") || env.rustc == "unknown");
    let snapshot = RuntimeSnapshot::capture_from(dir.path());
    assert_eq!(snapshot.cpu_frequency_khz, Some(4_900_000));
    assert_eq!(
        snapshot.max_thermal_c,
        Some(55.5),
        "the hottest zone, in degrees"
    );
    assert_eq!(snapshot.loadavg, Some([0.42, 0.30, 0.20]));
}

#[test]
fn a_machine_without_cpufreq_is_unknown_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let env = Environment::capture_from(dir.path());
    assert_eq!(env.governor, None);
    assert_eq!(env.governor_class(), GovernorClass::Unknown);
    assert_eq!(env.cpu_model, None);
    let snapshot = RuntimeSnapshot::capture_from(dir.path());
    assert_eq!(
        snapshot,
        RuntimeSnapshot {
            cpu_frequency_khz: None,
            max_thermal_c: None,
            loadavg: None
        }
    );
    let json = env.to_json();
    assert!(json["governor"].is_null() && json["cpu_model"].is_null());
    assert!(RuntimeSnapshot::capture_from(dir.path()).to_json()["loadavg"].is_null());
}

#[test]
fn governors_are_classified() {
    assert_eq!(
        GovernorClass::from_governor(Some("performance")),
        GovernorClass::Performance
    );
    assert_eq!(
        GovernorClass::from_governor(Some("performance\n")),
        GovernorClass::Performance
    );
    assert_eq!(
        GovernorClass::from_governor(Some("powersave")),
        GovernorClass::Other("powersave".to_string())
    );
    assert_eq!(
        GovernorClass::from_governor(Some("  ")),
        GovernorClass::Unknown
    );
    assert_eq!(GovernorClass::from_governor(None), GovernorClass::Unknown);
}

#[test]
fn garbage_values_become_missing_values() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "proc/loadavg", "not numbers here\n");
    put(
        dir.path(),
        "sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
        "fast\n",
    );
    put(dir.path(), "sys/class/thermal/thermal_zone0/temp", "hot\n");
    let snapshot = RuntimeSnapshot::capture_from(dir.path());
    assert_eq!(snapshot.loadavg, None);
    assert_eq!(snapshot.cpu_frequency_khz, None);
    assert_eq!(snapshot.max_thermal_c, None);
}

#[test]
fn the_live_machine_can_always_be_captured() {
    let env = Environment::capture();
    assert!(!env.os.is_empty() && !env.arch.is_empty());
    assert!(["release", "debug"].contains(&env.profile));
    let _ = RuntimeSnapshot::capture();
}

#[test]
fn json_has_stable_keys() {
    let dir = tempfile::tempdir().unwrap();
    fake_machine(dir.path(), "powersave");
    let json = Environment::capture_from(dir.path()).to_json();
    for key in [
        "os",
        "arch",
        "kernel",
        "cpu_model",
        "logical_cpus",
        "governor",
        "max_frequency_khz",
        "rustc",
        "build",
    ] {
        assert!(json.get(key).is_some(), "missing key {key}");
    }
    assert_eq!(json["governor"], "powersave");
}
