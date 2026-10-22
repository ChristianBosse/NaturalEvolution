//! `nevo-seed`: prepares USB sticks for offline NEVO nodes.
//!
//! ```text
//! nevo-seed download <url> <out> [--sha256 <hex>] [--size <bytes>] [--mirror <url>]...
//!                    [--rate <bytes per second>] [--retries <n>]
//! nevo-seed test-stick <folder> [--size <bytes>] [--write-only] [--keep] [--restart]
//! nevo-seed manifest <folder>
//! nevo-seed verify <folder>
//! ```
//!
//! Sizes take a K, M, G or T suffix (powers of 1024): `--size 8G`, `--rate 500K`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use nevo_seed::download::{self, Event};
use nevo_seed::stick::{self, Phase};
use nevo_seed::vault::{self, Stamp};
use nevo_seed::Sha256;

const USAGE: &str = "\
nevo-seed download <url> <out> [--sha256 <hex>] [--size <bytes>] [--mirror <url>]...
                   [--rate <bytes per second>] [--retries <n>]
nevo-seed test-stick <folder> [--size <bytes>] [--write-only] [--keep] [--restart]
nevo-seed manifest <folder>
nevo-seed verify <folder>

Sizes take a K, M, G or T suffix (powers of 1024), e.g. --size 8G or --rate 500K.";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("nevo-seed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the command; `Ok(false)` means it ran but found a problem (bad stick, damaged vault).
fn run() -> Result<bool, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "download" => cmd_download(rest),
        "test-stick" => cmd_test_stick(rest),
        "manifest" => cmd_manifest(rest),
        "verify" => cmd_verify(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(true)
        }
        "" => Err(format!("missing command\n\n{USAGE}")),
        other => Err(format!("unknown command: {other}\n\n{USAGE}")),
    }
}

/// Positional arguments, and `--flag value` pairs (switches get an empty value).
type Split = (Vec<String>, Vec<(String, String)>);

/// Splits arguments into positionals and flags; `switches` take no value.
fn split_args(args: Vec<String>, switches: &[&str]) -> Result<Split, String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            positional.push(arg);
        } else if switches.contains(&arg.as_str()) {
            flags.push((arg, String::new()));
        } else {
            let value = it.next().ok_or(format!("{arg} needs a value"))?;
            flags.push((arg, value));
        }
    }
    Ok((positional, flags))
}

fn cmd_download(args: Vec<String>) -> Result<bool, String> {
    let (pos, flags) = split_args(args, &[])?;
    let [url, out] = <[String; 2]>::try_from(pos).map_err(|_| "download needs <url> <out>".to_string())?;
    let mut request = download::Request {
        urls: vec![url],
        ..Default::default()
    };
    let mut options = download::Options::default();
    for (flag, value) in flags {
        match flag.as_str() {
            "--sha256" => request.sha256 = Some(Sha256::from_hex(&value).ok_or("--sha256 needs 64 hex digits")?),
            "--size" => request.size = Some(parse_size(&value)?),
            "--mirror" => request.urls.push(value),
            "--rate" => options.max_rate = Some(parse_size(&value)?),
            "--retries" => options.retries = value.parse().map_err(|_| format!("not a number: {value}"))?,
            other => return Err(format!("unknown option: {other}")),
        }
    }
    let out = PathBuf::from(out);
    let mut meter = Meter::new();
    let result = download::download(&request, &out, &options, &mut |event| match event {
        Event::Connecting { url, from: 0 } => meter.line(&format!("downloading {url}")),
        Event::Connecting { url, from } => meter.line(&format!("resuming at {} from {url}", bytes(from))),
        Event::Data { done, total } => meter.progress(done, total),
        Event::Retrying { failure, wait, .. } => meter.line(&format!("{failure}; retrying in {}s", wait.as_secs())),
        Event::UrlFailed { url, failure } => meter.line(&format!("giving up on {url}: {failure}")),
        Event::Verifying => meter.line("checking"),
    });
    meter.done();
    let done = result.map_err(|e| e.to_string())?;
    match done.url {
        None => println!("{} is already complete", out.display()),
        Some(_) => println!("saved {} ({})", out.display(), bytes(done.size)),
    }
    println!("sha256 {}", done.sha256);
    Ok(true)
}

fn cmd_test_stick(args: Vec<String>) -> Result<bool, String> {
    let (pos, flags) = split_args(args, &["--write-only", "--keep", "--restart"])?;
    let [folder] = <[String; 1]>::try_from(pos).map_err(|_| "test-stick needs <folder>".to_string())?;
    let folder = PathBuf::from(folder);
    let mut options = stick::Options::default();
    let (mut keep, mut restart) = (false, false);
    for (flag, value) in flags {
        match flag.as_str() {
            "--size" => options.limit = Some(parse_size(&value)?),
            "--write-only" => options.write_only = true,
            "--keep" => keep = true,
            "--restart" => restart = true,
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if restart {
        stick::clean(&folder).map_err(|e| e.to_string())?;
    }
    if stick::status(&folder).map_err(|e| e.to_string())?.is_some() {
        println!("resuming the test in {}", folder.join(stick::TEST_DIR).display());
    } else {
        println!(
            "testing {}: writing test files to {}",
            folder.display(),
            stick::TEST_DIR
        );
        println!("existing files are not touched. Stop at any time and run again to resume.");
    }
    let stop = AtomicBool::new(false);
    let mut meter = Meter::new();
    let mut phase = None;
    let report = stick::run(&folder, &options, &stop, &mut |p| {
        if phase != Some(p.phase) {
            phase = Some(p.phase);
            meter.line(if p.phase == Phase::Writing {
                "writing"
            } else {
                "reading back"
            });
        }
        meter.progress(p.done, p.total);
    })
    .map_err(|e| e.to_string())?;
    meter.done();

    let full = if report.full { " (stick full)" } else { "" };
    println!("written:   {}{full}", bytes(report.written));
    if !report.finished() {
        println!("Written. For a reliable result, unplug and replug the stick, then run this again to read back.");
        return Ok(true);
    }
    println!("read back: {}", bytes(report.verified));
    if report.bad == 0 {
        println!("OK: all {} read back correctly.", bytes(report.good));
    } else {
        println!("bad:       {}", bytes(report.bad));
        for (start, len) in &report.bad_ranges {
            println!("  {} bad from offset {}", bytes(*len), bytes(*start));
        }
        println!(
            "PROBLEM: only the first {} can be trusted. This stick is fake or failing; don't use it for NEVO.",
            bytes(report.usable())
        );
    }
    if keep {
        println!("test files kept in {}", folder.join(stick::TEST_DIR).display());
    } else {
        stick::clean(&folder).map_err(|e| e.to_string())?;
    }
    Ok(report.bad == 0)
}

fn cmd_manifest(args: Vec<String>) -> Result<bool, String> {
    let (pos, flags) = split_args(args, &[])?;
    if let Some((flag, _)) = flags.first() {
        return Err(format!("unknown option: {flag}"));
    }
    let [folder] = <[String; 1]>::try_from(pos).map_err(|_| "manifest needs <folder>".to_string())?;
    let folder = PathBuf::from(folder);
    let manifest =
        vault::create(&folder, Stamp::now(), &mut |path| eprintln!("hashing {path}")).map_err(|e| e.to_string())?;
    let total: u64 = manifest.files.iter().map(|e| e.size).sum();
    println!(
        "wrote {}: {} files, {}",
        vault::MANIFEST,
        manifest.files.len(),
        bytes(total)
    );
    Ok(true)
}

fn cmd_verify(args: Vec<String>) -> Result<bool, String> {
    let (pos, flags) = split_args(args, &[])?;
    if let Some((flag, _)) = flags.first() {
        return Err(format!("unknown option: {flag}"));
    }
    let [folder] = <[String; 1]>::try_from(pos).map_err(|_| "verify needs <folder>".to_string())?;
    let folder = PathBuf::from(folder);
    if !folder.join(vault::MANIFEST).exists() {
        return Err(format!(
            "no {} in {}; create one with `nevo-seed manifest <folder>`",
            vault::MANIFEST,
            folder.display()
        ));
    }
    let now = Stamp::now();
    let last = vault::last_verified(&folder).map_err(|e| e.to_string())?;
    println!("last verified: {} ({} days ago)", day(last), last.days_until(now));
    let report = vault::verify(&folder, &mut |path| eprintln!("checking {path}")).map_err(|e| e.to_string())?;
    vault::record(&folder, &report, now).map_err(|e| e.to_string())?;

    println!("{} files, {}", report.files, bytes(report.bytes));
    for path in &report.missing {
        println!("MISSING  {path}");
    }
    for (path, problem) in &report.corrupt {
        println!("DAMAGED  {path}: {problem}");
    }
    for path in &report.extra {
        println!("extra    {path} (not in the manifest)");
    }
    if report.ok() {
        println!("OK: every file is intact.");
    } else {
        println!(
            "PROBLEM: {} missing, {} damaged. Restore them from your second copy.",
            report.missing.len(),
            report.corrupt.len()
        );
    }
    let next = Stamp(now.0 + vault::RECHECK_AFTER_DAYS * 86_400);
    println!(
        "next check due: {} (unpowered flash fades; keep a second copy on other media)",
        day(next)
    );
    Ok(report.ok())
}

/// `2026-10-22T13:05:00Z` → `2026-10-22`.
fn day(stamp: Stamp) -> String {
    stamp.to_string()[..10].to_string()
}

fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let t = t.strip_suffix("iB").or_else(|| t.strip_suffix('B')).unwrap_or(t);
    let (digits, shift) = match t.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&t[..t.len() - 1], 10),
        Some('M') => (&t[..t.len() - 1], 20),
        Some('G') => (&t[..t.len() - 1], 30),
        Some('T') => (&t[..t.len() - 1], 40),
        _ => (t, 0),
    };
    let n: u64 = digits.trim().parse().map_err(|_| format!("not a size: {s}"))?;
    n.checked_mul(1 << shift).ok_or(format!("too big: {s}"))
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A progress line on stderr, redrawn at most twice a second.
struct Meter {
    last: Option<Instant>,
    start: Instant,
    first: u64,
    open: bool,
}

impl Meter {
    fn new() -> Meter {
        Meter {
            last: None,
            start: Instant::now(),
            first: 0,
            open: false,
        }
    }

    fn line(&mut self, text: &str) {
        self.done();
        eprintln!("{text}");
        self.last = None;
    }

    fn progress(&mut self, done: u64, total: Option<u64>) {
        let now = Instant::now();
        if self.last.is_none() {
            self.start = now;
            self.first = done;
        } else if self.last.is_some_and(|t| now - t < Duration::from_millis(500)) && Some(done) != total {
            return;
        }
        self.last = Some(now);
        let secs = (now - self.start).as_secs_f64();
        let rate = if secs > 0.5 {
            format!(", {}/s", bytes(((done - self.first) as f64 / secs) as u64))
        } else {
            String::new()
        };
        let of = match total {
            Some(t) if t > 0 => format!(" of {} ({:.0}%)", bytes(t), done as f64 * 100.0 / t as f64),
            _ => String::new(),
        };
        eprint!("\r  {}{of}{rate}          ", bytes(done));
        self.open = true;
    }

    fn done(&mut self) {
        if self.open {
            eprintln!();
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1000"), Ok(1000));
        assert_eq!(parse_size("8G"), Ok(8 << 30));
        assert_eq!(parse_size("500k"), Ok(500 << 10));
        assert_eq!(parse_size("2MiB"), Ok(2 << 20));
        assert!(parse_size("lots").is_err());
        assert_eq!(bytes(1536), "1.5 KiB");
    }
}
