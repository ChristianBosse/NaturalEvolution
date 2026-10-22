//! NEVO import safety checks and text extraction.
//!
//! Everything people add (USB or Wi-Fi upload) goes through two steps:
//!
//! 1. [`check`] (or [`check_head`] for files too big to read into memory) finds the real type from
//!    the file's first bytes, never from its name or the browser's claim, and rejects files that
//!    break a safety limit: size per type, zip bombs, image dimension bombs, unreadable headers.
//!    It also says whether the file may be shown inline in a browser ([`Disposition`]).
//! 2. [`extract`] turns PDF, EPUB, HTML, Markdown and plain text into clean text plus passages
//!    of about 500–1500 characters for search and the AI. The result is meant to be stored as
//!    derived data (`t: "derived"` in the content-model spec) with [`PRODUCER`] as its producer.
//!
//! ```
//! use nevo_extract::{check, extract, Limits};
//! let limits = Limits::default();
//! let bytes = b"# Boiling water\n\nBring water to a rolling boil for one minute.".to_vec();
//! let checked = check(&bytes, Some("water.md"), &limits).unwrap();
//! assert_eq!(checked.mime(), "text/markdown");
//! let out = extract(bytes, checked.kind, &limits).unwrap();
//! assert_eq!(out.passages[0].heading.as_deref(), Some("Boiling water"));
//! ```
//!
//! ## Clean text format
//!
//! [`Extracted::text`] is UTF-8 made of blocks separated by one blank line. A block is a heading
//! (`#` to `######`, a space, then one line), a page break (a lone form feed, PDF only) or a
//! paragraph. Paragraph lines that start with `#` or `\` get a `\` in front. The format is
//! simple on purpose: [`passages`] recomputes the passages from stored text, so only the text
//! needs to be kept.
//!
//! ## What the guards cover, and what they don't
//!
//! Extraction runs on its own thread with a deadline. A panic in a parser (pdf-extract and lopdf
//! have many `unwrap`s on malformed input) is caught and becomes [`ExtractError::Panicked`]; a
//! PDF page that panics is skipped and the other pages are kept. Known unbounded recursions in
//! the PDF code (page `Parent` loops, form XObjects that draw themselves) are checked for before
//! parsing, because a stack overflow can't be caught: it aborts the whole process.
//!
//! Limits: a thread can't be killed, so after a timeout the worker keeps running until its next
//! check of the cancel flag (every drawn character for PDFs). A hang inside lopdf's loader, before
//! any page is drawn, is not interrupted. Memory isn't capped either: lopdf keeps the whole parsed PDF
//! in memory; the working memory measured 4–10x the file size (see `tests/pdf_bench.rs`). Panics are only caught when
//! the program is built with `panic = "unwind"` (Rust's default). Full isolation would need a
//! child process, which the node can add later by running extraction as a sub-command.
//!
//! PDF JavaScript is never run: no code here executes anything from a file, it only reads text.
//! [`Checked::active_content`] still reports PDFs that carry scripts, so the node can warn.

mod archive;
mod error;
mod guard;
mod html;
mod image;
mod pdf;
mod sniff;
mod text;

use std::time::Duration;

pub use error::{ExtractError, Rejected};
pub use image::Dimensions;
pub use sniff::{is_executable_name, safe_file_name, sniff, Detected, Disposition, Kind, TextEncoding, SNIFF_LEN};
pub use text::{passages, Passage, MAX_PASSAGE, MIN_PASSAGE};

/// The `producer` of derived data made by this crate. Bump it whenever the output for the same
/// input changes, so nodes know to recompute instead of mixing old and new text.
pub const PRODUCER: &str = "nevo-extract/0.1";

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

/// Safety limits. The defaults suit a 4 GB node; Seed (a desktop) can raise them.
#[derive(Debug, Clone)]
pub struct Limits {
    /// PDF and EPUB files.
    pub max_document_bytes: u64,
    /// HTML, Markdown and plain text.
    pub max_text_bytes: u64,
    /// Raster images and SVG.
    pub max_image_bytes: u64,
    /// Video and audio.
    pub max_media_bytes: u64,
    /// ZIM packs. Wikipedia in English with pictures is about 110 GB.
    pub max_zim_bytes: u64,
    /// Plain zip archives, executables and unknown types (stored, never opened).
    pub max_other_bytes: u64,
    /// Largest width × height. Decoding takes about 4 bytes per pixel, so 64 million pixels is
    /// 256 MB, already a lot next to the AI model.
    pub max_image_pixels: u64,
    /// Largest width or height alone.
    pub max_image_side: u32,
    /// Most entries in a zip or EPUB.
    pub max_zip_entries: u64,
    /// Most uncompressed bytes in a zip or EPUB, all entries together.
    pub max_zip_unpacked: u64,
    /// Highest uncompressed / compressed ratio, for any entry over 1 MiB and for the whole file.
    /// Text compresses about 3–10x; zip bombs reach 1000x and more.
    pub max_zip_ratio: u64,
    /// Extraction stops after this many characters of text and sets `truncated`.
    pub max_text_chars: usize,
    /// Extraction reads at most this many PDF pages, then sets `truncated`.
    pub max_pdf_pages: u32,
    /// Extraction gives up after this long.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_document_bytes: 512 * MIB,
            max_text_bytes: 32 * MIB,
            max_image_bytes: 64 * MIB,
            max_media_bytes: 16 * GIB,
            max_zim_bytes: 256 * GIB,
            max_other_bytes: 4 * GIB,
            max_image_pixels: 64_000_000,
            max_image_side: 30_000,
            max_zip_entries: 20_000,
            max_zip_unpacked: GIB,
            max_zip_ratio: 100,
            max_text_chars: 16_000_000,
            max_pdf_pages: 5_000,
            timeout: Duration::from_secs(300),
        }
    }
}

impl Limits {
    /// The largest accepted file of this kind.
    pub fn max_bytes(&self, kind: Kind) -> u64 {
        use Kind::*;
        match kind {
            Pdf | Epub => self.max_document_bytes,
            Html | Markdown | Text => self.max_text_bytes,
            Png | Jpeg | Gif | Webp | Svg => self.max_image_bytes,
            Mp4 | Mp4Audio | Webm | OggVideo | OggAudio | Mp3 | Wav | Flac => self.max_media_bytes,
            Zim => self.max_zim_bytes,
            Zip | Executable | Unknown => self.max_other_bytes,
        }
    }
}

/// A file that passed the safety checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub kind: Kind,
    /// Set for text kinds (HTML, Markdown, plain text).
    pub encoding: Option<TextEncoding>,
    /// Width and height, for raster images.
    pub dimensions: Option<Dimensions>,
    /// How a browser may show it. Executable names always get [`Disposition::Attachment`].
    pub disposition: Disposition,
    /// The name looks like a program (`.exe`, `.sh`, …). It may be stored but never shown inline.
    pub executable_name: bool,
    /// Script or launch actions found in a PDF (best effort: names inside compressed object
    /// streams are not seen). Nothing here runs them; this is for warning people.
    pub active_content: bool,
    /// False when only the head was checked ([`check_head`]) and the file is an archive whose
    /// entries were not looked at. [`extract`] checks EPUBs again anyway.
    pub archive_checked: bool,
}

impl Checked {
    /// The trusted media type, to store and serve instead of what the browser claimed.
    /// Executable names get `application/octet-stream` whatever their content.
    pub fn mime(&self) -> &'static str {
        if self.executable_name {
            "application/octet-stream"
        } else {
            self.kind.mime()
        }
    }
}

/// Checks a whole file held in memory. `name` is the file name people gave, if any: it only
/// tells Markdown from plain text, finds EPUBs whose `mimetype` entry is not first, and flags
/// executable names. It never turns a binary file into a type that is shown inline.
pub fn check(bytes: &[u8], name: Option<&str>, limits: &Limits) -> Result<Checked, Rejected> {
    let mut checked = check_head(bytes, bytes.len() as u64, name, limits)?;
    if matches!(checked.kind, Kind::Zip | Kind::Epub) {
        let summary = archive::check(bytes, limits)?;
        if checked.kind == Kind::Zip && summary.has_epub_container && name.is_some_and(has_epub_extension) {
            checked.kind = Kind::Epub;
            checked.disposition = disposition_for(&checked);
        }
        checked.archive_checked = true;
    }
    if checked.kind == Kind::Pdf {
        checked.active_content = pdf::has_active_content(bytes);
    }
    Ok(checked)
}

/// Checks a file from its first bytes and total size, for files that are streamed to disk
/// (video, ZIM). Pass at least [`SNIFF_LEN`] bytes, or the whole file if it is shorter. Zip
/// entries are not checked ([`Checked::archive_checked`] is false).
pub fn check_head(head: &[u8], size: u64, name: Option<&str>, limits: &Limits) -> Result<Checked, Rejected> {
    let detected = sniff(head, name);
    let max = limits.max_bytes(detected.kind);
    if size > max {
        return Err(Rejected::TooLarge {
            kind: detected.kind,
            size,
            max,
        });
    }
    let dimensions = if detected.kind.is_raster_image() {
        let d = image::dimensions(detected.kind, head).ok_or(Rejected::BadImageHeader)?;
        if d.width == 0 || d.height == 0 {
            return Err(Rejected::BadImageHeader);
        }
        let pixels = d.width as u64 * d.height as u64;
        if d.width > limits.max_image_side || d.height > limits.max_image_side || pixels > limits.max_image_pixels {
            return Err(Rejected::ImageTooLarge {
                width: d.width,
                height: d.height,
            });
        }
        Some(d)
    } else {
        None
    };
    let mut checked = Checked {
        kind: detected.kind,
        encoding: detected.encoding,
        dimensions,
        disposition: Disposition::Attachment,
        executable_name: name.is_some_and(is_executable_name),
        active_content: false,
        archive_checked: false,
    };
    checked.disposition = disposition_for(&checked);
    Ok(checked)
}

fn disposition_for(checked: &Checked) -> Disposition {
    if checked.executable_name {
        Disposition::Attachment
    } else {
        checked.kind.disposition()
    }
}

fn has_epub_extension(name: &str) -> bool {
    name.to_ascii_lowercase()
        .trim_end_matches(['.', ' '])
        .ends_with(".epub")
}

/// Text extracted from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// Always [`PRODUCER`]; store it as the derived data's `producer`.
    pub producer: &'static str,
    /// The document's own title (HTML `<title>`, EPUB metadata, PDF info), if it has one.
    pub title: Option<String>,
    /// Clean text in the format described in the crate docs. Store this as the derived blob.
    pub text: String,
    /// The text cut into passages for search and the AI.
    pub passages: Vec<Passage>,
    /// Number of PDF pages, or EPUB chapters read.
    pub pages: Option<u32>,
    /// A limit stopped extraction early (too much text, too many pages); `text` is the start.
    pub truncated: bool,
    /// PDF pages skipped because the parser failed on them.
    pub failed_pages: u32,
}

/// Extracts text from a file already accepted by [`check`]. `kind` is [`Checked::kind`]. Takes
/// the bytes by value because the worker thread may outlive a timeout (see the crate docs).
pub fn extract(bytes: Vec<u8>, kind: Kind, limits: &Limits) -> Result<Extracted, ExtractError> {
    if !kind.is_extractable() {
        return Err(ExtractError::Unsupported(kind));
    }
    let limits = limits.clone();
    let timeout = limits.timeout;
    guard::run(timeout, move |cancel| {
        let mut doc = text::DocBuilder::new(limits.max_text_chars);
        let mut title = None;
        let mut pages = None;
        let mut failed_pages = 0;
        match kind {
            Kind::Text | Kind::Markdown => {
                let (s, _) = text::decode(&bytes);
                if kind == Kind::Markdown {
                    text::markdown_to_blocks(&s, &mut doc);
                } else {
                    text::plain_to_blocks(&s, &mut doc);
                }
            }
            Kind::Html => {
                let (s, _) = text::decode(&bytes);
                title = html::to_blocks(&s, &mut doc);
            }
            Kind::Epub => {
                let book = archive::epub_to_blocks(&bytes, &limits, &mut doc, cancel)?;
                title = book.title;
                pages = Some(book.chapters);
            }
            Kind::Pdf => {
                let info = pdf::to_blocks(&bytes, &limits, &mut doc, cancel)?;
                title = info.title;
                pages = Some(info.pages);
                failed_pages = info.failed_pages;
                if info.truncated {
                    doc.truncated = true;
                }
            }
            _ => return Err(ExtractError::Unsupported(kind)),
        }
        let truncated = doc.truncated;
        let text = doc.finish();
        let passages = passages(&text);
        Ok(Extracted {
            producer: PRODUCER,
            title: title.map(|t| text::squash_spaces(&t)).filter(|t| !t.is_empty()),
            text,
            passages,
            pages,
            truncated,
            failed_pages,
        })
    })
}
