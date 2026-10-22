//! Markdown to HTML for guides, made safe: raw HTML is shown as text, and links can't run code.

use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

/// Renders `text`. `file_url` turns a link or image that names one of the guide's own files
/// (`![diagram](diagram.webp)`) into that file's URL.
pub fn render(text: &str, file_url: impl Fn(&str) -> Option<String>) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(text, options).map(|event| match event {
        // Raw HTML in a guide is displayed, never interpreted.
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe_url(dest_url, &file_url),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: safe_url(dest_url, &file_url),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::with_capacity(text.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

/// Keeps web, mail and same-site links; maps the guide's own file names; drops everything else
/// (`javascript:`, `data:`, `file:`, …).
fn safe_url<'a>(url: CowStr<'a>, file_url: &impl Fn(&str) -> Option<String>) -> CowStr<'a> {
    if let Some(mapped) = file_url(&url) {
        return mapped.into();
    }
    let lower = url.trim().to_ascii_lowercase();
    let scheme_free = match lower.find(':') {
        None => true,
        Some(colon) => lower[..colon].contains(['/', '?', '#']),
    };
    let allowed =
        lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || scheme_free;
    if allowed {
        url
    } else {
        "#".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> String {
        render(text, |name| {
            (name == "diagram.webp").then(|| "/file/x/1/diagram.webp".to_string())
        })
    }

    #[test]
    fn renders_basic_markdown() {
        assert!(r("**Boil** water").contains("<strong>Boil</strong>"));
        assert!(r("1. one\n2. two").contains("<ol>"));
        assert!(r("| a | b |\n|---|---|\n| 1 | 2 |").contains("<table>"));
    }

    #[test]
    fn raw_html_is_shown_as_text() {
        let out = r("<script>alert(1)</script>\n\nHello <b onmouseover=x>there</b>");
        assert!(!out.contains("<script"));
        assert!(!out.contains("<b "));
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn dangerous_links_are_dropped() {
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " javascript:x",
            "data:text/html,x",
            "file:///etc/passwd",
            "vbscript:x",
        ] {
            let out = r(&format!("[click]({bad})"));
            assert!(out.contains("href=\"#\""), "{bad}: {out}");
        }
        for good in [
            "https://example.org",
            "http://10.0.0.2/x",
            "mailto:a@b.c",
            "/item/x",
            "#top",
            "other.md",
            "a/b:c",
        ] {
            assert!(
                r(&format!("[x]({good})")).contains(&format!("href=\"{good}\"")),
                "{good}"
            );
        }
    }

    #[test]
    fn own_files_are_linked() {
        assert!(r("![Diagram](diagram.webp)").contains("src=\"/file/x/1/diagram.webp\""));
    }
}
