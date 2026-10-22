//! Differences between versions of an item: a line diff of the guide text, and the other
//! fields that changed. Maud escapes every value, so content can't inject HTML here.

use std::time::Duration;

use maud::{html, Markup};
use nevo_format::{ContentFile, Revision, Visibility};
use similar::{ChangeTag, DiffTag, TextDiff};

/// After this long the diff stops searching for the smallest result and settles for a larger
/// one, so a huge guide can't hold a Pi's CPU. The result is still correct, only less tidy.
const TIMEOUT: Duration = Duration::from_millis(300);
/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;

/// One side of a comparison: a revision and its guide text.
pub(crate) struct Side<'a> {
    pub rev: &'a Revision,
    /// Index in `content` of the file read as the guide text. It is compared as text, so it is
    /// left out of the file lists.
    pub guide: Option<usize>,
    pub text: &'a str,
}

impl Side<'_> {
    /// The files other than the guide text.
    fn files(&self) -> impl Iterator<Item = &ContentFile> {
        let guide = self.guide;
        self.rev
            .content
            .iter()
            .enumerate()
            .filter(move |(i, _)| Some(*i) != guide)
            .map(|(_, f)| f)
    }
}

/// The changed lines of `new` compared with `old`, with a few unchanged lines around each.
pub(crate) fn text(old: &str, new: &str) -> Markup {
    let diff = TextDiff::configure().timeout(TIMEOUT).diff_lines(old, new);
    if diff.ops().iter().all(|op| op.tag() == DiffTag::Equal) {
        return html! { p.muted { "The text is the same." } };
    }
    html! {
        div.diff {
            @for (i, group) in diff.grouped_ops(CONTEXT).iter().enumerate() {
                @if i > 0 { div.gap { "…" } }
                @for op in group {
                    @for change in diff.iter_changes(op) {
                        @let (class, sign) = match change.tag() {
                            ChangeTag::Delete => ("del", "−"),
                            ChangeTag::Insert => ("add", "+"),
                            ChangeTag::Equal => ("same", " "),
                        };
                        div class=(class) {
                            span.sign { (sign) " " }
                            (change.value().trim_end_matches(['\n', '\r']))
                        }
                    }
                }
            }
        }
    }
}

fn visibility(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Public => "Shared with other nodes",
        Visibility::Node => "Only on this NEVO",
        Visibility::Private => "Private",
    }
}

/// The fields compared between versions, in display order.
fn fields(side: &Side) -> [(&'static str, String); 6] {
    let r = side.rev;
    [
        ("Title", r.title.clone()),
        ("Summary", r.summary.clone().unwrap_or_default()),
        (
            "Topics",
            r.topics.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", "),
        ),
        ("Language", r.lang.clone()),
        ("License", r.license.clone()),
        ("Visibility", visibility(r.vis).into()),
    ]
}

/// What changed from `old` to `new`: fields, files, then the text.
pub(crate) fn changes(old: &Side, new: &Side) -> Markup {
    let rows: Vec<_> = fields(old)
        .into_iter()
        .zip(fields(new))
        .filter(|((_, a), (_, b))| a != b)
        .map(|((label, a), (_, b))| (label, a, b))
        .collect();
    let same_file = |a: &ContentFile, b: &ContentFile| a.name == b.name && a.blob == b.blob;
    let added: Vec<_> = new.files().filter(|f| !old.files().any(|o| same_file(o, f))).collect();
    let removed: Vec<_> = old.files().filter(|f| !new.files().any(|n| same_file(n, f))).collect();
    html! {
        @if !rows.is_empty() {
            div.scroll {
                table.compare {
                    tr { th {} th { "Before" } th { "After" } }
                    @for (label, before, after) in &rows {
                        tr { th { (label) } td.del { (before) } td.add { (after) } }
                    }
                }
            }
        }
        @if !added.is_empty() {
            p { "Files added: " @for (i, f) in added.iter().enumerate() { @if i > 0 { ", " } (f.name) } }
        }
        @if !removed.is_empty() {
            p { "Files removed: " @for (i, f) in removed.iter().enumerate() { @if i > 0 { ", " } (f.name) } }
        }
        (text(old.text, new.text))
    }
}

/// Table rows comparing several versions side by side, one column each. Rows whose values
/// differ are marked.
pub(crate) fn side_by_side(sides: &[Side]) -> Markup {
    let columns: Vec<_> = sides.iter().map(fields).collect();
    let file_lists: Vec<Vec<&ContentFile>> = sides.iter().map(|s| s.files().collect()).collect();
    let files_differ = file_lists.windows(2).any(|w| {
        w[0].len() != w[1].len()
            || w[0]
                .iter()
                .zip(&w[1])
                .any(|(a, b)| a.name != b.name || a.blob != b.blob)
    });
    html! {
        @for row in 0..6 {
            @let differs = columns.windows(2).any(|w| w[0][row].1 != w[1][row].1);
            tr.differs[differs] {
                th { (columns[0][row].0) }
                @for column in &columns { td { (column[row].1) } }
            }
        }
        tr.differs[files_differ] {
            th { "Files" }
            @for files in &file_lists {
                td { @for (i, f) in files.iter().enumerate() { @if i > 0 { br; } (f.name) } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_lines_are_marked() {
        let out = text("a\nb\nc\n", "a\nB\nc\nd\n").into_string();
        assert!(
            out.contains(r#"<div class="del"><span class="sign">− </span>b</div>"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<div class="add"><span class="sign">+ </span>B</div>"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<div class="add"><span class="sign">+ </span>d</div>"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<div class="same"><span class="sign">  </span>a</div>"#),
            "{out}"
        );
    }

    #[test]
    fn same_text_says_so() {
        assert!(text("x\n", "x\n").into_string().contains("The text is the same."));
        assert!(text("", "").into_string().contains("The text is the same."));
    }

    #[test]
    fn far_apart_changes_are_grouped() {
        let old: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 2\n", "line two\n").replace("line 35\n", "line 35!\n");
        let out = text(&old, &new).into_string();
        assert!(out.contains("…"));
        assert!(
            !out.contains("line 20<"),
            "unchanged lines far from a change are left out"
        );
    }

    #[test]
    fn content_is_escaped() {
        let out = text("", "<script>alert(1)</script>\n").into_string();
        assert!(!out.contains("<script>"));
        assert!(out.contains("&lt;script&gt;"));
    }
}
