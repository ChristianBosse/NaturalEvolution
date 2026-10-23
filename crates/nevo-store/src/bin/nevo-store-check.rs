//! `nevo-store-check <path> [--blobs] [--repair]`: checks a node's store, like fsck.
//!
//! Exit code 0 means no errors, 1 means errors were found (after repair: errors remain), 2 means
//! the check couldn't start.

use std::path::PathBuf;
use std::process::ExitCode;

use nevo_store::{CheckOptions, Store};

const USAGE: &str = "usage: nevo-store-check <store folder> [--blobs] [--repair]

  --blobs   also re-hash every blob file (slow: reads all content)
  --repair  rebuild index tables and move corrupt objects and blobs into quarantine/
            (nothing is deleted)";

fn main() -> ExitCode {
    let mut path = None;
    let mut options = CheckOptions::default();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--blobs") => options.blobs = true,
            Some("--repair") => options.repair = true,
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(flag) if flag.starts_with('-') => {
                eprintln!("unknown option {flag}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    // Opening a store creates it; a typo in the path must not create an empty one.
    if !path.join("objects.sqlite").is_file() {
        eprintln!("{} is not a NEVO store (no objects.sqlite)", path.display());
        return ExitCode::from(2);
    }
    let store = match Store::open(&path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("can't open {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    let mut report = store.check(options);
    println!("{report}");
    if options.repair && !report.repairs.is_empty() {
        // Check again, so the answer describes the store as it is now.
        report = store.check(CheckOptions {
            repair: false,
            ..options
        });
        println!("\nafter repair:\n{report}");
    }
    if report.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
