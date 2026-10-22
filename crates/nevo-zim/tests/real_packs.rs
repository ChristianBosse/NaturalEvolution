//! Tests on real Kiwix packs. They run only when `NEVO_ZIM_DIR` names a folder holding them:
//!
//! - `wikipedia_en_ray-charles_maxi_2026-08.zim` (3 MB)
//! - `wikipedia_en_medicine_mini_2026-10.zim` (174 MB)
//! - `wikipedia_en_medicine_nopic_2026-10.zim` (1 GB)
//!
//! The measurement over the whole 1 GB pack is ignored by default; run it with
//! `cargo test --release -p nevo-zim --test real_packs -- --ignored --nocapture`.

use std::path::PathBuf;
use std::time::Instant;

use nevo_zim::{extract, Pack, ZimError, PASSAGE_MAX};

const RAY_CHARLES: &str = "wikipedia_en_ray-charles_maxi_2026-08.zim";
const WIKIMED_MINI: &str = "wikipedia_en_medicine_mini_2026-10.zim";
const WIKIMED_NOPIC: &str = "wikipedia_en_medicine_nopic_2026-10.zim";

fn pack_path(name: &str) -> Option<PathBuf> {
    let Some(dir) = std::env::var_os("NEVO_ZIM_DIR") else {
        eprintln!("skipped: NEVO_ZIM_DIR is not set");
        return None;
    };
    let path = PathBuf::from(dir).join(name);
    if !path.exists() {
        eprintln!("skipped: {} not found", path.display());
        return None;
    }
    Some(path)
}

fn open(name: &str) -> Option<Pack> {
    pack_path(name).map(|p| Pack::open(p).expect("pack opens"))
}

#[test]
fn ray_charles_metadata_and_pages() {
    let Some(pack) = open(RAY_CHARLES) else { return };
    let m = pack.metadata();
    assert_eq!(m.title.as_deref(), Some("Ray Charles"));
    assert_eq!(m.language.as_deref(), Some("eng"));
    assert_eq!(m.flavour.as_deref(), Some("maxi"));
    assert_eq!(m.name.as_deref(), Some("wikipedia_en_ray-charles"));
    assert!(m.date.is_some() && m.creator.is_some() && m.publisher.is_some() && m.description.is_some());
    assert!(m.has_xapian_index);
    assert!(m.article_count.unwrap() > 0 && m.article_count.unwrap() <= m.entry_count);
    let icon = pack.illustration().unwrap().expect("pack has an icon");
    assert!(icon.starts_with(b"\x89PNG"));

    let main = pack.main_page().unwrap().expect("main page");
    assert!(main.mime.starts_with("text/html"));
    assert_eq!(main.redirected_from, None);
    let html = pack.read(&main).unwrap();
    assert!(!html.is_empty());
    assert_eq!(pack.find(&main.path).unwrap().as_ref(), Some(&main));

    let ray = pack.find("Ray_Charles").unwrap().expect("article");
    assert_eq!(ray.title, "Ray Charles");
    let doc = extract(&String::from_utf8_lossy(&pack.read(&ray).unwrap()));
    assert!(doc.sections.len() > 5);
    assert!(doc.text().contains("Georgia"));

    for missing in ["", "No_such_article", "M/Title", &"x".repeat(10_000)] {
        assert_eq!(pack.find(missing).unwrap(), None, "{missing}");
    }
}

#[test]
fn ray_charles_serves_resources() {
    let Some(pack) = open(RAY_CHARLES) else { return };
    // Every article's images and stylesheets resolve, so a browser can render it.
    let ray = pack.find("Ray_Charles").unwrap().unwrap();
    let html = String::from_utf8(pack.read(&ray).unwrap()).unwrap();
    let mut checked = 0;
    for attr in ["src=\"", "href=\""] {
        for part in html.split(attr).skip(1) {
            let link = &part[..part.find('"').unwrap()];
            if link.contains("://") || link.starts_with('#') || !(link.ends_with(".css") || link.contains("webp")) {
                continue;
            }
            let path = link.trim_start_matches("./").split(['?', '#']).next().unwrap();
            let path = path.strip_prefix("../").unwrap_or(path);
            let entry = pack.find(path).unwrap().unwrap_or_else(|| panic!("{path} not found"));
            assert!(!pack.read(&entry).unwrap().is_empty());
            checked += 1;
        }
    }
    assert!(checked > 0, "no resources found in the page");
}

#[test]
fn ray_charles_articles_iterate_and_extract() {
    let Some(pack) = open(RAY_CHARLES) else { return };
    let mut count = 0;
    for article in pack.articles().unwrap() {
        let article = article.unwrap();
        assert!(!article.path.is_empty() && !article.title.is_empty());
        let found = pack.find(&article.path).unwrap().expect("article path resolves");
        assert_eq!(found.title, article.title);
        let doc = extract(&article.html);
        assert!(doc.passages().iter().all(|p| p.text.chars().count() <= PASSAGE_MAX));
        count += 1;
    }
    // The listing also counts redirects; pages are fewer.
    assert!(
        count > 50 && count <= pack.metadata().article_count.unwrap() as usize,
        "{count}"
    );
}

#[test]
fn heart_attack_redirects_to_myocardial_infarction() {
    let Some(pack) = open(WIKIMED_MINI) else { return };
    let e = pack.find("Heart_attack").unwrap().expect("redirect resolves");
    assert_eq!(e.title, "Myocardial infarction");
    assert_eq!(e.path, "Myocardial_infarction");
    assert_eq!(e.redirected_from.as_deref(), Some("Heart_attack"));

    let article = pack
        .articles()
        .unwrap()
        .map(Result::unwrap)
        .find(|a| a.path == "Myocardial_infarction")
        .expect("article is listed");
    assert!(
        article.aliases.iter().any(|a| a == "Heart attack"),
        "{:?}",
        article.aliases
    );
    assert!(!article.aliases.contains(&article.title));
}

#[test]
fn snakebite_text_is_clean() {
    let Some(pack) = open(WIKIMED_NOPIC) else { return };
    let e = pack.find("Snakebite").unwrap().unwrap();
    let doc = extract(&String::from_utf8_lossy(&pack.read(&e).unwrap()));
    assert_eq!(doc.title.as_deref(), Some("Snakebite"));
    let headings: Vec<_> = doc.sections.iter().map(|s| s.heading.as_str()).collect();
    for wanted in [
        "Signs and symptoms",
        "Treatment",
        "First aid",
        "Antivenom",
        "Epidemiology",
    ] {
        assert!(headings.contains(&wanted), "{wanted} missing from {headings:?}");
    }
    for unwanted in ["References", "Further reading", "External links"] {
        assert!(!headings.contains(&unwanted), "{unwanted} kept");
    }
    let text = doc.text();
    assert!(text.contains("Antivenom is effective at preventing death"));
    assert!(text.contains("Treatment | Washing the wound with soap and water, antivenom"));
    for junk in [
        "Retrieved",
        "doi:",
        "ISBN",
        "PMID",
        "[1]",
        "[edit]",
        "Categories:",
        "This page is issued from",
        "See also:",
        "Main articles:",
        "mw-parser-output",
        "RLCONF",
    ] {
        assert!(!text.contains(junk), "found {junk:?}");
    }

    let passages = doc.passages();
    assert!(passages.len() > 20);
    assert!(passages.iter().any(|p| p.heading == "Treatment > First aid"));
    for p in &passages {
        assert!(p.text.chars().count() <= PASSAGE_MAX);
    }
}

/// Hostile files: corrupted copies of a real pack must give errors, not panics or hangs.
#[test]
fn damaged_packs_fail_cleanly() {
    let Some(path) = pack_path(RAY_CHARLES) else { return };
    let original = std::fs::read(&path).unwrap();
    let dir = std::env::temp_dir().join(format!("nevo-zim-damaged-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut opened, mut errors) = (0, 0);
    // More rounds for a longer hunt: NEVO_ZIM_DAMAGE_ROUNDS=5000.
    let rounds = std::env::var("NEVO_ZIM_DAMAGE_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(60);
    for round in 0..rounds {
        let mut bytes = original.clone();
        if round % 10 == 0 {
            bytes.truncate(next() as usize % bytes.len());
        } else {
            // Half the damage goes to the first 256 KB, where the header, pointer lists and
            // directory entries are.
            let span = if round % 2 == 0 {
                bytes.len().min(256 << 10)
            } else {
                bytes.len()
            };
            for _ in 0..1 + next() % 20 {
                let at = next() as usize % span;
                bytes[at] = next() as u8;
            }
        }
        let file = dir.join(format!("damaged-{round}.zim"));
        std::fs::write(&file, &bytes).unwrap();
        let Ok(pack) = Pack::open(&file) else {
            errors += 1;
            continue;
        };
        opened += 1;
        let _ = pack.main_page();
        let _ = pack.illustration();
        for path in ["Ray_Charles", "index", "Heart_attack"] {
            if let Ok(Some(e)) = pack.find(path) {
                let _ = pack.read(&e);
            }
        }
        match pack.articles() {
            Ok(articles) => {
                for article in articles.take(400) {
                    match article {
                        Ok(a) => drop(extract(&a.html)),
                        Err(ZimError::Corrupt(_) | ZimError::TooLarge { .. } | ZimError::Io(_)) => errors += 1,
                    }
                }
            }
            Err(_) => errors += 1,
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
    eprintln!("damaged packs: {opened} opened, {errors} errors reported");
    assert!(errors > 0);
}

/// Peak memory from /proc/self/status, in MB (Linux only).
fn status_mb(key: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status
        .lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1e3)
}

/// Iterates and extracts the whole 1 GB pack, as a node indexing it would.
#[test]
#[ignore = "takes a minute; run with --release -- --ignored --nocapture"]
fn measure_large_pack() {
    let Some(path) = pack_path(WIKIMED_NOPIC) else { return };
    let t0 = Instant::now();
    let pack = Pack::open(&path).unwrap();
    let opened = t0.elapsed();
    let articles = pack.articles().unwrap();
    let scanned = t0.elapsed();
    let (mut count, mut html_bytes, mut text_bytes, mut passages, mut aliases, mut errors) = (0u64, 0, 0, 0, 0, 0);
    let mut peak_anon = 0f64;
    for article in articles {
        let Ok(article) = article else {
            errors += 1;
            continue;
        };
        let doc = extract(&article.html);
        html_bytes += article.html.len();
        text_bytes += doc.text().len();
        passages += doc.passages().len();
        aliases += article.aliases.len();
        count += 1;
        if count % 500 == 0 {
            peak_anon = peak_anon.max(status_mb("RssAnon:").unwrap_or(0.0));
        }
    }
    let total = t0.elapsed();
    println!(
        "open: {:.1} ms; scan of entries: {:.1} s",
        opened.as_secs_f64() * 1e3,
        scanned.as_secs_f64()
    );
    println!(
        "{count} articles, {aliases} aliases, {errors} errors in {:.1} s ({:.0} articles/s)",
        total.as_secs_f64(),
        count as f64 / total.as_secs_f64()
    );
    println!(
        "HTML {:.0} MB -> text {:.0} MB, {passages} passages",
        html_bytes as f64 / 1e6,
        text_bytes as f64 / 1e6
    );
    println!(
        "peak RssAnon (heap, sampled) {peak_anon:.0} MB; VmHWM (includes the mapped file) {:.0} MB",
        status_mb("VmHWM:").unwrap_or(0.0)
    );
    assert_eq!(errors, 0);
    assert!(count > 100_000);
}
