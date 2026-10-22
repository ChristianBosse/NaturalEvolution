//! Zip checks (bombs, entry counts, unsafe paths) and EPUB reading.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use zip::ZipArchive;

use crate::guard::Cancel;
use crate::html::{self, attr, local_name, Token, Tokenizer};
use crate::text::{self, DocBuilder};
use crate::{ExtractError, Limits, Rejected, MIB};

/// Entries smaller than this are not ratio-checked: tiny files of repeated bytes compress
/// very well and are harmless.
const RATIO_FLOOR: u64 = MIB;
/// The central directory lists all entries; more than this is an entry-count bomb even if the
/// count field says otherwise.
const MAX_DIRECTORY_BYTES: u64 = 64 * MIB;
const MAX_CONTAINER_BYTES: u64 = MIB;
const MAX_OPF_BYTES: u64 = 8 * MIB;
const MAX_CHAPTER_BYTES: u64 = 64 * MIB;

pub(crate) struct Summary {
    /// Has `META-INF/container.xml`, the file every EPUB must have.
    pub(crate) has_epub_container: bool,
}

/// Checks a whole zip file without unpacking it.
pub(crate) fn check(bytes: &[u8], limits: &Limits) -> Result<Summary, Rejected> {
    // Read the entry count from the end record before the zip crate allocates one record per
    // entry: a small file can claim millions.
    let (entries, directory) = end_record(bytes)?;
    if entries > limits.max_zip_entries {
        return Err(Rejected::ZipBomb(format!(
            "{entries} entries, more than {}",
            limits.max_zip_entries
        )));
    }
    if directory > MAX_DIRECTORY_BYTES {
        return Err(Rejected::ZipBomb(format!("central directory of {directory} bytes")));
    }
    let mut zip = open(bytes)?;
    if zip.len() as u64 > limits.max_zip_entries {
        return Err(Rejected::ZipBomb(format!(
            "{} entries, more than {}",
            zip.len(),
            limits.max_zip_entries
        )));
    }
    // Entries sharing compressed data are how the densest zip bombs work.
    if zip
        .has_overlapping_files()
        .map_err(|e| Rejected::BadArchive(e.to_string()))?
    {
        return Err(Rejected::ZipBomb("entries overlap".into()));
    }
    let mut unpacked = 0u64;
    let mut packed = 0u64;
    let mut has_epub_container = false;
    for i in 0..zip.len() {
        let f = zip.by_index_raw(i).map_err(|e| Rejected::BadArchive(e.to_string()))?;
        if f.enclosed_name().is_none() {
            return Err(Rejected::UnsafeArchivePath(f.name().to_string()));
        }
        let (size, compressed) = (f.size(), f.compressed_size());
        if size > RATIO_FLOOR && size / compressed.max(1) > limits.max_zip_ratio {
            return Err(Rejected::ZipBomb(format!(
                "{} unpacks {compressed} bytes to {size}",
                f.name()
            )));
        }
        unpacked = unpacked.saturating_add(size);
        packed = packed.saturating_add(compressed);
        has_epub_container |= f.name() == "META-INF/container.xml";
    }
    if unpacked > limits.max_zip_unpacked {
        return Err(Rejected::ZipBomb(format!(
            "unpacks to {unpacked} bytes, more than {}",
            limits.max_zip_unpacked
        )));
    }
    if unpacked > RATIO_FLOOR && unpacked / packed.max(1) > limits.max_zip_ratio {
        return Err(Rejected::ZipBomb(format!("unpacks {packed} bytes to {unpacked}")));
    }
    Ok(Summary { has_epub_container })
}

fn open(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, Rejected> {
    ZipArchive::new(Cursor::new(bytes)).map_err(|e| Rejected::BadArchive(e.to_string()))
}

fn le16(b: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u64)
}

fn le32(b: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as u64)
}

fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Entry count and central directory size from the end-of-central-directory record (or its
/// zip64 version).
fn end_record(b: &[u8]) -> Result<(u64, u64), Rejected> {
    let bad = |why: &str| Rejected::BadArchive(why.into());
    if b.len() < 22 {
        return Err(bad("too short"));
    }
    // The record is 22 bytes plus a comment of up to 65535 bytes, at the very end.
    let lowest = b.len().saturating_sub(22 + 65535);
    let pos = (lowest..=b.len() - 22)
        .rev()
        .find(|&i| b[i..].starts_with(b"PK\x05\x06"))
        .ok_or_else(|| bad("no end record"))?;
    let entries = le16(b, pos + 10).ok_or_else(|| bad("end record"))?;
    let directory = le32(b, pos + 12).ok_or_else(|| bad("end record"))?;
    if entries != 0xffff && directory != 0xffff_ffff {
        return Ok((entries, directory));
    }
    let locator = pos
        .checked_sub(20)
        .filter(|&l| b[l..].starts_with(b"PK\x06\x07"))
        .ok_or_else(|| bad("zip64 locator"))?;
    let record = le64(b, locator + 8).ok_or_else(|| bad("zip64 locator"))? as usize;
    if !b.get(record..).is_some_and(|r| r.starts_with(b"PK\x06\x06")) {
        return Err(bad("zip64 end record"));
    }
    let entries = le64(b, record + 32).ok_or_else(|| bad("zip64 end record"))?;
    let directory = le64(b, record + 40).ok_or_else(|| bad("zip64 end record"))?;
    Ok((entries, directory))
}

/// Reads one entry, refusing to unpack more than `max` bytes or more than is left of `budget`,
/// whatever the entry claims.
fn read_entry(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
    max: u64,
    budget: &mut u64,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ExtractError::Malformed(format!("{name}: {e}"))),
    };
    let cap = max.min(*budget);
    if f.size() > cap {
        return Err(Rejected::ZipBomb(format!("{name} is {} bytes unpacked", f.size())).into());
    }
    let mut out = Vec::with_capacity(f.size().min(MIB) as usize);
    f.take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|e| ExtractError::Malformed(format!("{name}: {e}")))?;
    if out.len() as u64 > cap {
        return Err(Rejected::ZipBomb(format!("{name} unpacks to more than it claims")).into());
    }
    *budget -= out.len() as u64;
    Ok(Some(out))
}

pub(crate) struct Book {
    pub(crate) title: Option<String>,
    pub(crate) chapters: u32,
}

/// Reads an EPUB's chapters in reading order (the OPF spine) into `doc`.
pub(crate) fn epub_to_blocks(
    bytes: &[u8],
    limits: &Limits,
    doc: &mut DocBuilder,
    cancel: &Cancel,
) -> Result<Book, ExtractError> {
    check(bytes, limits)?;
    let mut zip = open(bytes)?;
    let mut budget = limits.max_zip_unpacked;
    let missing = |what: &str| ExtractError::Malformed(format!("EPUB has no {what}"));

    let container = read_entry(&mut zip, "META-INF/container.xml", MAX_CONTAINER_BYTES, &mut budget)?
        .ok_or_else(|| missing("META-INF/container.xml"))?;
    let container = text::decode(&container).0;
    let opf_path = Tokenizer::new(&container)
        .find_map(|t| match t {
            Token::Start(name, attrs, _) if local_name(&name) == "rootfile" => {
                attr(&attrs, "full-path").map(String::from)
            }
            _ => None,
        })
        .ok_or_else(|| missing("rootfile"))?;
    let opf_path = opf_path.trim_start_matches('/').to_string();
    let opf = read_entry(&mut zip, &opf_path, MAX_OPF_BYTES, &mut budget)?.ok_or_else(|| missing(&opf_path))?;
    let package = parse_opf(&text::decode(&opf).0);
    let base = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir);

    let mut paths: Vec<String> = package
        .spine
        .iter()
        .filter_map(|id| package.manifest.get(id))
        // XHTML chapters; a missing media type is given the benefit of the doubt.
        .filter(|(_, media)| media.contains("html") || media.is_empty())
        .map(|(href, _)| resolve(base, href))
        .collect();
    if paths.is_empty() {
        // No usable spine: fall back to every XHTML file in name order.
        let mut names: Vec<String> = zip
            .file_names()
            .filter(|n| {
                let n = n.to_ascii_lowercase();
                n.ends_with(".xhtml") || n.ends_with(".html") || n.ends_with(".htm")
            })
            .map(String::from)
            .collect();
        names.sort();
        paths = names;
    }

    let mut chapters = 0;
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        cancel.check()?;
        if doc.is_full() {
            break;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(chapter) = read_entry(&mut zip, &path, MAX_CHAPTER_BYTES, &mut budget)? else {
            continue;
        };
        html::to_blocks(&text::decode(&chapter).0, doc);
        chapters += 1;
    }
    Ok(Book {
        title: package.title,
        chapters,
    })
}

struct Package {
    title: Option<String>,
    /// id → (href, media type)
    manifest: HashMap<String, (String, String)>,
    /// Item ids in reading order.
    spine: Vec<String>,
}

fn parse_opf(opf: &str) -> Package {
    let mut package = Package {
        title: None,
        manifest: HashMap::new(),
        spine: Vec::new(),
    };
    let mut in_title = false;
    for token in Tokenizer::new(opf) {
        match token {
            Token::Start(name, attrs, self_closing) => match local_name(&name) {
                "title" => in_title = !self_closing,
                "item" => {
                    if let (Some(id), Some(href)) = (attr(&attrs, "id"), attr(&attrs, "href")) {
                        let media = attr(&attrs, "media-type").unwrap_or("").to_ascii_lowercase();
                        package.manifest.insert(id.to_string(), (href.to_string(), media));
                    }
                }
                "itemref" => {
                    if let Some(id) = attr(&attrs, "idref") {
                        package.spine.push(id.to_string());
                    }
                }
                _ => {}
            },
            Token::Raw(t) if in_title => {
                in_title = false;
                if package.title.is_none() {
                    package.title = Some(text::squash_spaces(&html::decode_entities(t)));
                }
            }
            _ => {}
        }
    }
    package
}

/// Resolves a manifest `href` against the OPF's folder: drops the `#fragment`, decodes `%xx`
/// and folds `.` and `..`. The result is only used to look entries up, never as a disk path.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_hrefs() {
        assert_eq!(resolve("OEBPS", "text/ch%201.xhtml#start"), "OEBPS/text/ch 1.xhtml");
        assert_eq!(resolve("OEBPS/a", "../b/c.xhtml"), "OEBPS/b/c.xhtml");
        assert_eq!(resolve("", "./c.xhtml"), "c.xhtml");
        assert_eq!(resolve("x", "../../../c.xhtml"), "c.xhtml");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%4"), "%4");
    }

    #[test]
    fn end_record_needs_signature() {
        assert!(matches!(
            end_record(b"PK\x03\x04 not a zip at all......"),
            Err(Rejected::BadArchive(_))
        ));
    }
}
