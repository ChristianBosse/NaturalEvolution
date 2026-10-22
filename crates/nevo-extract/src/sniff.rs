//! File type detection from content ("magic bytes"), never from names or browser claims.

use crate::text;

/// How many leading bytes [`sniff`] looks at. Callers that stream uploads should pass at least
/// this much. It is enough to get past large JPEG metadata blocks to the image size.
pub const SNIFF_LEN: usize = 64 * 1024;

/// A file type NEVO knows how to treat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Pdf,
    Png,
    Jpeg,
    Gif,
    Webp,
    /// SVG can carry scripts, so it is never shown inline.
    Svg,
    /// MP4 or QuickTime video (ISO base media file).
    Mp4,
    /// MP4 audio (`.m4a`, `.m4b`).
    Mp4Audio,
    Webm,
    OggVideo,
    OggAudio,
    Mp3,
    Wav,
    Flac,
    Epub,
    /// Any other zip-based file (`.zip`, `.docx`, `.odt`, …): stored, not opened.
    Zip,
    Html,
    Markdown,
    Text,
    Zim,
    /// A program: Windows (MZ), Linux (ELF), macOS (Mach-O) or a script starting with `#!`.
    Executable,
    Unknown,
}

/// How a browser may receive a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Safe to show in the page: raster images, PDF, video, audio, plain text and Markdown
    /// (Markdown is served as plain text unless NEVO renders it).
    Inline,
    /// Download only, as `application/octet-stream`. HTML and SVG run scripts in the NEVO
    /// page's origin, so they are only ever shown as extracted text.
    Attachment,
}

impl Kind {
    /// The media type to store and serve.
    pub fn mime(self) -> &'static str {
        use Kind::*;
        match self {
            Pdf => "application/pdf",
            Png => "image/png",
            Jpeg => "image/jpeg",
            Gif => "image/gif",
            Webp => "image/webp",
            Svg => "image/svg+xml",
            Mp4 => "video/mp4",
            Mp4Audio => "audio/mp4",
            Webm => "video/webm",
            OggVideo => "video/ogg",
            OggAudio => "audio/ogg",
            Mp3 => "audio/mpeg",
            Wav => "audio/wav",
            Flac => "audio/flac",
            Epub => "application/epub+zip",
            Zip => "application/zip",
            Html => "text/html",
            Markdown => "text/markdown",
            Text => "text/plain",
            Zim => "application/x-zim",
            Executable | Unknown => "application/octet-stream",
        }
    }

    pub fn disposition(self) -> Disposition {
        use Kind::*;
        match self {
            Pdf | Png | Jpeg | Gif | Webp | Mp4 | Mp4Audio | Webm | OggVideo | OggAudio | Mp3 | Wav | Flac
            | Markdown | Text => Disposition::Inline,
            Svg | Epub | Zip | Html | Zim | Executable | Unknown => Disposition::Attachment,
        }
    }

    /// Images whose header gives a size that is checked against the pixel limits.
    pub fn is_raster_image(self) -> bool {
        matches!(self, Kind::Png | Kind::Jpeg | Kind::Gif | Kind::Webp)
    }

    /// Kinds [`extract`](crate::extract) can read text from.
    pub fn is_extractable(self) -> bool {
        matches!(self, Kind::Pdf | Kind::Epub | Kind::Html | Kind::Markdown | Kind::Text)
    }
}

/// The character encoding of a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// ISO 8859-1, read as its superset Windows-1252 (what "Latin-1" files usually are).
    Latin1,
}

/// What [`sniff`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    pub kind: Kind,
    /// Set for HTML, Markdown and plain text.
    pub encoding: Option<TextEncoding>,
}

/// Finds a file's type from its first bytes (pass at least [`SNIFF_LEN`], or the whole file).
/// `name` only picks Markdown over plain text and EPUB over zip; it never changes a binary
/// type.
pub fn sniff(head: &[u8], name: Option<&str>) -> Detected {
    let head = &head[..head.len().min(SNIFF_LEN)];
    let ext = name.map(extension).unwrap_or_default();
    let binary = |kind| Detected { kind, encoding: None };

    if head.starts_with(b"%PDF-") || find(&head[..head.len().min(1024)], b"%PDF-").is_some() {
        // Readers accept a PDF header anywhere in the first kilobyte.
        return binary(Kind::Pdf);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return binary(Kind::Png);
    }
    if head.starts_with(b"\xff\xd8\xff") {
        return binary(Kind::Jpeg);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return binary(Kind::Gif);
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" {
        match &head[8..12] {
            b"WEBP" => return binary(Kind::Webp),
            b"WAVE" => return binary(Kind::Wav),
            _ => {}
        }
    }
    if head.starts_with(b"ZIM\x04") {
        return binary(Kind::Zim);
    }
    if head.starts_with(b"fLaC") {
        return binary(Kind::Flac);
    }
    if head.starts_with(b"\x1a\x45\xdf\xa3") {
        // Matroska and WebM share the EBML header; browsers play WebM, often Matroska too.
        return binary(Kind::Webm);
    }
    if head.starts_with(b"OggS") {
        let video = find(head, b"\x80theora").is_some() || find(head, b"\x01video").is_some();
        return binary(if video { Kind::OggVideo } else { Kind::OggAudio });
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return binary(iso_media(&head[8..12]));
    }
    if head.starts_with(b"ID3") || is_mp3_frame(head) {
        return binary(Kind::Mp3);
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        let epub = head.len() >= 58 && &head[30..38] == b"mimetype" && &head[38..58] == b"application/epub+zip";
        // EPUBs must start with an uncompressed `mimetype` entry; `check` also accepts an
        // `.epub` whose entries are in another order but that has `META-INF/container.xml`.
        return binary(if epub { Kind::Epub } else { Kind::Zip });
    }
    if head.starts_with(b"MZ")
        || head.starts_with(b"\x7fELF")
        || head.starts_with(b"\xfe\xed\xfa\xce")
        || head.starts_with(b"\xfe\xed\xfa\xcf")
        || head.starts_with(b"\xce\xfa\xed\xfe")
        || head.starts_with(b"\xcf\xfa\xed\xfe")
        || head.starts_with(b"#!")
    {
        return binary(Kind::Executable);
    }

    let Some((s, encoding)) = text::decode_head(head) else {
        return binary(Kind::Unknown);
    };
    let kind = classify_text(&s, &ext);
    Detected {
        kind,
        encoding: Some(encoding),
    }
}

fn classify_text(s: &str, ext: &str) -> Kind {
    let t = s.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n', '\x0c']);
    let lower: String = t.chars().take(1024).collect::<String>().to_ascii_lowercase();
    if lower.starts_with("<svg")
        || ((lower.starts_with("<?xml") || lower.starts_with("<!doctype svg")) && has_svg(&lower))
    {
        return Kind::Svg;
    }
    if lower.starts_with("<?xml") && lower.contains("<html") {
        return Kind::Html;
    }
    // The tags browsers use to recognize HTML (WHATWG MIME sniffing), plus a few common ones.
    const HTML_STARTS: &[&str] = &[
        "<!doctype html",
        "<html",
        "<head",
        "<script",
        "<iframe",
        "<h1",
        "<h2",
        "<div",
        "<font",
        "<table",
        "<a",
        "<style",
        "<title",
        "<b",
        "<body",
        "<br",
        "<p",
        "<meta",
        "<!--",
    ];
    for tag in HTML_STARTS {
        if let Some(rest) = lower.strip_prefix(tag) {
            let ends = *tag == "<!--" || rest.starts_with([' ', '>', '\t', '\n', '\r', '/']);
            if ends {
                return Kind::Html;
            }
        }
    }
    if matches!(ext, "md" | "markdown" | "mdown" | "mkd") || looks_like_markdown(s) {
        Kind::Markdown
    } else {
        Kind::Text
    }
}

fn has_svg(lower: &str) -> bool {
    match (lower.find("<svg"), lower.find("<html")) {
        (Some(svg), Some(html)) => svg < html,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Markdown without a `.md` name: needs a heading and a few other Markdown marks, so ordinary
/// prose with a stray `#` stays plain text.
fn looks_like_markdown(s: &str) -> bool {
    let mut headings = 0;
    let mut marks = 0;
    for line in s.lines().take(400) {
        let l = line.trim_start();
        let hashes = l.bytes().take_while(|&b| b == b'#').count();
        let block_mark = ["```", "- ", "* ", "> "].iter().any(|m| l.starts_with(m));
        // Setext heading underlines.
        let underline = (!l.is_empty() && l.chars().all(|c| c == '=')) || (l.len() >= 3 && l.chars().all(|c| c == '-'));
        if (1..=6).contains(&hashes) && l[hashes..].starts_with(' ') {
            headings += 1;
        } else if block_mark || underline {
            marks += 1;
        }
        if l.contains("](") || l.contains("**") {
            marks += 1;
        }
    }
    headings >= 1 && headings + marks >= 3
}

fn iso_media(brand: &[u8]) -> Kind {
    match brand {
        b"M4A " | b"M4B " | b"M4P " => Kind::Mp4Audio,
        b"isom" | b"iso2" | b"iso3" | b"iso4" | b"iso5" | b"iso6" | b"mp41" | b"mp42" | b"avc1" | b"dash" | b"M4V "
        | b"M4VH" | b"M4VP" | b"qt  " | b"mmp4" | b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" | b"f4v " | b"MSNV" => {
            Kind::Mp4
        }
        // HEIF/AVIF images and anything else in an ISO box: not something browsers reliably show.
        _ => Kind::Unknown,
    }
}

/// An MPEG audio frame header: 11 sync bits, a valid version, layer III and bitrate.
fn is_mp3_frame(head: &[u8]) -> bool {
    head.len() >= 4
        && head[0] == 0xff
        && head[1] & 0xe0 == 0xe0
        && head[1] & 0x18 != 0x08
        && head[1] & 0x06 == 0x02
        && head[2] & 0xf0 != 0xf0
        && head[2] & 0x0c != 0x0c
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Lowercase extension of the last path part, ignoring the trailing dots and spaces that
/// Windows drops (`evil.exe.` runs as `evil.exe`).
fn extension(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = base.split(':').next().unwrap_or(base).trim_end_matches(['.', ' ']);
    match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Extensions that run code when opened on a common desktop or phone.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "scr",
    "pif",
    "msi",
    "msp",
    "msc",
    "cpl",
    "dll",
    "sys",
    "drv",
    "ocx",
    "ps1",
    "psm1",
    "psd1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "mjs",
    "wsf",
    "wsh",
    "wsc",
    "hta",
    "lnk",
    "url",
    "reg",
    "inf",
    "scf",
    "jar",
    "apk",
    "aab",
    "xapk",
    "ipa",
    "app",
    "dmg",
    "pkg",
    "deb",
    "rpm",
    "appimage",
    "run",
    "bin",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "fish",
    "command",
    "desktop",
    "so",
    "dylib",
    "elf",
    "py",
    "pyw",
    "pl",
    "rb",
    "php",
    "action",
    "workflow",
    "applescript",
    "scpt",
    "gadget",
    "msix",
    "appx",
    "appxbundle",
    "vb",
    "xll",
    "xlam",
    "docm",
    "xlsm",
    "pptm",
];

/// True if the name ends in a program or script extension (`.exe`, `.bat`, `.sh`, …).
pub fn is_executable_name(name: &str) -> bool {
    let ext = extension(name);
    EXECUTABLE_EXTENSIONS.contains(&ext.as_str())
}

/// A file name safe to store and show: the last path part only, without control characters,
/// path separators or characters Windows forbids, trimmed, at most 200 characters, and never
/// a reserved Windows device name. Empty names become `file`.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) || c == '\u{202e}' {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim_matches([' ', '.']).to_string();
    if out.chars().count() > 200 {
        // Keep the extension, so the type stays recognizable.
        let ext = extension(&out);
        let keep = 200 - ext.chars().count().min(20) - 1;
        let stem: String = out.chars().take(keep).collect();
        out = if ext.is_empty() || ext.chars().count() > 20 {
            out.chars().take(200).collect()
        } else {
            format!("{stem}.{ext}")
        };
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    let numbered =
        (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit();
    if RESERVED.contains(&stem.as_str()) || numbered {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out = "file".into();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert!(is_executable_name("setup.EXE"));
        assert!(is_executable_name("photo.jpg.exe"));
        assert!(is_executable_name("evil.bat. . "));
        assert!(is_executable_name("dir/run.sh"));
        assert!(!is_executable_name("photo.jpg"));
        assert!(!is_executable_name("exe"));
        assert!(!is_executable_name(".bashrc"));
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\a\\report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name("a\u{0}b\nc.txt"), "a_b_c.txt");
        assert_eq!(safe_file_name("..."), "file");
        assert_eq!(safe_file_name("cv\u{202e}fdp.exe"), "cv_fdp.exe");
        let long = format!("{}.pdf", "x".repeat(300));
        let s = safe_file_name(&long);
        assert_eq!(s.chars().count(), 200);
        assert!(s.ends_with(".pdf"));
    }

    #[test]
    fn magic() {
        let k = |b: &[u8]| sniff(b, None).kind;
        assert_eq!(k(b"%PDF-1.7\n"), Kind::Pdf);
        assert_eq!(k(b"\x89PNG\r\n\x1a\n...."), Kind::Png);
        assert_eq!(k(b"\xff\xd8\xff\xe0"), Kind::Jpeg);
        assert_eq!(k(b"GIF89a"), Kind::Gif);
        assert_eq!(k(b"RIFF\0\0\0\0WEBPVP8 "), Kind::Webp);
        assert_eq!(k(b"RIFF\0\0\0\0WAVEfmt "), Kind::Wav);
        assert_eq!(k(b"\0\0\0\x18ftypmp42\0\0\0\0"), Kind::Mp4);
        assert_eq!(k(b"\0\0\0\x18ftypM4A \0\0\0\0"), Kind::Mp4Audio);
        assert_eq!(k(b"\0\0\0\x18ftypheic\0\0\0\0"), Kind::Unknown);
        assert_eq!(k(b"\x1a\x45\xdf\xa3"), Kind::Webm);
        assert_eq!(k(b"OggS\0\x02....\x01vorbis"), Kind::OggAudio);
        assert_eq!(k(b"OggS\0\x02....\x80theora"), Kind::OggVideo);
        assert_eq!(k(b"ID3\x04"), Kind::Mp3);
        assert_eq!(k(b"\xff\xfb\x90\x64"), Kind::Mp3);
        assert_eq!(k(b"ZIM\x04\x05\0"), Kind::Zim);
        assert_eq!(k(b"MZ\x90\0"), Kind::Executable);
        assert_eq!(k(b"\x7fELF\x02"), Kind::Executable);
        assert_eq!(k(b"#!/bin/sh\nrm -rf /\n"), Kind::Executable);
        assert_eq!(k(b"\0\x01\x02\x03binary"), Kind::Unknown);
    }

    #[test]
    fn text_kinds() {
        let k = |b: &[u8], n: Option<&str>| sniff(b, n).kind;
        assert_eq!(k(b"<!DOCTYPE html><html>", None), Kind::Html);
        assert_eq!(k(b"\xef\xbb\xbf  <p>hi</p>", None), Kind::Html);
        assert_eq!(k(b"<?xml version=\"1.0\"?>\n<html xmlns=\"x\">", None), Kind::Html);
        assert_eq!(k(b"<svg xmlns=\"http://www.w3.org/2000/svg\">", None), Kind::Svg);
        assert_eq!(k(b"<?xml version=\"1.0\"?><svg>", None), Kind::Svg);
        assert_eq!(k(b"just some notes", None), Kind::Text);
        assert_eq!(k(b"just some notes", Some("notes.md")), Kind::Markdown);
        assert_eq!(k(b"# Title\n\n- one\n- two\n\nSee [x](y).", None), Kind::Markdown);
        assert_eq!(k(b"# not a heading in a script", None), Kind::Text);
        // A name never turns binary into text.
        assert_eq!(k(b"\0\x01\x02\x03", Some("a.md")), Kind::Unknown);
        assert_eq!(k(b"%PDF-1.4", Some("a.txt")), Kind::Pdf);
    }
}
