//! Fixture builders: small but real PDFs, zips, EPUBs and image headers, made in code so no
//! binary files live in the repository.

#![allow(dead_code)]

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Escapes a PDF literal string. Characters above ASCII are written as Latin-1 octal escapes
/// (the font uses WinAnsiEncoding).
fn pdf_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 128 => out.push(c),
            c if (c as u32) < 256 => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }
    out
}

/// One page: an optional heading in 18 pt, then paragraphs of lines in 11 pt.
pub struct Page {
    pub heading: Option<String>,
    pub paragraphs: Vec<Vec<String>>,
}

/// The content stream for a page: 14 pt line spacing, an extra blank line between paragraphs.
pub fn page_content(page: &Page) -> String {
    let mut ops = String::new();
    let mut y = 760;
    if let Some(h) = &page.heading {
        ops.push_str(&format!("BT /F1 18 Tf 72 {y} Td ({}) Tj ET\n", pdf_string(h)));
        y -= 36;
    }
    for para in &page.paragraphs {
        for line in para {
            ops.push_str(&format!("BT /F1 11 Tf 72 {y} Td ({}) Tj ET\n", pdf_string(line)));
            y -= 14;
        }
        y -= 14;
    }
    ops
}

/// A PDF from raw page content streams. `page_extra` is added to every page dictionary;
/// `extra_objects` are appended as objects 5, 6, … (after catalog 1, pages 2, font 3, info 4).
pub fn pdf_raw(contents: &[String], title: &str, page_extra: &str, extra_objects: &[String]) -> Vec<u8> {
    let n = contents.len();
    let first_page = 5 + extra_objects.len();
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!(
            "<< /Type /Pages /Count {n} /Kids [{}] >>",
            (0..n)
                .map(|i| format!("{} 0 R", first_page + 2 * i))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Title ({}) /Producer (nevo-extract tests) >>", pdf_string(title)),
    ];
    objects.extend(extra_objects.iter().cloned());
    for (i, content) in contents.iter().enumerate() {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> \
             /Contents {} 0 R {page_extra} >>",
            first_page + 2 * i + 1
        ));
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

pub fn pdf(pages: &[Page], title: &str) -> Vec<u8> {
    let contents: Vec<String> = pages.iter().map(page_content).collect();
    pdf_raw(&contents, title, "", &[])
}

const WORDS: &[&str] = &[
    "water", "boil", "filter", "clean", "cloth", "sand", "charcoal", "minute", "bucket", "germs", "safe", "drink",
    "store", "cover", "pot", "fire", "heat", "cool", "family", "village", "well", "rain", "roof", "tank",
];

/// A book of `pages` pages, each with a heading and about 2800 characters of text in
/// paragraphs of 3–6 lines.
pub fn book(pages: usize) -> Vec<Page> {
    let mut seed = 7u32;
    let mut word = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        WORDS[(seed >> 16) as usize % WORDS.len()]
    };
    (1..=pages)
        .map(|p| {
            let mut paragraphs = Vec::new();
            let mut lines_left = 44;
            let mut k = 0;
            while lines_left > 0 {
                let n = (3 + k % 4).min(lines_left);
                let para = (0..n)
                    .map(|_| {
                        let mut line = String::new();
                        while line.len() < 70 {
                            line.push_str(word());
                            line.push(' ');
                        }
                        line.trim_end().to_string()
                    })
                    .collect();
                paragraphs.push(para);
                lines_left -= n;
                k += 1;
            }
            Page {
                heading: Some(format!("Chapter {p}")),
                paragraphs,
            }
        })
        .collect()
}

/// A zip with the given entries, in order. `None` content means a directory.
pub fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    for &(name, data, compress) in entries {
        let method = if compress {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        };
        w.start_file(name, SimpleFileOptions::default().compression_method(method))
            .unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

pub const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

pub fn chapter(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\">\
         <head><title>{title}</title></head><body><h1>{title}</h1><p>{body}</p></body></html>"
    )
}

/// An EPUB whose zip order, manifest order and spine order all differ: the spine says
/// One, Two, Three.
pub fn epub() -> Vec<u8> {
    let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Safe &amp; Clean Water</dc:title><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c3" href="text/three.xhtml" media-type="application/xhtml+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/one%20a.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/two.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/><itemref idref="css"/></spine>
</package>"#;
    let one = chapter("One", "Collect rain water from a clean roof.");
    let two = chapter("Two", "Filter it through cloth, sand and charcoal.");
    let three = chapter("Three", "Boil it for one minute before drinking.");
    zip(&[
        ("mimetype", b"application/epub+zip", false),
        ("META-INF/container.xml", CONTAINER.as_bytes(), true),
        ("OEBPS/text/two.xhtml", two.as_bytes(), true),
        ("OEBPS/text/three.xhtml", three.as_bytes(), true),
        ("OEBPS/content.opf", opf.as_bytes(), true),
        ("OEBPS/text/one a.xhtml", one.as_bytes(), true),
        (
            "OEBPS/nav.xhtml",
            b"<html><body><nav><ol><li>One</li></ol></nav></body></html>",
            true,
        ),
        ("OEBPS/style.css", b"p { margin: 0 }", true),
    ])
}

/// The first bytes of a PNG claiming the given size.
pub fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend(width.to_be_bytes());
    b.extend(height.to_be_bytes());
    b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.extend([0u8; 100]);
    b
}
