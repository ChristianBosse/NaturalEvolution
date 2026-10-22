//! Compares pdf-extract and lopdf's own `extract_text` on one PDF.
//!
//! Usage: pdf-eval <file.pdf> [reference.txt]
//! Quality is the share of the reference text's words found in the output, in order
//! (a longest-common-subsequence over words), so split, merged or shuffled words all count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let now = NOW.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static A: Counting = Counting;

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// Longest common subsequence length over words, in O(n*m) time and O(m) memory.
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut prev = vec![0u32; b.len() + 1];
    let mut cur = vec![0u32; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if x == y { prev[j] + 1 } else { cur[j].max(prev[j + 1]) };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] as usize
}

fn report(name: &str, text: &str, secs: f64, reference: Option<&[String]>) {
    let peak = PEAK.load(Ordering::Relaxed) as f64 / 1e6;
    print!("{name}: {secs:.3} s, peak heap {peak:.1} MB, {} chars", text.chars().count());
    if let Some(r) = reference {
        let got = words(text);
        let common = lcs(r, &got);
        print!(", word recall in order {:.1}%, extra words {}", 100.0 * common as f64 / r.len() as f64, got.len() - common);
    }
    println!();
    let sample: String = text.chars().skip(2000).take(400).collect();
    println!("--- sample ---\n{sample}\n--------------");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let reference = args.get(2).map(|p| words(&std::fs::read_to_string(p).unwrap()));
    println!("file: {} bytes", bytes.len());

    NOW.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    let t = Instant::now();
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    let load = t.elapsed().as_secs_f64();
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    println!("lopdf load: {load:.3} s, {} pages", pages.len());
    let text = doc.extract_text(&pages).unwrap_or_default();
    report("lopdf extract_text", &text, t.elapsed().as_secs_f64(), reference.as_deref());
    drop(doc);

    PEAK.store(NOW.load(Ordering::Relaxed), Ordering::Relaxed);
    let t = Instant::now();
    let text = pdf_extract::extract_text_from_mem(&bytes).unwrap();
    report("pdf-extract", &text, t.elapsed().as_secs_f64(), reference.as_deref());
}
