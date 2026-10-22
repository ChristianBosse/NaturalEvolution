//! HTML pages. Plain server-rendered HTML with a little CSS and no JavaScript, so they work on
//! old phones and slow links. Maud escapes every value it inserts.

use maud::{html, Markup, PreEscaped, DOCTYPE};
use nevo_format::{ContentFile, Id, PublicKey, Revision, Sealed, Topic, Visibility, STARTING_TOPICS};

const CSS: &str = r#"
:root { color-scheme: light dark; --fg: #1d1d1b; --bg: #fbfaf7; --muted: #6b6a66; --line: #dddad2;
        --accent: #2f6f4e; --warn-bg: #fff4d6; --warn-fg: #6b4e00; }
@media (prefers-color-scheme: dark) {
  :root { --fg: #ecebe6; --bg: #171716; --muted: #a3a29c; --line: #34332f; --accent: #7cc49b;
          --warn-bg: #3a3017; --warn-fg: #f3d98b; }
}
* { box-sizing: border-box; }
body { margin: 0; font: 17px/1.55 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
       color: var(--fg); background: var(--bg); }
header { border-bottom: 1px solid var(--line); padding: .6rem 1rem; display: flex; gap: 1rem;
         align-items: baseline; flex-wrap: wrap; }
header .brand { font-weight: 700; font-size: 1.15rem; color: var(--fg); text-decoration: none; }
header nav a { margin-right: .9rem; }
main { max-width: 46rem; margin: 0 auto; padding: 1rem; }
a { color: var(--accent); }
h1 { font-size: 1.6rem; line-height: 1.25; margin: .4rem 0 .6rem; }
h2 { font-size: 1.2rem; margin-top: 2rem; }
.muted, .meta { color: var(--muted); font-size: .92rem; }
.meta { display: flex; flex-wrap: wrap; gap: .1rem .9rem; }
.meta span { white-space: nowrap; }
ul.items { list-style: none; padding: 0; }
ul.items li { padding: .55rem 0; border-bottom: 1px solid var(--line); }
ul.items a { font-weight: 600; text-decoration: none; }
.tag { display: inline-block; border: 1px solid var(--line); border-radius: 1rem; padding: 0 .55rem;
       margin: 0 .3rem .3rem 0; font-size: .85rem; text-decoration: none; }
.notice { background: var(--warn-bg); color: var(--warn-fg); padding: .6rem .8rem; border-radius: .4rem; }
.guide { border-top: 1px solid var(--line); margin-top: 1rem; padding-top: .5rem; }
.guide img, video, img.file { max-width: 100%; height: auto; }
table { border-collapse: collapse; } td, th { border: 1px solid var(--line); padding: .3rem .5rem; }
form label { display: block; margin-top: 1rem; font-weight: 600; }
form .hint { font-weight: 400; color: var(--muted); font-size: .9rem; }
input[type=text], input[type=search], select, textarea { width: 100%; font: inherit; padding: .5rem;
       border: 1px solid var(--line); border-radius: .3rem; background: var(--bg); color: var(--fg); }
textarea { min-height: 14rem; }
button { font: inherit; margin-top: 1.2rem; padding: .6rem 1.2rem; border: 0; border-radius: .3rem;
         background: var(--accent); color: var(--bg); font-weight: 600; }
.error { color: #b3261e; font-weight: 600; }
footer { max-width: 46rem; margin: 2rem auto; padding: 0 1rem; color: var(--muted); font-size: .85rem; }
"#;

pub(crate) fn layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · NEVO" }
                style { (PreEscaped(CSS)) }
            }
            body {
                header {
                    a.brand href="/" { "NEVO" }
                    nav {
                        a href="/" { "Library" }
                        a href="/topics" { "Topics" }
                        a href="/add" { "Add a guide" }
                    }
                }
                main { (body) }
                footer { "NEVO works without the internet. Everything here is stored on this device." }
            }
        }
    }
}

/// Short form of a key for display: the first characters are enough to tell people apart.
pub fn short_key(key: &PublicKey) -> String {
    key.to_string().chars().skip(1).take(8).collect()
}

/// `YYYY-MM-DD` (UTC) from Unix milliseconds, without a date library.
pub fn date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A list of items: (item ID, its current version).
fn item_list(items: &[(Id, Sealed<Revision>)]) -> Markup {
    html! {
        ul.items {
            @for (item, rev) in items {
                li {
                    a href={ "/item/" (item) } { (rev.object.title) }
                    @if let Some(summary) = &rev.object.summary { div.muted { (summary) } }
                    div.meta {
                        @for t in &rev.object.topics { span { (t) } }
                        span { (rev.object.lang) }
                    }
                }
            }
        }
    }
}

pub fn home(recent: &[(Id, Sealed<Revision>)]) -> Markup {
    layout(
        "Library",
        html! {
            h1 { "Library" }
            p.muted { "Guides and documents on this NEVO. Search is coming next." }
            h2 { "Topics" }
            p { @for (top, _) in STARTING_TOPICS { a.tag href={ "/topic/" (top) } { (top) } } }
            h2 { "Recently added" }
            @if recent.is_empty() {
                p { "Nothing here yet. " a href="/add" { "Add the first guide." } }
            } @else {
                (item_list(recent))
            }
        },
    )
}

pub fn topics() -> Markup {
    layout(
        "Topics",
        html! {
            h1 { "Topics" }
            @for (top, subs) in STARTING_TOPICS {
                h2 { a href={ "/topic/" (top) } { (top) } }
                p { @for sub in subs.iter() { a.tag href={ "/topic/" (top) "/" (sub) } { (sub) } } }
            }
        },
    )
}

pub fn topic(topic: &Topic, items: &[(Id, Sealed<Revision>)]) -> Markup {
    layout(
        topic.as_str(),
        html! {
            p.muted { a href="/topics" { "Topics" } " / " (topic) }
            h1 { (topic) }
            @if items.is_empty() {
                p { "Nothing in this topic yet." }
            } @else {
                (item_list(items))
            }
        },
    )
}

/// What the item page shows.
pub struct ItemView<'a> {
    pub item: Id,
    pub tip: &'a Sealed<Revision>,
    pub tips: usize,
    pub history: &'a [Sealed<Revision>],
    pub local_author: bool,
    /// Rendered Markdown of the guide text, if the revision has one.
    pub guide_html: Option<String>,
}

pub fn file_url(rev: Id, index: usize, file: &ContentFile) -> String {
    format!("/file/{rev}/{index}/{}", url_segment(&file.name))
}

/// Percent-encodes a file name for use as one URL path segment.
fn url_segment(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn item(view: &ItemView) -> Markup {
    let r = &view.tip.object;
    let rev_id = view.tip.id();
    html! {
        (layout(&r.title, html! {
            p.muted {
                @for t in &r.topics { a.tag href={ "/topic/" (t) } { (t) } }
            }
            h1 { (r.title) }
            @if let Some(summary) = &r.summary { p { (summary) } }
            p.meta {
                span { (if view.local_author { "Added on this NEVO" } else { "From another NEVO" }) }
                span { "Language: " (r.lang) }
                span { "License: " (r.license) }
                @if r.vis == Visibility::Node { span { "Only on this NEVO" } }
            }
            p { a href={ "/item/" (view.item) "/edit" } { "Edit" } " · " a href={ "/item/" (view.item) "/history" } { "History" } }
            @if view.tips > 1 {
                p.notice {
                    "This item has " (view.tips) " versions that were edited separately and haven't been merged. You are seeing the newest. "
                    a href={ "/item/" (view.item) "/merge" } { "Compare and merge" }
                }
            }
            @if let Some(guide) = &view.guide_html {
                div.guide { (PreEscaped(guide)) }
            }
            h2 { "Files" }
            ul.items {
                @for (i, f) in r.content.iter().enumerate() {
                    @let url = file_url(rev_id, i, f);
                    li {
                        @if f.mime.starts_with("image/") && f.mime != "image/svg+xml" {
                            img.file src=(url) alt=(f.name);
                        } @else if f.mime.starts_with("video/") {
                            video controls preload="metadata" src=(url) {}
                        } @else if f.mime.starts_with("audio/") {
                            audio controls preload="none" src=(url) {}
                        }
                        a href=(url) { (f.name) }
                        span.muted { " · " (f.mime) " · " (size(f.size)) }
                    }
                }
            }
            h2 { "History" }
            ul.items {
                @for h in view.history.iter().rev() {
                    li {
                        (h.object.title)
                        div.meta {
                            span { "Version " (h.object.clock) }
                            span { (date(h.object.time.ms)) }
                            span { "by " (short_key(&h.object.author)) }
                        }
                    }
                }
            }
            p.muted { "Item " (view.item) }
        }))
    }
}

pub fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.0} KB", bytes as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.2} GB", bytes as f64 / 1e9),
    }
}

/// What the add form shows again after a mistake.
#[derive(Default)]
pub struct AddForm {
    pub title: String,
    pub summary: String,
    pub text: String,
    pub topics: Vec<String>,
    pub lang: String,
    pub license: String,
    pub node_only: bool,
    pub error: Option<String>,
}

pub fn add(form: &AddForm) -> Markup {
    layout(
        "Add a guide",
        html! {
            h1 { "Add a guide" }
            p.muted { "Write steps in the text box, attach files (PDF, photos, video), or both." }
            @if let Some(error) = &form.error { p.error role="alert" { (error) } }
            form method="post" action="/add" enctype="multipart/form-data" {
                (guide_fields(form))
                label { input type="checkbox" name="node_only" value="1" checked[form.node_only]; " Keep it on this NEVO only (don't share with other nodes)" }
                // Files come last: browsers send fields in page order, so the server can check the
                // typed fields before it stores any upload.
                label for="files" { "Files " span.hint { "(optional: PDF, photos, video)" } }
                input type="file" id="files" name="files" multiple;
                button type="submit" { "Add" }
            }
        },
    )
}

/// The typed fields the add and edit forms share, from the title to the license.
pub(crate) fn guide_fields(form: &AddForm) -> Markup {
    let lang = if form.lang.is_empty() { "en" } else { &form.lang };
    let license = if form.license.is_empty() {
        "CC-BY-SA-4.0"
    } else {
        &form.license
    };
    html! {
        label for="title" { "Title" }
        input type="text" id="title" name="title" required maxlength="500" value=(form.title);
        label for="summary" { "Summary " span.hint { "(optional, one or two sentences)" } }
        input type="text" id="summary" name="summary" maxlength="1000" value=(form.summary);
        label for="topics" { "Topics " span.hint { "(pick one or more)" } }
        select id="topics" name="topics" multiple size="8" required {
            @for (top, subs) in STARTING_TOPICS {
                optgroup label=(top) {
                    option value=(top) selected[form.topics.iter().any(|t| t == top)] { (top) }
                    @for sub in subs.iter() {
                        @let path = format!("{top}/{sub}");
                        option value=(path) selected[form.topics.contains(&path)] { (top) " / " (sub) }
                    }
                }
            }
        }
        label for="text" { "Text " span.hint { "(Markdown: **bold**, 1. numbered steps, ![photo](photo.jpg) for an attached photo)" } }
        textarea id="text" name="text" { (form.text) }
        label for="lang" { "Language " span.hint { "(code such as en, fr, es)" } }
        input type="text" id="lang" name="lang" maxlength="35" value=(lang);
        label for="license" { "License " span.hint { "(CC-BY-SA-4.0 lets others share and improve it)" } }
        input type="text" id="license" name="license" maxlength="200" value=(license);
    }
}

pub fn error(status: u16, message: &str) -> Markup {
    layout(
        "Error",
        html! {
            h1 { (status) }
            p { (message) }
            p { a href="/" { "Back to the library" } }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_792_000_000_000), "2026-10-14");
        assert_eq!(date(951_782_400_000), "2000-02-29");
        assert_eq!(date(-86_400_000), "1969-12-31");
    }

    #[test]
    fn url_segments_are_encoded() {
        assert_eq!(url_segment("my guide/../x?.pdf"), "my%20guide%2F..%2Fx%3F.pdf");
        assert_eq!(url_segment("café.jpg"), "caf%C3%A9.jpg");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(48_200), "48 KB");
        assert_eq!(size(2_500_000), "2.5 MB");
    }
}
