mod common;

use std::time::Duration;

use common::*;
use nevo_extract::{
    check, check_head, extract, passages, Disposition, ExtractError, Kind, Limits, Rejected, TextEncoding, PRODUCER,
};

fn limits() -> Limits {
    Limits::default()
}

// ---------------------------------------------------------------------------------------------
// PDF

#[test]
fn pdf_text_headings_pages_and_title() {
    let pages = vec![
        Page {
            heading: Some("Clean water".into()),
            paragraphs: vec![
                vec![
                    "Boiling is the surest way to make water safe. Bring it to a rolling".into(),
                    "boil for one minute.".into(),
                ],
                vec!["At high altitude, boil for three minutes. Let it cool in a covered pot.".into()],
            ],
        },
        Page {
            heading: None,
            paragraphs: vec![vec![
                "Store it in a clean container with a lid. Caf\u{e9} (test).".into()
            ]],
        },
        Page {
            heading: Some("Filters".into()),
            paragraphs: vec![vec!["Cloth, sand and charcoal remove dirt but not all germs.".into()]],
        },
    ];
    let bytes = pdf(&pages, "Water guide");
    let checked = check(&bytes, Some("guide.txt"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Pdf);
    assert_eq!(checked.mime(), "application/pdf");
    assert_eq!(checked.disposition, Disposition::Inline);
    assert!(!checked.active_content);

    let out = extract(bytes, Kind::Pdf, &limits()).unwrap();
    assert_eq!(out.producer, PRODUCER);
    assert_eq!(out.title.as_deref(), Some("Water guide"));
    assert_eq!(out.pages, Some(3));
    assert_eq!(out.failed_pages, 0);
    assert!(!out.truncated);
    assert_eq!(
        out.text,
        "# Clean water\n\n\
         Boiling is the surest way to make water safe. Bring it to a rolling boil for one minute.\n\n\
         At high altitude, boil for three minutes. Let it cool in a covered pot.\n\n\x0c\n\n\
         Store it in a clean container with a lid. Caf\u{e9} (test).\n\n\x0c\n\n\
         # Filters\n\nCloth, sand and charcoal remove dirt but not all germs."
    );
    assert_eq!(out.passages.len(), 2);
    assert_eq!(out.passages[0].heading.as_deref(), Some("Clean water"));
    assert_eq!(out.passages[0].page, Some(1));
    assert!(out.passages[0].text.ends_with("(test)."));
    assert_eq!(out.passages[1].heading.as_deref(), Some("Filters"));
    assert_eq!(out.passages[1].page, Some(3));
    // Passages can be recomputed from the stored text alone.
    assert_eq!(passages(&out.text), out.passages);
}

#[test]
fn truncated_pdf_never_panics_or_hangs() {
    let bytes = pdf(&book(3), "Cut");
    for cut in (0..bytes.len()).step_by(97).chain([bytes.len() - 10, bytes.len() - 1]) {
        let part = bytes[..cut].to_vec();
        // Any outcome but a crash or a hang is fine; lopdf recovers some cut files.
        let _ = extract(part, Kind::Pdf, &limits());
    }
    let half = bytes[..bytes.len() / 2].to_vec();
    match extract(half, Kind::Pdf, &limits()) {
        Ok(out) => assert!(out.text.is_empty() || out.text.contains("Chapter")),
        Err(e) => assert!(
            matches!(e, ExtractError::Malformed(_) | ExtractError::Panicked(_)),
            "{e}"
        ),
    }
}

#[test]
fn garbage_pdfs_are_errors() {
    let cases: Vec<Vec<u8>> = vec![
        b"%PDF-1.7\n".to_vec(),
        b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 1 0 R >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF".to_vec(),
        b"%PDF-1.4\nxref\n0 99999999\ntrailer << /Size 99999999 >>\nstartxref\n9\n%%EOF".to_vec(),
        [b"%PDF-1.4\n1 0 obj ".as_slice(), &[b'['; 100_000], b"\nendobj\n%%EOF"].concat(),
    ];
    for bytes in cases {
        assert!(matches!(
            extract(bytes, Kind::Pdf, &limits()),
            Err(ExtractError::Malformed(_))
        ));
    }
}

#[test]
fn page_that_panics_is_skipped() {
    // pdf-extract unwraps the operand of `Do`; a number there panics mid-page. The text drawn
    // before the panic and the other pages must survive.
    let mut contents: Vec<String> = book(3).iter().map(page_content).collect();
    contents[1] =
        "BT /F1 11 Tf 72 700 Td (Before the crash) Tj ET\n1 Do\nBT /F1 11 Tf 72 600 Td (After) Tj ET\n".into();
    let bytes = pdf_raw(&contents, "Mixed", "", &[]);
    let out = extract(bytes, Kind::Pdf, &limits()).unwrap();
    assert_eq!(out.pages, Some(3));
    assert_eq!(out.failed_pages, 1);
    assert!(out.text.contains("# Chapter 1"));
    assert!(out.text.contains("\x0c\n\nBefore the crash\n\n\x0c\n\n# Chapter 3"));
    assert!(!out.text.contains("After"));
}

#[test]
fn recursive_pdf_drawing_is_refused() {
    // A form XObject that draws itself would recurse until the stack overflows.
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /X 5 0 R >> >> \
                /Length 7 >>\nstream\n/X Do\n\nendstream"
        .to_string();
    let bytes = pdf_raw(&["/X Do\n".into()], "Loop", "", &[form]);
    let err = extract(bytes, Kind::Pdf, &limits()).unwrap_err();
    assert!(
        matches!(&err, ExtractError::Malformed(m) if m.contains("loops")),
        "{err}"
    );

    // The same through inherited resources: the form has none of its own.
    let form =
        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 7 >>\nstream\n/X Do\n\nendstream".to_string();
    let bytes = pdf_raw(
        &["/X Do\n".into()],
        "Loop",
        "/Resources << /XObject << /X 5 0 R >> >>",
        &[form],
    );
    assert!(extract(bytes, Kind::Pdf, &limits()).is_err());
}

#[test]
fn pdf_page_tree_loop_is_refused() {
    let mut bytes = pdf(&book(1), "Loop");
    // Point the page's Parent at itself (page is object 5).
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let at = text.find("/Parent 2 0 R").unwrap();
    bytes[at..at + 13].copy_from_slice(b"/Parent 5 0 R");
    // Without a MediaBox pdf-extract climbs Parent links to find one.
    let at = text.find("/MediaBox [0 0 612 792]").unwrap();
    bytes[at..at + 23].copy_from_slice(b"/Rotate 0              ");
    // lopdf may refuse the tree itself; either way, no stack overflow.
    let _ = extract(bytes, Kind::Pdf, &limits());
}

#[test]
fn pdf_limits() {
    let bytes = pdf(&book(5), "Long");
    let small = Limits {
        max_text_chars: 3000,
        ..limits()
    };
    let out = extract(bytes.clone(), Kind::Pdf, &small).unwrap();
    assert!(out.truncated);
    assert!(out.text.chars().count() <= 3000);

    let few = Limits {
        max_pdf_pages: 2,
        ..limits()
    };
    let out = extract(bytes.clone(), Kind::Pdf, &few).unwrap();
    assert!(out.truncated);
    assert!(out.text.contains("Chapter 2") && !out.text.contains("Chapter 3"));

    let hurry = Limits {
        timeout: Duration::ZERO,
        ..limits()
    };
    assert_eq!(extract(bytes, Kind::Pdf, &hurry).unwrap_err(), ExtractError::TimedOut);
}

#[test]
fn pdf_scripts_are_reported_not_run() {
    let script = "<< /S /JavaScript /JS (app.alert\\('hi'\\)) >>".to_string();
    let bytes = pdf_raw(&[page_content(&book(1)[0])], "Script", "/AA << /O 5 0 R >>", &[script]);
    let checked = check(&bytes, None, &limits()).unwrap();
    assert!(checked.active_content);
    let out = extract(bytes, Kind::Pdf, &limits()).unwrap();
    assert!(out.text.starts_with("# Chapter 1"));
}

// ---------------------------------------------------------------------------------------------
// Zip and EPUB

#[test]
fn epub_chapters_follow_the_spine() {
    let bytes = epub();
    let checked = check(&bytes, Some("water.zip"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Epub);
    assert_eq!(checked.mime(), "application/epub+zip");
    assert!(checked.archive_checked);

    let out = extract(bytes, Kind::Epub, &limits()).unwrap();
    assert_eq!(out.title.as_deref(), Some("Safe & Clean Water"));
    assert_eq!(out.pages, Some(3));
    assert_eq!(
        out.text,
        "# One\n\nCollect rain water from a clean roof.\n\n# Two\n\nFilter it through cloth, sand and charcoal.\n\n\
         # Three\n\nBoil it for one minute before drinking."
    );
    let headings: Vec<_> = out.passages.iter().map(|p| p.heading.clone().unwrap()).collect();
    assert_eq!(headings, ["One", "Two", "Three"]);
}

#[test]
fn epub_without_leading_mimetype_is_found_by_name() {
    let bytes = zip(&[
        ("META-INF/container.xml", CONTAINER.as_bytes(), true),
        ("OEBPS/content.opf", b"<package><manifest/><spine/></package>", true),
        ("OEBPS/a.xhtml", chapter("Only", "Text.").as_bytes(), true),
    ]);
    assert_eq!(check(&bytes, None, &limits()).unwrap().kind, Kind::Zip);
    let checked = check(&bytes, Some("book.epub"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Epub);
    // An empty spine falls back to the XHTML files in name order.
    let out = extract(bytes, Kind::Epub, &limits()).unwrap();
    assert_eq!(out.text, "# Only\n\nText.");
}

#[test]
fn zip_bombs_are_refused() {
    // 20 MB of zeros deflates to about 20 KB: a ratio near 1000.
    let zeros = vec![0u8; 20 * 1024 * 1024];
    let bomb = zip(&[
        ("mimetype", b"application/epub+zip", false),
        ("big.xhtml", &zeros, true),
    ]);
    assert!(bomb.len() < 100_000);
    let err = check(&bomb, None, &limits()).unwrap_err();
    assert!(matches!(err, Rejected::ZipBomb(_)), "{err}");
    assert!(matches!(
        extract(bomb, Kind::Epub, &limits()),
        Err(ExtractError::Rejected(Rejected::ZipBomb(_)))
    ));

    // Many entries.
    let names: Vec<String> = (0..300).map(|i| format!("f{i}")).collect();
    let entries: Vec<(&str, &[u8], bool)> = names.iter().map(|n| (n.as_str(), &b""[..], false)).collect();
    let many = zip(&entries);
    let few = Limits {
        max_zip_entries: 200,
        ..limits()
    };
    assert!(matches!(check(&many, None, &few), Err(Rejected::ZipBomb(_))));
    assert!(check(&many, None, &limits()).is_ok());

    // Total unpacked size.
    let tight = Limits {
        max_zip_unpacked: 1000,
        ..limits()
    };
    let ok = zip(&[("a.txt", &[b'a'; 2000], false)]);
    assert!(matches!(check(&ok, None, &tight), Err(Rejected::ZipBomb(_))));
}

#[test]
fn zip_with_unsafe_paths_is_refused() {
    let bytes = zip(&[("../../etc/cron.d/evil", b"x", false)]);
    assert!(matches!(
        check(&bytes, None, &limits()),
        Err(Rejected::UnsafeArchivePath(_))
    ));
}

#[test]
fn damaged_zip_is_refused() {
    let mut bytes = epub();
    bytes.truncate(bytes.len() - 30);
    assert!(matches!(check(&bytes, None, &limits()), Err(Rejected::BadArchive(_))));
    assert!(extract(bytes, Kind::Epub, &limits()).is_err());
}

// ---------------------------------------------------------------------------------------------
// Images, names and sizes

#[test]
fn image_dimension_bomb_is_refused() {
    let head = png_header(100_000, 100_000);
    assert_eq!(
        check(&head, None, &limits()),
        Err(Rejected::ImageTooLarge {
            width: 100_000,
            height: 100_000
        })
    );
    // Within the side limit but too many pixels in total.
    assert!(matches!(
        check(&png_header(20_000, 20_000), None, &limits()),
        Err(Rejected::ImageTooLarge { .. })
    ));
    let ok = check(&png_header(4000, 3000), Some("photo.png"), &limits()).unwrap();
    assert_eq!(ok.kind, Kind::Png);
    assert_eq!(
        (ok.dimensions.unwrap().width, ok.dimensions.unwrap().height),
        (4000, 3000)
    );
    assert_eq!(ok.disposition, Disposition::Inline);
    // A cut header.
    assert_eq!(check(&head[..20], None, &limits()), Err(Rejected::BadImageHeader));
}

#[test]
fn executable_names_are_never_inline() {
    let checked = check(b"just text", Some("readme.txt.exe"), &limits()).unwrap();
    assert!(checked.executable_name);
    assert_eq!(checked.disposition, Disposition::Attachment);
    assert_eq!(checked.mime(), "application/octet-stream");

    let checked = check(b"MZ\x90\0\x03\0\0\0", Some("photo.jpg"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Executable);
    assert_eq!(checked.disposition, Disposition::Attachment);

    let checked = check(b"<html><script>alert(1)</script></html>", Some("page.txt"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Html);
    assert_eq!(checked.disposition, Disposition::Attachment);
}

#[test]
fn size_limits_per_type() {
    let l = Limits {
        max_text_bytes: 10,
        ..limits()
    };
    assert!(matches!(
        check(b"twelve bytes", None, &l),
        Err(Rejected::TooLarge { kind: Kind::Text, .. })
    ));
    // A big video is judged from its head and declared size.
    let head = b"\0\0\0\x18ftypisom\0\0\0\0isomiso2avc1mp41";
    let checked = check_head(head, 3 << 30, Some("talk.mp4"), &limits()).unwrap();
    assert_eq!(checked.mime(), "video/mp4");
    assert!(matches!(
        check_head(head, 20 << 30, None, &limits()),
        Err(Rejected::TooLarge { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// Text formats

#[test]
fn latin1_text_file() {
    let bytes = b"Caf\xe9 cr\xe8me\r\n\r\nNa\xefve \x93quotes\x94 \x80 5.".to_vec();
    let checked = check(&bytes, Some("notes.txt"), &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Text);
    assert_eq!(checked.encoding, Some(TextEncoding::Latin1));
    assert_eq!(checked.disposition, Disposition::Inline);
    let out = extract(bytes, Kind::Text, &limits()).unwrap();
    assert_eq!(out.text, "Café crème\n\nNaïve “quotes” € 5.");
    assert_eq!(out.passages.len(), 1);
    assert_eq!(out.passages[0].heading, None);
}

#[test]
fn html_page() {
    let bytes = b"<!doctype html><title>Wells</title><h1>Digging a well</h1><p>Dig below the water table.</p>\
                  <script>steal()</script><h2>Lining</h2><p>Line it with stones.</p>"
        .to_vec();
    let checked = check(&bytes, None, &limits()).unwrap();
    assert_eq!(checked.kind, Kind::Html);
    let out = extract(bytes, Kind::Html, &limits()).unwrap();
    assert_eq!(out.title.as_deref(), Some("Wells"));
    assert_eq!(
        out.text,
        "# Digging a well\n\nDig below the water table.\n\n## Lining\n\nLine it with stones."
    );
    assert_eq!(out.passages[1].heading.as_deref(), Some("Digging a well > Lining"));
}

#[test]
fn markdown_file() {
    let bytes = b"# Solar\n\nPanels face south.\n\n## Batteries\n\nKeep them dry.\n".to_vec();
    let checked = check(&bytes, Some("solar.md"), &limits()).unwrap();
    assert_eq!(checked.mime(), "text/markdown");
    let out = extract(bytes, Kind::Markdown, &limits()).unwrap();
    assert_eq!(out.passages.len(), 2);
    assert_eq!(out.passages[1].heading.as_deref(), Some("Solar > Batteries"));
}

#[test]
fn images_have_no_text() {
    assert_eq!(
        extract(png_header(1, 1), Kind::Png, &limits()),
        Err(ExtractError::Unsupported(Kind::Png))
    );
}
