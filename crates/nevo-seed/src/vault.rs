//! Vault verification: re-checking drives kept for long-term storage (D-028).
//!
//! Unpowered flash slowly loses data, so a vault drive carries a manifest of every file with its
//! size and SHA-256, and gets re-hashed about once a year. Each check is appended to a log on the
//! drive, which is how Seed knows when the next one is due.
//!
//! This manifest is deliberately minimal and unsigned; the signed seed manifest will replace or
//! wrap it once the catalog format exists. Files in a vault folder:
//!
//! ```text
//! nevo-vault.json         {"format": "nevo-vault", "version": 1, "created": "<time>",
//!                          "files": [{"path": "packs/a.zim", "size": 123, "sha256": "<hex>"}]}
//! nevo-vault-checks.json  {"checks": [{"at": "<time>", "files": 1, "missing": 0, ...}]}
//! ```
//!
//! Paths use `/` and are relative to the vault folder. Times are UTC, `2026-10-22T13:05:00Z`.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::hash::Sha256;

pub const MANIFEST: &str = "nevo-vault.json";
pub const CHECKS: &str = "nevo-vault-checks.json";
const FORMAT: &str = "nevo-vault";
/// How long after a check the next one is due (D-028: about yearly).
pub const RECHECK_AFTER_DAYS: u64 = 365;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub created: Stamp,
    pub files: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: Sha256,
}

/// One past verification, as kept in the checks log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub at: Stamp,
    pub files: usize,
    pub missing: usize,
    pub corrupt: usize,
    pub extra: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Checks {
    checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Size { expected: u64, actual: u64 },
    Hash { expected: Sha256, actual: Sha256 },
    Unreadable(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Size { expected, actual } => write!(f, "size is {actual} bytes, expected {expected}"),
            Problem::Hash { .. } => write!(f, "content changed (SHA-256 differs)"),
            Problem::Unreadable(e) => write!(f, "can't read: {e}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Files listed in the manifest.
    pub files: usize,
    pub bytes: u64,
    pub missing: Vec<String>,
    pub corrupt: Vec<(String, Problem)>,
    /// Files on the drive the manifest doesn't list. Not damage, but worth knowing.
    pub extra: Vec<String>,
}

impl VerifyReport {
    /// No file is missing or damaged.
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
}

/// Hashes every file in `folder` and writes its manifest. Refuses to replace an existing
/// manifest: doing that over damaged files would make the damage look official.
pub fn create(folder: &Path, now: Stamp, progress: &mut dyn FnMut(&str)) -> io::Result<Manifest> {
    let manifest_path = folder.join(MANIFEST);
    if manifest_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", manifest_path.display()),
        ));
    }
    let mut files = Vec::new();
    for path in list_files(folder)? {
        progress(&path);
        let full = join(folder, &path)?;
        let size = fs::metadata(&full)?.len();
        let sha256 = Sha256::of_file(&full, &mut |_| {})?;
        files.push(Entry { path, size, sha256 });
    }
    let manifest = Manifest {
        format: FORMAT.into(),
        version: 1,
        created: now,
        files,
    };
    write_json(&manifest_path, &manifest)?;
    Ok(manifest)
}

/// Reads the manifest of `folder`.
pub fn load(folder: &Path) -> io::Result<Manifest> {
    let manifest: Manifest = read_json(&folder.join(MANIFEST))?;
    if manifest.format != FORMAT || manifest.version != 1 {
        return Err(invalid(format!("{MANIFEST}: not a version 1 {FORMAT} manifest")));
    }
    for entry in &manifest.files {
        check_path(&entry.path)?;
    }
    Ok(manifest)
}

/// Re-hashes every file listed in the manifest and looks for unlisted ones. Reads only; see
/// [`record`] to log the result.
pub fn verify(folder: &Path, progress: &mut dyn FnMut(&str)) -> io::Result<VerifyReport> {
    let manifest = load(folder)?;
    let mut report = VerifyReport {
        files: manifest.files.len(),
        bytes: manifest.files.iter().map(|e| e.size).sum(),
        missing: Vec::new(),
        corrupt: Vec::new(),
        extra: Vec::new(),
    };
    for entry in &manifest.files {
        progress(&entry.path);
        let full = join(folder, &entry.path)?;
        let size = match fs::metadata(&full) {
            Ok(m) if m.is_file() => m.len(),
            Ok(_) => {
                report.missing.push(entry.path.clone());
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                report.missing.push(entry.path.clone());
                continue;
            }
            Err(e) => {
                report
                    .corrupt
                    .push((entry.path.clone(), Problem::Unreadable(e.to_string())));
                continue;
            }
        };
        if size != entry.size {
            report.corrupt.push((
                entry.path.clone(),
                Problem::Size {
                    expected: entry.size,
                    actual: size,
                },
            ));
            continue;
        }
        match Sha256::of_file(&full, &mut |_| {}) {
            Ok(actual) if actual == entry.sha256 => {}
            Ok(actual) => report.corrupt.push((
                entry.path.clone(),
                Problem::Hash {
                    expected: entry.sha256,
                    actual,
                },
            )),
            Err(e) => report
                .corrupt
                .push((entry.path.clone(), Problem::Unreadable(e.to_string()))),
        }
    }
    let listed: std::collections::HashSet<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
    report.extra = list_files(folder)?
        .into_iter()
        .filter(|p| !listed.contains(p.as_str()))
        .collect();
    Ok(report)
}

/// Appends a check to the vault's log.
pub fn record(folder: &Path, report: &VerifyReport, at: Stamp) -> io::Result<Check> {
    let path = folder.join(CHECKS);
    let mut log: Checks = match fs::metadata(&path) {
        Ok(_) => read_json(&path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Checks::default(),
        Err(e) => return Err(e),
    };
    let check = Check {
        at,
        files: report.files,
        missing: report.missing.len(),
        corrupt: report.corrupt.len(),
        extra: report.extra.len(),
    };
    log.checks.push(check.clone());
    write_json(&path, &log)?;
    Ok(check)
}

/// Every logged check, oldest first.
pub fn history(folder: &Path) -> io::Result<Vec<Check>> {
    let path = folder.join(CHECKS);
    match fs::metadata(&path) {
        Ok(_) => Ok(read_json::<Checks>(&path)?.checks),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// When the vault was last verified: the latest logged check, or else the manifest's creation
/// (the files were hashed then).
pub fn last_verified(folder: &Path) -> io::Result<Stamp> {
    match history(folder)?.iter().map(|c| c.at).max() {
        Some(at) => Ok(at),
        None => Ok(load(folder)?.created),
    }
}

/// Whether a vault last verified at `last` should be checked again.
pub fn is_due(last: Stamp, now: Stamp) -> bool {
    now.0.saturating_sub(last.0) >= RECHECK_AFTER_DAYS * 86_400
}

/// Every regular file under `folder` as a relative `/` path, sorted, without the vault's own
/// files. Symbolic links are skipped: they could point outside the drive.
fn list_files(folder: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![(folder.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                return Err(invalid(format!(
                    "file name isn't valid Unicode: {}",
                    entry.path().display()
                )));
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push((entry.path(), rel));
            } else if kind.is_file() && !(prefix.is_empty() && is_own_file(&name)) {
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn is_own_file(name: &str) -> bool {
    name == MANIFEST || name == CHECKS || name.ends_with(".vault-tmp")
}

/// Manifest paths come from a drive that may have been tampered with: allow only plain relative
/// paths, so nothing outside the vault folder is ever read.
fn check_path(path: &str) -> io::Result<()> {
    let ok = !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("{MANIFEST}: bad path {path:?}")))
    }
}

fn join(folder: &Path, path: &str) -> io::Result<PathBuf> {
    check_path(path)?;
    Ok(folder.join(path))
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

/// Writes through a temporary file and a rename, so an interrupted write never leaves half a
/// manifest.
fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".vault-tmp");
    let mut file = File::create(&tmp)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

/// A UTC time in whole seconds since 1970, written as `2026-10-22T13:05:00Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(pub u64);

impl Stamp {
    pub fn now() -> Stamp {
        Stamp(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }

    /// Whole days from `self` to `later`.
    pub fn days_until(self, later: Stamp) -> u64 {
        later.0.saturating_sub(self.0) / 86_400
    }

    pub fn parse(s: &str) -> Option<Stamp> {
        let b = s.as_bytes();
        let shape = b.len() == 20
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b'T'
            && b[13] == b':'
            && b[16] == b':'
            && b[19] == b'Z';
        if !shape {
            return None;
        }
        let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u64>().ok();
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !(1970..=9999).contains(&y)
            || !(1..=12).contains(&mo)
            || !(1..=31).contains(&d)
            || h > 23
            || mi > 59
            || se > 59
        {
            return None;
        }
        let days = days_from_civil(y as i64, mo as i64, d as i64);
        let stamp = Stamp(days as u64 * 86_400 + h * 3600 + mi * 60 + se);
        // Rejects dates like 02-30 that would silently roll over.
        (stamp.to_string() == s).then_some(stamp)
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = civil_from_days((self.0 / 86_400) as i64);
        let s = self.0 % 86_400;
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            s / 3600,
            s / 60 % 60,
            s % 60
        )
    }
}

impl Serialize for Stamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Stamp, D::Error> {
        let s = String::deserialize(d)?;
        Stamp::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a UTC time: {s:?}")))
    }
}

// Calendar conversions from Howard Hinnant's "chrono-compatible low-level date algorithms".
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(Stamp(0).to_string(), "1970-01-01T00:00:00Z");
        let s = Stamp::parse("2026-10-22T13:05:09Z").unwrap();
        assert_eq!(s.to_string(), "2026-10-22T13:05:09Z");
        assert_eq!(
            Stamp::parse("2024-02-29T00:00:00Z").unwrap().to_string(),
            "2024-02-29T00:00:00Z"
        );
        assert_eq!(Stamp::parse("2026-02-30T00:00:00Z"), None);
        assert_eq!(Stamp::parse("2026-10-22 13:05:09"), None);
        assert!(!is_due(s, Stamp(s.0 + 364 * 86_400)));
        assert!(is_due(s, Stamp(s.0 + 365 * 86_400)));
    }

    #[test]
    fn manifest_paths() {
        for good in ["a.zim", "packs/a.zim"] {
            assert!(check_path(good).is_ok());
        }
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "a//b", "C:/x", "a\\b", "./a"] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }
}
