//! Capacity test on a temp folder: small sizes, injected corruption, stop and resume.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use nevo_seed::stick::{self, Options, Phase, TEST_DIR};

const FILE: u64 = 256 * 1024;

fn options(limit: u64) -> Options {
    Options {
        limit: Some(limit),
        file_size: FILE,
        write_only: false,
    }
}

fn run(folder: &Path, options: &Options) -> stick::Report {
    stick::run(folder, options, &AtomicBool::new(false), &mut |_| {}).unwrap()
}

#[test]
fn a_good_folder_passes_and_existing_files_are_untouched() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("notes.txt"), "keep me").unwrap();
    // Not a multiple of the file size, so the last file is short.
    let limit = 4 * FILE + 3 * 512;

    let report = run(dir.path(), &options(limit));
    assert!(report.finished());
    assert!(!report.full);
    assert_eq!(
        (report.written, report.verified, report.good, report.bad),
        (limit, limit, limit, 0)
    );
    assert_eq!(report.usable(), limit);
    assert_eq!(
        fs::metadata(dir.path().join(TEST_DIR).join("000004.fill"))
            .unwrap()
            .len(),
        3 * 512
    );

    stick::clean(dir.path()).unwrap();
    assert!(!dir.path().join(TEST_DIR).exists());
    assert_eq!(fs::read_to_string(dir.path().join("notes.txt")).unwrap(), "keep me");
}

#[test]
fn corruption_and_wrap_around_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let test = dir.path().join(TEST_DIR);
    let limit = 5 * FILE;
    let report = run(
        dir.path(),
        &Options {
            write_only: true,
            ..options(limit)
        },
    );
    assert!(report.write_done && !report.finished());
    assert_eq!(report.verified, 0);

    // A flipped byte in the third file.
    let third = test.join("000002.fill");
    let mut bytes = fs::read(&third).unwrap();
    bytes[1000] ^= 0x01;
    fs::write(&third, bytes).unwrap();
    // A fake stick wrapping around: the fifth file holds the first file's data.
    fs::copy(test.join("000000.fill"), test.join("000004.fill")).unwrap();

    let report = run(dir.path(), &options(limit));
    assert!(report.finished());
    assert_eq!(report.bad_ranges, [(2 * FILE + 512, 512), (4 * FILE, FILE)]);
    assert_eq!(report.bad, 512 + FILE);
    assert_eq!(report.good, limit - report.bad);
    assert_eq!(report.usable(), 2 * FILE + 512);
}

#[test]
fn a_missing_or_short_file_counts_as_lost() {
    let dir = tempfile::tempdir().unwrap();
    let test = dir.path().join(TEST_DIR);
    let limit = 3 * FILE;
    run(
        dir.path(),
        &Options {
            write_only: true,
            ..options(limit)
        },
    );
    fs::remove_file(test.join("000001.fill")).unwrap();
    let last = fs::OpenOptions::new()
        .write(true)
        .open(test.join("000002.fill"))
        .unwrap();
    last.set_len(FILE / 2).unwrap();

    let report = run(dir.path(), &options(limit));
    assert_eq!(report.bad_ranges, [(FILE, FILE), (2 * FILE + FILE / 2, FILE / 2)]);
    assert_eq!(report.good, FILE + FILE / 2);
}

#[test]
fn stops_and_resumes_in_both_phases() {
    let dir = tempfile::tempdir().unwrap();
    let limit = 6 * FILE;
    let opts = options(limit);

    // Stop after the second chunk written.
    let stop = AtomicBool::new(false);
    let report = stick::run(dir.path(), &opts, &stop, &mut |p| {
        if p.phase == Phase::Writing && p.done >= 2 * FILE {
            stop.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(!report.write_done);
    assert_eq!(report.written, 2 * FILE);
    assert_eq!(stick::status(dir.path()).unwrap(), Some(report));

    // Resume, then stop again while reading back.
    let stop = AtomicBool::new(false);
    let report = stick::run(dir.path(), &opts, &stop, &mut |p| {
        if p.phase == Phase::Reading && p.done >= 3 * FILE {
            stop.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(report.write_done && !report.finished());
    assert_eq!(report.written, limit);

    // Different options on resume are ignored: the saved test continues as it started.
    let report = run(dir.path(), &options(FILE));
    assert!(report.finished());
    assert_eq!((report.good, report.bad), (limit, 0));
}

#[test]
fn a_foreign_test_folder_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let test = dir.path().join(TEST_DIR);
    fs::create_dir(&test).unwrap();
    fs::write(test.join("mine.txt"), "x").unwrap();

    assert!(stick::run(dir.path(), &options(FILE), &AtomicBool::new(false), &mut |_| {}).is_err());
    assert!(stick::clean(dir.path()).is_err());
    assert_eq!(fs::read_to_string(test.join("mine.txt")).unwrap(), "x");
}
