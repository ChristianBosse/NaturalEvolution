//! Time and peak heap for extracting a 50-page PDF. Its own test binary, because it counts every
//! allocation in the process. Run with `--release --nocapture` for meaningful numbers; set
//! `NEVO_PDF=<path>` to measure another file too.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use nevo_extract::{extract, Kind, Limits};

struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator unchanged and only updates counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = NOW.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Extracts and returns (seconds, peak heap above the starting point in MB, characters, passages).
fn measure(bytes: Vec<u8>) -> (f64, f64, usize, usize) {
    let size = bytes.len();
    let base = NOW.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let start = Instant::now();
    let out = extract(bytes, Kind::Pdf, &Limits::default()).unwrap();
    let secs = start.elapsed().as_secs_f64();
    // The input was moved in; count it as part of the peak, as a caller holding it would.
    let peak = (PEAK.load(Ordering::Relaxed) - base) as f64 / 1e6;
    assert_eq!(out.failed_pages, 0);
    let _ = size;
    (secs, peak, out.text.chars().count(), out.passages.len())
}

#[test]
fn fifty_page_pdf() {
    let bytes = common::pdf(&common::book(50), "Fifty pages");
    let size = bytes.len();
    let (secs, peak, chars, passages) = measure(bytes);
    println!("50-page PDF, {size} bytes: {secs:.3} s, peak heap {peak:.1} MB, {chars} chars, {passages} passages");
    assert!(chars > 50 * 2500);
    // Loose bounds: debug builds on a busy machine are slow. The real numbers are printed.
    assert!(secs < 60.0, "took {secs} s");
    assert!(peak < 100.0, "peak heap {peak} MB");

    if let Ok(path) = std::env::var("NEVO_PDF") {
        let bytes = std::fs::read(&path).unwrap();
        let size = bytes.len();
        let (secs, peak, chars, passages) = measure(bytes.clone());
        println!("{path}, {size} bytes: {secs:.3} s, peak heap {peak:.1} MB, {chars} chars, {passages} passages");
        // `NEVO_PDF_OUT=<path>` saves the clean text, to judge its quality by eye.
        if let Ok(out) = std::env::var("NEVO_PDF_OUT") {
            std::fs::write(out, extract(bytes, Kind::Pdf, &Limits::default()).unwrap().text).unwrap();
        }
    }
}
