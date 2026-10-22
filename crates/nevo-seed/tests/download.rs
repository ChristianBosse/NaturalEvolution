//! Downloader tests against a tiny HTTP server on 127.0.0.1 that can cut connections, ignore
//! `Range`, fail, or serve the wrong bytes.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nevo_seed::download::{self, part_path, DownloadError, Failure, Options, Request};
use nevo_seed::Sha256;

/// What the server does for one request.
#[derive(Clone)]
enum Reply {
    /// Serve `body`, honouring `Range` if `ranges`, and closing the connection after `cut` body
    /// bytes if set.
    Body {
        body: Vec<u8>,
        ranges: bool,
        cut: Option<usize>,
    },
    Status(u16),
}

fn body(body: &[u8]) -> Reply {
    Reply::Body {
        body: body.to_vec(),
        ranges: true,
        cut: None,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    path: String,
    range: Option<String>,
}

type Handler = dyn Fn(&str, usize) -> Reply + Send + Sync;

/// Starts a server; `handler(path, n)` answers the `n`th request (from 0) for `path`.
fn serve(handler: impl Fn(&str, usize) -> Reply + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let counts = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let handler: Arc<Handler> = Arc::new(handler);
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let (handler, log, counts) = (handler.clone(), log.clone(), counts.clone());
            thread::spawn(move || answer(stream.unwrap(), &*handler, &log, &counts));
        }
    });
    (base, seen)
}

fn answer(mut stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Seen>>, counts: &Mutex<HashMap<String, usize>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.trim().to_string());
            }
        }
    }
    log.lock().unwrap().push(Seen {
        path: path.clone(),
        range: range.clone(),
    });
    let n = {
        let mut counts = counts.lock().unwrap();
        let n = counts.entry(path.clone()).or_default();
        *n += 1;
        *n - 1
    };
    match handler(&path, n) {
        Reply::Status(code) => {
            let _ = write!(
                stream,
                "HTTP/1.1 {code} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
        Reply::Body { body, ranges, cut } => {
            let from = range
                .filter(|_| ranges)
                .and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse::<usize>().ok());
            let (head, part) = match from {
                Some(f) if f >= body.len() => {
                    let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", body.len());
                    (head, &body[..0])
                }
                Some(f) => {
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {f}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
                        body.len(),
                        body.len() - f
                    );
                    (head, &body[f..])
                }
                None => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    (head, &body[..])
                }
            };
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&part[..cut.unwrap_or(part.len()).min(part.len())]);
            let _ = stream.flush();
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn data(len: usize, salt: u8) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + i / 251) as u8) ^ salt).collect()
}

fn fast() -> Options {
    Options {
        backoff: Duration::from_millis(10),
        retries: 2,
        ..Options::default()
    }
}

fn request(urls: Vec<String>, content: &[u8]) -> Request {
    Request {
        urls,
        sha256: Some(Sha256::of(content)),
        size: Some(content.len() as u64),
    }
}

#[test]
fn resumes_after_a_cut_connection() {
    let content = data(300_000, 1);
    let served = content.clone();
    let (base, seen) = serve(move |_, n| Reply::Body {
        body: served.clone(),
        ranges: true,
        cut: (n == 0).then_some(100_000),
    });
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("file.zim");
    let mut events = Vec::new();
    let done = download::download(
        &request(vec![format!("{base}/file.zim")], &content),
        &out,
        &fast(),
        &mut |e| events.push(format!("{e:?}")),
    )
    .unwrap();

    assert_eq!(fs::read(&out).unwrap(), content);
    assert!(!part_path(&out).exists());
    assert_eq!(done.sha256, Sha256::of(&content));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].range, None);
    assert_eq!(seen[1].range.as_deref(), Some("bytes=100000-"));
    assert!(events.iter().any(|e| e.starts_with("Retrying")));
    assert!(events
        .iter()
        .any(|e| e.contains("Data { done: 300000, total: Some(300000) }")));
}

#[test]
fn continues_a_part_file_from_an_earlier_run() {
    let content = data(50_000, 2);
    let served = content.clone();
    let (base, seen) = serve(move |_, _| body(&served));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    fs::write(part_path(&out), &content[..20_000]).unwrap();

    download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(&out).unwrap(), content);
    assert_eq!(seen.lock().unwrap()[0].range.as_deref(), Some("bytes=20000-"));
}

#[test]
fn hash_mismatch_is_rejected_and_the_part_removed() {
    let content = data(40_000, 3);
    let wrong = data(40_000, 4);
    let (base, _) = serve(move |_, _| body(&wrong));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");

    let err = download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap_err();
    let DownloadError::Failed(failures) = err else {
        panic!("{err}")
    };
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(failures[0].1, Failure::HashMismatch { .. }),
        "{:?}",
        failures[0].1
    );
    assert!(!out.exists());
    assert!(!part_path(&out).exists());
}

#[test]
fn falls_back_to_mirrors_in_order() {
    let content = data(30_000, 5);
    let served = content.clone();
    let (base, seen) = serve(move |path, _| match path {
        "/gone" => Reply::Status(404),
        "/broken" => Reply::Status(503),
        _ => body(&served),
    });
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    let urls = vec![format!("{base}/gone"), format!("{base}/broken"), format!("{base}/good")];
    let mut failed = Vec::new();
    let options = Options { retries: 1, ..fast() };
    let done = download::download(&request(urls, &content), &out, &options, &mut |e| {
        if let download::Event::UrlFailed { url, failure } = e {
            failed.push((url.to_string(), failure.clone()));
        }
    })
    .unwrap();

    assert_eq!(done.url, Some(format!("{base}/good")));
    assert_eq!(fs::read(&out).unwrap(), content);
    assert_eq!(
        failed,
        [
            (format!("{base}/gone"), Failure::Status(404)),
            (format!("{base}/broken"), Failure::Status(503))
        ]
    );
    // 404 is final; 503 is retried once.
    let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, ["/gone", "/broken", "/broken", "/good"]);
}

#[test]
fn bad_bytes_in_an_old_part_get_one_clean_retry() {
    let content = data(30_000, 6);
    let served = content.clone();
    let (base, seen) = serve(move |_, _| body(&served));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    fs::write(part_path(&out), vec![0xAA; 10_000]).unwrap();

    download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(&out).unwrap(), content);
    let ranges: Vec<Option<String>> = seen.lock().unwrap().iter().map(|s| s.range.clone()).collect();
    assert_eq!(ranges, [Some("bytes=10000-".to_string()), None]);
}

#[test]
fn server_without_range_support_restarts_from_zero() {
    let content = data(30_000, 7);
    let served = content.clone();
    let (base, _) = serve(move |_, _| Reply::Body {
        body: served.clone(),
        ranges: false,
        cut: None,
    });
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    fs::write(part_path(&out), &content[..5_000]).unwrap();

    download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(&out).unwrap(), content);
}

#[test]
fn complete_file_is_not_downloaded_again() {
    let content = data(10_000, 8);
    let (base, seen) = serve(|_, _| Reply::Status(500));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    fs::write(&out, &content).unwrap();

    let done = download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(done.url, None);
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn wrong_size_from_the_server_is_refused() {
    let content = data(10_000, 9);
    let bigger = data(12_000, 9);
    let (base, _) = serve(move |_, _| body(&bigger));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");

    let err = download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap_err();
    let DownloadError::Failed(failures) = err else {
        panic!("{err}")
    };
    assert_eq!(
        failures[0].1,
        Failure::SizeMismatch {
            expected: 10_000,
            actual: 12_000
        }
    );
}

#[test]
fn bandwidth_limit_slows_the_transfer() {
    let content = data(64 * 1024, 10);
    let served = content.clone();
    let (base, _) = serve(move |_, _| body(&served));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    let options = Options {
        max_rate: Some(128 * 1024),
        ..fast()
    };

    let start = Instant::now();
    download::download(
        &request(vec![format!("{base}/f")], &content),
        &out,
        &options,
        &mut |_| {},
    )
    .unwrap();
    // 64 KiB at 128 KiB/s: about half a second.
    assert!(start.elapsed() >= Duration::from_millis(400), "{:?}", start.elapsed());
}

#[test]
fn unreachable_server_fails_after_retries() {
    // Bind then drop, so nothing listens on the port.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f");
    let err = download::download(
        &Request {
            urls: vec![format!("http://127.0.0.1:{port}/f")],
            ..Request::default()
        },
        &out,
        &fast(),
        &mut |_| {},
    )
    .unwrap_err();
    let DownloadError::Failed(failures) = err else {
        panic!("{err}")
    };
    assert!(matches!(failures[0].1, Failure::Network(_)));
}
