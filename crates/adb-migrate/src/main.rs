//! `adb-migrate`: offline migration of an Adaptive DB directory from data format 1 (engines up
//! to 2.2.3, raw primary-key bits) to data format 2 (2.3.0, order-preserving primary keys).
//!
//! ```text
//! adb-migrate SOURCE TARGET   migrate SOURCE (left unchanged) into the new directory TARGET
//! adb-migrate SOURCE          migrate in place; the original is kept as SOURCE.format1
//! ```
//!
//! The database must not be open while it is migrated. See `adb_engine::migrate` for the steps
//! and the verification.
use std::{path::PathBuf, process::ExitCode};

use adb_engine::{migrate, migrate_in_place, MigrationReport};

/// Usage text printed for wrong arguments.
const USAGE: &str = "usage: adb-migrate SOURCE [TARGET]
  SOURCE TARGET  migrate SOURCE (unchanged) into the new directory TARGET
  SOURCE         migrate in place; the original is kept as SOURCE.format1";

/// Parses the arguments, runs the migration and prints its report.
fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let outcome = match args.as_slice() {
        [source, target] => migrate(source, target).map(|report| (report, None)),
        [dir] => migrate_in_place(dir).map(|(report, backup)| (report, Some(backup))),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match outcome {
        Ok((report, backup)) => {
            print_report(&report);
            if let Some(backup) = backup {
                println!(
                    "original kept at {} (remove it once you are satisfied)",
                    backup.display()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("adb-migrate: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints what the migration did.
fn print_report(report: &MigrationReport) {
    println!(
        "migrated {} log records in {} segments; {} statistics documents copied",
        report.records, report.segments, report.statistics_documents
    );
    if report.dropped_tail_bytes > 0 {
        println!(
            "dropped an incomplete {}-byte crash tail at the end of the log",
            report.dropped_tail_bytes
        );
    }
    for (entity, rows) in &report.rows_per_entity {
        println!("entity {entity}: {rows} rows");
    }
}
