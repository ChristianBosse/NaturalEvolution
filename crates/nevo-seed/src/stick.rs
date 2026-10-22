//! Real-capacity test for USB sticks and SD cards, in the spirit of the `f3` tool.
//!
//! Fake sticks report a large size but store far less: past the real capacity, writes are lost
//! or wrap around and overwrite earlier data. Everything still looks fine until the data is read
//! back. So the test fills the free space with files of pseudo-random data, then reads it all
//! back and compares.
//!
//! Each 8-byte word's value is derived from a per-test seed and the word's position in the whole
//! test, so data that ends up at the wrong place (wrap-around) or comes from an earlier test is
//! caught, not just lost writes.
//!
//! Everything lives in one folder, [`TEST_DIR`], inside the folder being tested; nothing else is
//! created, read or changed. Progress is saved in `state.json` every [`CHECKPOINT`] bytes, so the
//! test can be stopped at any time (or the program killed) and resumed by running it again.
//!
//! The operating system may answer reads from its memory cache instead of the stick. For a
//! trustworthy result on a stick smaller than the computer's free memory, write first
//! ([`Options::write_only`]), unplug and replug the stick, then run again to read back.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The folder the test writes into, inside the folder being tested.
pub const TEST_DIR: &str = "NEVO-CAPACITY-TEST";
const STATE: &str = "state.json";
const STATE_TMP: &str = "state.json.tmp";
/// Unit of corruption accounting: the classic disk sector.
pub const SECTOR: u64 = 512;
const CHUNK: usize = 1 << 20;
/// How often progress is made durable (the file synced, the state saved).
pub const CHECKPOINT: u64 = 64 << 20;
/// 1 GiB test files, like f3: well under FAT32's 4 GiB file limit, and not too many of them.
pub const DEFAULT_FILE_SIZE: u64 = 1 << 30;
/// Bad ranges kept in the report; beyond that they are only counted.
const MAX_RANGES: usize = 64;

#[derive(Debug, Clone)]
pub struct Options {
    /// Test at most this many bytes instead of all the free space.
    pub limit: Option<u64>,
    pub file_size: u64,
    /// Stop after writing, so the stick can be replugged before reading back.
    pub write_only: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            limit: None,
            file_size: DEFAULT_FILE_SIZE,
            write_only: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Writing,
    Reading,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    /// Known when reading, or when writing with a limit.
    pub total: Option<u64>,
}

/// Where a test stands. Byte offsets count across the test files in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub written: u64,
    /// Writing ended because the stick was full (rather than at the limit).
    pub full: bool,
    pub write_done: bool,
    pub verified: u64,
    pub good: u64,
    pub bad: u64,
    /// Bad areas as (offset, length), the first [`MAX_RANGES`] of them.
    pub bad_ranges: Vec<(u64, u64)>,
}

impl Report {
    /// Written and read back completely.
    pub fn finished(&self) -> bool {
        self.write_done && self.verified == self.written
    }

    /// The capacity to trust: data before the first bad sector read back correctly.
    pub fn usable(&self) -> u64 {
        self.bad_ranges.first().map_or(self.good, |r| r.0)
    }
}

/// Saved progress (`state.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    version: u32,
    seed: u64,
    file_size: u64,
    limit: Option<u64>,
    written: u64,
    full: bool,
    write_done: bool,
    verified: u64,
    good: u64,
    bad: u64,
    bad_ranges: Vec<(u64, u64)>,
}

impl State {
    fn report(&self) -> Report {
        Report {
            written: self.written,
            full: self.full,
            write_done: self.write_done,
            verified: self.verified,
            good: self.good,
            bad: self.bad,
            bad_ranges: self.bad_ranges.clone(),
        }
    }

    fn mark_bad(&mut self, start: u64, len: u64) {
        self.bad += len;
        if let Some(last) = self.bad_ranges.last_mut().filter(|r| r.0 + r.1 == start) {
            last.1 += len;
        } else if self.bad_ranges.len() < MAX_RANGES {
            self.bad_ranges.push((start, len));
        }
    }
}

/// Runs (or resumes) the test in `folder`. Returns early, with progress saved, when `stop` is
/// set.
pub fn run(
    folder: &Path,
    options: &Options,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> io::Result<Report> {
    if !folder.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("not a folder: {}", folder.display()),
        ));
    }
    let dir = folder.join(TEST_DIR);
    let mut state = match load(&dir)? {
        Some(state) => state,
        None => {
            if dir.exists() {
                return Err(io::Error::other(format!(
                    "{} exists but holds no test state; not touching it",
                    dir.display()
                )));
            }
            fs::create_dir(&dir)?;
            let file_size = (options.file_size / SECTOR).max(1) * SECTOR;
            let state = State {
                version: 1,
                seed: new_seed(),
                file_size,
                limit: options.limit.map(|l| l / SECTOR * SECTOR),
                written: 0,
                full: false,
                write_done: false,
                verified: 0,
                good: 0,
                bad: 0,
                bad_ranges: Vec::new(),
            };
            save(&dir, &state)?;
            state
        }
    };
    if !state.write_done {
        write_phase(&dir, &mut state, stop, progress)?;
        if !state.write_done || options.write_only {
            return Ok(state.report());
        }
    }
    read_phase(&dir, &mut state, stop, progress)?;
    Ok(state.report())
}

/// The saved state of a test in `folder`, if there is one.
pub fn status(folder: &Path) -> io::Result<Option<Report>> {
    Ok(load(&folder.join(TEST_DIR))?.map(|s| s.report()))
}

/// Removes the test files from `folder`. Only files the test creates are deleted; if anything
/// else is in [`TEST_DIR`], it stays and the folder is kept.
pub fn clean(folder: &Path) -> io::Result<()> {
    let dir = folder.join(TEST_DIR);
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name == STATE || name == STATE_TMP || is_test_file(&name)) && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    match fs::remove_dir(&dir) {
        Ok(()) => Ok(()),
        Err(_) => Err(io::Error::other(format!(
            "{} holds other files; left in place",
            dir.display()
        ))),
    }
}

fn test_file(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index:06}.fill"))
}

fn is_test_file(name: &str) -> bool {
    name.len() == 11 && name.ends_with(".fill") && name[..6].bytes().all(|b| b.is_ascii_digit())
}

fn is_full(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::StorageFull || e.kind() == io::ErrorKind::FileTooLarge
}

fn write_phase(dir: &Path, state: &mut State, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    let end = state.limit.unwrap_or(u64::MAX);
    while state.written < end {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let index = state.written / state.file_size;
        let file_start = index * state.file_size;
        let path = test_file(dir, index);
        let mut file = match OpenOptions::new().create(true).write(true).truncate(false).open(&path) {
            Ok(f) => f,
            Err(e) if is_full(&e) => return finish_full(dir, state, None),
            Err(e) => return Err(e),
        };
        // Anything past the last checkpoint wasn't confirmed; write it again.
        file.set_len(state.written - file_start)?;
        file.seek(SeekFrom::End(0))?;
        let mut synced = state.written;
        let file_end = (file_start + state.file_size).min(end);
        while state.written < file_end {
            let n = (file_end - state.written).min(CHUNK as u64) as usize;
            fill(&mut buf[..n], state.seed, state.written);
            match file.write_all(&buf[..n]) {
                Ok(()) => state.written += n as u64,
                Err(e) if is_full(&e) => return finish_full(dir, state, Some((file, file_start))),
                Err(e) => return Err(e),
            }
            progress(Progress {
                phase: Phase::Writing,
                done: state.written,
                total: state.limit,
            });
            let stopping = stop.load(Ordering::Relaxed);
            if state.written - synced >= CHECKPOINT || stopping || state.written == file_end {
                match file.sync_all() {
                    Ok(()) => {}
                    // Some systems only notice the lack of space when flushing.
                    Err(e) if is_full(&e) => {
                        state.written = synced;
                        return finish_full(dir, state, Some((file, file_start)));
                    }
                    Err(e) => return Err(e),
                }
                synced = state.written;
                save(dir, state)?;
            }
            if stopping {
                return Ok(());
            }
        }
    }
    state.write_done = true;
    save(dir, state)
}

/// Ends writing when the stick is full. The current file is cut back to what was written, minus
/// one chunk so there is room to save the state.
fn finish_full(dir: &Path, state: &mut State, file: Option<(File, u64)>) -> io::Result<()> {
    if let Some((file, file_start)) = file {
        state.written = state.written.saturating_sub(CHUNK as u64).max(file_start);
        file.set_len(state.written - file_start)?;
        file.sync_all()?;
    }
    state.full = true;
    state.write_done = true;
    save(dir, state)
}

fn read_phase(dir: &Path, state: &mut State, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    let mut expected = vec![0u8; CHUNK];
    let mut saved = state.verified;
    while state.verified < state.written {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let index = state.verified / state.file_size;
        let file_start = index * state.file_size;
        let file_end = (file_start + state.file_size).min(state.written);
        let mut file = match File::open(test_file(dir, index)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // A lost file is lost data.
                let len = file_end - state.verified;
                state.mark_bad(state.verified, len);
                state.verified = file_end;
                continue;
            }
            Err(e) => return Err(e),
        };
        file.seek(SeekFrom::Start(state.verified - file_start))?;
        while state.verified < file_end && !stop.load(Ordering::Relaxed) {
            let pos = state.verified;
            let n = (file_end - pos).min(CHUNK as u64) as usize;
            let got = match read_full(&mut file, &mut buf[..n]) {
                Ok(got) => got,
                Err(_) => {
                    // An unreadable area counts as bad; carry on after it.
                    file.seek(SeekFrom::Start(pos + n as u64 - file_start))?;
                    0
                }
            };
            fill(&mut expected[..n], state.seed, pos);
            // A short file means the rest is missing; zero-filled, it fails the comparison.
            buf[got..n].fill(0);
            for (i, (seen, want)) in buf[..n]
                .chunks(SECTOR as usize)
                .zip(expected[..n].chunks(SECTOR as usize))
                .enumerate()
            {
                let at = pos + (i as u64) * SECTOR;
                if seen == want && i * (SECTOR as usize) < got {
                    state.good += seen.len() as u64;
                } else {
                    state.mark_bad(at, seen.len() as u64);
                }
            }
            state.verified += n as u64;
            progress(Progress {
                phase: Phase::Reading,
                done: state.verified,
                total: Some(state.written),
            });
            if state.verified - saved >= CHECKPOINT {
                saved = state.verified;
                save(dir, state)?;
            }
        }
    }
    save(dir, state)
}

/// Reads until `buf` is full or the file ends; returns the bytes read.
fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// The test pattern for `buf`, which starts at byte `pos` of the test (a multiple of 8).
fn fill(buf: &mut [u8], seed: u64, pos: u64) {
    for (i, word) in buf.chunks_exact_mut(8).enumerate() {
        word.copy_from_slice(&splitmix64(seed ^ (pos + 8 * i as u64)).to_le_bytes());
    }
}

/// A fast, well-mixed hash of a 64-bit number (SplitMix64's output function).
fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Different for every test, so leftovers of an earlier test never pass as good data.
fn new_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    splitmix64(nanos ^ (u64::from(std::process::id()) << 32))
}

fn load(dir: &Path) -> io::Result<Option<State>> {
    let bytes = match fs::read(dir.join(STATE)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let state: State = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{STATE}: {e}")))?;
    if state.version != 1 || state.file_size == 0 || state.file_size % SECTOR != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{STATE}: unsupported test state"),
        ));
    }
    Ok(Some(state))
}

/// Saves the state atomically, so a crash leaves the old or the new one, never half of each.
fn save(dir: &Path, state: &State) -> io::Result<()> {
    let tmp = dir.join(STATE_TMP);
    let mut file = File::create(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(state).map_err(io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(tmp, dir.join(STATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_depends_on_position_and_seed() {
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        fill(&mut a, 1, 0);
        fill(&mut b, 1, 8);
        assert_eq!(a[8..], b[..8]);
        assert_ne!(a[..8], a[8..]);
        fill(&mut b, 2, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn test_file_names() {
        assert!(is_test_file("000012.fill"));
        assert!(!is_test_file("notes.fill"));
        assert!(!is_test_file("000012.txt"));
    }
}
