//! Resumable, verified downloads.
//!
//! A download goes to `<out>.part` and only becomes `<out>` after its size and SHA-256 check
//! out, so a file with the final name is always complete. If the transfer stops for any reason
//! (cut connection, timeout, the program was closed), the next attempt or the next run asks the
//! server for the rest with an HTTP `Range` request.
//!
//! Mirrors are tried in order. A mirror that fails for good (404, wrong size, wrong hash) is
//! skipped; one that fails for a passing reason (network error, 5xx) is retried with growing
//! waits first. A `.part` left by one mirror is continued from the next, because the expected
//! hash is checked at the end anyway.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use ureq::Agent;

use crate::hash::Sha256;

/// What to download.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The main URL first, then mirrors, in the order to try them.
    pub urls: Vec<String>,
    pub sha256: Option<Sha256>,
    pub size: Option<u64>,
}

/// How to download.
#[derive(Debug, Clone)]
pub struct Options {
    /// Bandwidth limit in bytes per second.
    pub max_rate: Option<u64>,
    /// Extra attempts per URL after a passing failure. A failure after some data arrived doesn't
    /// count, so a slow but working link always gets there.
    pub retries: u32,
    /// Wait before the first retry; it doubles on each retry, up to `max_backoff`.
    pub backoff: Duration,
    pub max_backoff: Duration,
    pub connect_timeout: Duration,
    /// Time budget for one request's body. When it runs out the connection is dropped and the
    /// download resumes on a fresh one. This is what catches a connection that stalls without
    /// closing; a healthy one just costs a reconnect. Not applied to servers that ignore `Range`,
    /// since they would restart from zero every time.
    pub request_budget: Option<Duration>,
    pub user_agent: String,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            max_rate: None,
            retries: 5,
            backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(30),
            request_budget: Some(Duration::from_secs(15 * 60)),
            user_agent: concat!("nevo-seed/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

/// What happened, for progress displays.
#[derive(Debug)]
pub enum Event<'a> {
    /// Starting a request; `from` is the byte offset asked for (non-zero when resuming).
    Connecting { url: &'a str, from: u64 },
    /// Bytes in the `.part` file so far, and the full size if known.
    Data { done: u64, total: Option<u64> },
    /// Waiting before trying the same URL again.
    Retrying {
        url: &'a str,
        failure: &'a Failure,
        wait: Duration,
    },
    /// Giving up on this URL; the next mirror (if any) is tried.
    UrlFailed { url: &'a str, failure: &'a Failure },
    /// Checking size and hash.
    Verifying,
}

/// Why one URL didn't deliver the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The server answered with an HTTP error.
    Status(u16),
    /// Connection problem, timeout or cut transfer.
    Network(String),
    /// The server answered a `Range` request with a different range.
    BadRange(String),
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
    HashMismatch {
        expected: Sha256,
        actual: Sha256,
    },
}

impl Failure {
    /// Whether trying the same URL again may help.
    fn passing(&self) -> bool {
        match self {
            Failure::Status(s) => *s >= 500 || *s == 408 || *s == 429,
            Failure::Network(_) => true,
            Failure::BadRange(_) | Failure::SizeMismatch { .. } | Failure::HashMismatch { .. } => false,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Status(s) => write!(f, "HTTP status {s}"),
            Failure::Network(e) => write!(f, "network error: {e}"),
            Failure::BadRange(r) => write!(f, "server sent the wrong range: {r}"),
            Failure::SizeMismatch { expected, actual } => write!(f, "size is {actual} bytes, expected {expected}"),
            Failure::HashMismatch { expected, actual } => write!(f, "SHA-256 is {actual}, expected {expected}"),
        }
    }
}

#[derive(Debug)]
pub enum DownloadError {
    NoUrls,
    /// A local file problem (disk full, no permission). Other mirrors wouldn't help.
    Io(io::Error),
    /// Every URL failed; the last failure of each, in order.
    Failed(Vec<(String, Failure)>),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::NoUrls => write!(f, "no URL to download from"),
            DownloadError::Io(e) => write!(f, "file error: {e}"),
            DownloadError::Failed(list) => {
                write!(f, "download failed")?;
                for (url, failure) in list {
                    write!(f, "\n  {url}: {failure}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<io::Error> for DownloadError {
    fn from(e: io::Error) -> Self {
        DownloadError::Io(e)
    }
}

/// A finished download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub size: u64,
    pub sha256: Sha256,
    /// Where it came from, or `None` if `out` already held the expected file.
    pub url: Option<String>,
}

/// The `.part` file used while downloading to `out`.
pub fn part_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    out.with_file_name(name)
}

/// Downloads `request` to `out`, see the module docs.
pub fn download(
    request: &Request,
    out: &Path,
    options: &Options,
    on_event: &mut dyn FnMut(Event),
) -> Result<Downloaded, DownloadError> {
    if request.urls.is_empty() {
        return Err(DownloadError::NoUrls);
    }
    // Only a hash proves an existing file is the right one; without it, download again.
    if let (Some(expected), Ok(meta)) = (request.sha256, fs::metadata(out)) {
        if meta.is_file() && request.size.is_none_or(|s| s == meta.len()) {
            on_event(Event::Verifying);
            if Sha256::of_file(out, &mut |_| {})? == expected {
                return Ok(Downloaded {
                    size: meta.len(),
                    sha256: expected,
                    url: None,
                });
            }
        }
    }

    let part = part_path(out);
    let agent: Agent = Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(options.user_agent.as_str())
        // Compressed transfer would break byte offsets for resuming.
        .accept_encoding("identity")
        .timeout_connect(Some(options.connect_timeout))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into();

    let mut failures = Vec::new();
    for url in &request.urls {
        let mut budget = options.request_budget;
        let mut strikes = 0u32;
        let mut fresh_retry = false;
        let failure = loop {
            let (failure, progressed) = match fetch(&agent, url, request.size, &part, options, &mut budget, on_event)? {
                Fetch::Done { resumed } => {
                    on_event(Event::Verifying);
                    match verify(&part, request)? {
                        Ok(downloaded) => {
                            fs::rename(&part, out)?;
                            return Ok(Downloaded {
                                url: Some(url.clone()),
                                ..downloaded
                            });
                        }
                        Err(failure) => {
                            fs::remove_file(&part)?;
                            // The bad bytes may come from an earlier, different transfer: give
                            // this URL one clean try before blaming it.
                            if resumed && !fresh_retry {
                                fresh_retry = true;
                                continue;
                            }
                            break failure;
                        }
                    }
                }
                Fetch::Failed { failure, progressed } => (failure, progressed),
            };
            if !failure.passing() {
                break failure;
            }
            if progressed {
                strikes = 0;
            }
            strikes += 1;
            if strikes > options.retries {
                break failure;
            }
            let wait = options
                .backoff
                .saturating_mul(1 << (strikes - 1).min(16))
                .min(options.max_backoff);
            on_event(Event::Retrying {
                url,
                failure: &failure,
                wait,
            });
            thread::sleep(wait);
        };
        // A .part longer than the expected size is dropped by the next fetch.
        on_event(Event::UrlFailed { url, failure: &failure });
        failures.push((url.clone(), failure));
    }
    Err(DownloadError::Failed(failures))
}

enum Fetch {
    /// The server sent everything it had. `resumed`: the `.part` already held data before.
    Done {
        resumed: bool,
    },
    Failed {
        failure: Failure,
        progressed: bool,
    },
}

/// One request: continues `part` from its current length.
fn fetch(
    agent: &Agent,
    url: &str,
    size: Option<u64>,
    part: &Path,
    options: &Options,
    budget: &mut Option<Duration>,
    on_event: &mut dyn FnMut(Event),
) -> io::Result<Fetch> {
    let failed = |failure, progressed| Ok(Fetch::Failed { failure, progressed });
    let mut have = match fs::metadata(part) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e),
    };
    if let Some(size) = size {
        if have > size {
            fs::remove_file(part)?;
            have = 0;
        } else if have == size {
            return Ok(Fetch::Done { resumed: true });
        }
    }

    on_event(Event::Connecting { url, from: have });
    let mut builder = agent.get(url).config().timeout_recv_body(*budget).build();
    if have > 0 {
        builder = builder.header("Range", format!("bytes={have}-"));
    }
    let mut response = match builder.call() {
        Ok(r) => r,
        Err(e) => return failed(Failure::Network(e.to_string()), false),
    };

    let status = response.status().as_u16();
    let (start, total) = match status {
        206 => {
            let header = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            match parse_content_range(&header) {
                Some((first, total)) if first == have => (have, total),
                _ => return failed(Failure::BadRange(header), false),
            }
        }
        200 => {
            if have > 0 {
                // The server ignores Range and sends the whole file. Restarting it on a timer
                // would never finish, so let it run.
                *budget = None;
            }
            (0, response.body().content_length())
        }
        // Nothing left past our offset: the .part is probably complete; verify decides.
        416 if have > 0 => return Ok(Fetch::Done { resumed: true }),
        _ => return failed(Failure::Status(status), false),
    };
    if let (Some(expected), Some(actual)) = (size, total) {
        if expected != actual {
            return failed(Failure::SizeMismatch { expected, actual }, false);
        }
    }

    let mut file = OpenOptions::new().create(true).write(true).truncate(false).open(part)?;
    file.set_len(start)?;
    file.seek(SeekFrom::Start(start))?;
    let total = total.or(size);
    let mut done = start;
    let mut throttle = Throttle::new(options.max_rate);
    let mut buf = vec![0u8; throttle.chunk()];
    let mut reader = response.body_mut().as_reader();
    let failure = loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => break Some(Failure::Network(e.to_string())),
        };
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(expected) = size.filter(|&s| done > s) {
            break Some(Failure::SizeMismatch { expected, actual: done });
        }
        on_event(Event::Data { done, total });
        throttle.wait(n);
    };
    // Keep what arrived even on failure: it is where the next attempt resumes.
    file.sync_all()?;
    let progressed = done > start;
    match failure {
        Some(failure) => failed(failure, progressed),
        None if total.is_some_and(|t| done < t) => failed(
            Failure::Network(format!("connection closed at {done} of {} bytes", total.unwrap_or(0))),
            progressed,
        ),
        None => Ok(Fetch::Done { resumed: start > 0 }),
    }
}

/// Checks the finished `.part` against the expected size and hash.
fn verify(part: &Path, request: &Request) -> io::Result<Result<Downloaded, Failure>> {
    let size = fs::metadata(part)?.len();
    if let Some(expected) = request.size.filter(|&s| s != size) {
        return Ok(Err(Failure::SizeMismatch { expected, actual: size }));
    }
    let actual = Sha256::of_file(part, &mut |_| {})?;
    if let Some(expected) = request.sha256.filter(|&h| h != actual) {
        return Ok(Err(Failure::HashMismatch { expected, actual }));
    }
    Ok(Ok(Downloaded {
        size,
        sha256: actual,
        url: None,
    }))
}

/// Parses `bytes <first>-<last>/<total or *>` into the first byte and the full size.
fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let first: u64 = first.trim().parse().ok()?;
    let last: u64 = last.trim().parse().ok()?;
    if last < first {
        return None;
    }
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some((first, total))
}

/// Keeps the average rate of one transfer under a limit by sleeping between reads.
struct Throttle {
    rate: Option<u64>,
    start: Instant,
    bytes: u64,
}

impl Throttle {
    fn new(rate: Option<u64>) -> Throttle {
        Throttle {
            rate: rate.filter(|&r| r > 0),
            start: Instant::now(),
            bytes: 0,
        }
    }

    /// Read size: small enough that a low limit still gives a smooth flow.
    fn chunk(&self) -> usize {
        match self.rate {
            Some(rate) => (rate / 8).clamp(1024, 64 * 1024) as usize,
            None => 64 * 1024,
        }
    }

    fn wait(&mut self, n: usize) {
        let Some(rate) = self.rate else { return };
        self.bytes += n as u64;
        let due = Duration::from_secs_f64(self.bytes as f64 / rate as f64);
        if let Some(ahead) = due.checked_sub(self.start.elapsed()) {
            thread::sleep(ahead);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 100-199/200"), Some((100, Some(200))));
        assert_eq!(parse_content_range("bytes 0-9/*"), Some((0, None)));
        assert_eq!(parse_content_range("bytes */200"), None);
        assert_eq!(parse_content_range("bytes 9-0/10"), None);
    }

    #[test]
    fn part_name() {
        assert_eq!(part_path(Path::new("a/b.zim")), Path::new("a/b.zim.part"));
    }
}
