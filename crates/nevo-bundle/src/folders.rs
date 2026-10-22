//! Plain-folder export (spec section 5, "Export"): one folder per item holding its current files
//! and a `meta.json`, so a library stays usable with nothing but a file browser.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use nevo_format::cbor::Value;
use nevo_format::{Id, Revision, Sealed, Visibility};
use nevo_store::Store;

use crate::error::{BundleError, Result};
use crate::Problems;

/// Longest folder or file name written, in characters. Leaves room under Windows' 260-character
/// path limit for the export folder itself.
const MAX_NAME: usize = 100;

const META: &str = "meta.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderOptions {
    /// Also export node-only and private items. Off by default: a folder export usually leaves
    /// the node on a USB stick.
    pub include_private: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderReport {
    pub items: usize,
    pub files: usize,
    /// Node-only and private items left out.
    pub skipped_private: usize,
    /// Files whose blob this library doesn't have. They are listed in `meta.json` anyway.
    pub missing_blobs: usize,
    pub problems: Problems,
}

/// Writes every item's current version under `dir`, which must be empty or not exist yet.
///
/// The current version is the branch tip with the highest clock. Trust isn't applied here: the
/// export shows what the library holds, and `meta.json` notes when an item has unmerged edits.
pub fn export_folders(store: &Store, dir: &Path, options: &FolderOptions) -> Result<FolderReport> {
    if dir.exists() && fs::read_dir(dir)?.next().is_some() {
        return Err(BundleError::Target(format!("{} is not empty", dir.display())));
    }
    fs::create_dir_all(dir)?;

    // One current version per item. Tips come newest clock first within an item, ties by ID.
    let mut current: BTreeMap<Id, (Sealed<Revision>, usize)> = BTreeMap::new();
    for tip in store.recent_tips(usize::MAX)? {
        let item = tip.object.item_id(tip.id());
        match current.get_mut(&item) {
            Some((best, branches)) => {
                *branches += 1;
                if (tip.object.clock, std::cmp::Reverse(tip.id())) > (best.object.clock, std::cmp::Reverse(best.id())) {
                    *best = tip;
                }
            }
            None => {
                current.insert(item, (tip, 1));
            }
        }
    }

    let mut report = FolderReport::default();
    let mut items: Vec<(Id, Sealed<Revision>, usize)> = Vec::new();
    for (item, (rev, branches)) in current {
        if rev.object.vis != Visibility::Public && !options.include_private {
            report.skipped_private += 1;
        } else {
            items.push((item, rev, branches));
        }
    }
    // Sorted by title so duplicate titles are numbered the same way on every run.
    items.sort_by_cached_key(|(item, rev, _)| (rev.object.title.to_lowercase(), rev.object.title.clone(), *item));

    let mut folders = HashSet::new();
    for (item, rev, branches) in items {
        let folder = unique(&safe_name(&rev.object.title), &mut folders, false);
        let path = dir.join(&folder);
        fs::create_dir(&path)?;
        let mut names = HashSet::from([META.to_string()]);
        let mut files = Vec::new();
        for f in &rev.object.content {
            let name = unique(&safe_name(&f.name), &mut names, true);
            let missing = match store.open_blob(f.blob)? {
                Some(mut blob) => {
                    let mut out = File::create_new(path.join(&name))?;
                    io::copy(&mut blob, &mut out)?;
                    out.sync_all()?;
                    report.files += 1;
                    false
                }
                None => {
                    report.missing_blobs += 1;
                    report
                        .problems
                        .add(format!("{folder}/{name}: blob {} is not in this library", f.blob));
                    true
                }
            };
            files.push(Json::obj([
                ("name", name.into()),
                ("original_name", f.name.as_str().into()),
                ("blob", f.blob.to_string().into()),
                ("size", Json::Int(f.size.into())),
                ("mime", f.mime.as_str().into()),
                ("missing", Json::Bool(missing)),
            ]));
        }
        let versions = store
            .item_revisions(item)?
            .into_iter()
            .filter(|r| r.object.vis == Visibility::Public || options.include_private)
            .map(|r| {
                Json::obj([
                    ("revision", r.id().to_string().into()),
                    (
                        "parents",
                        Json::Array(r.object.parents.iter().map(|p| p.to_string().into()).collect()),
                    ),
                    ("author", r.object.author.to_string().into()),
                    ("clock", Json::Int(r.object.clock.into())),
                    ("time_ms", Json::Int(r.object.time.ms.into())),
                    ("title", r.object.title.as_str().into()),
                    ("current", Json::Bool(r.id() == rev.id())),
                ])
            })
            .collect();
        let meta = meta(item, &rev, branches, files, versions);
        let mut out = BufWriter::new(File::create_new(path.join(META))?);
        meta.write(&mut out, 0)?;
        out.write_all(b"\n")?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        report.items += 1;
    }
    Ok(report)
}

fn meta(item: Id, rev: &Sealed<Revision>, branches: usize, files: Vec<Json>, versions: Vec<Json>) -> Json {
    let r = &rev.object;
    let mut fields = vec![
        ("title", r.title.as_str().into()),
        ("item", item.to_string().into()),
        ("revision", rev.id().to_string().into()),
        ("kind", r.kind.as_str().into()),
        ("lang", r.lang.as_str().into()),
        (
            "topics",
            Json::Array(r.topics.iter().map(|t| t.as_str().into()).collect()),
        ),
        ("license", r.license.as_str().into()),
        ("visibility", r.vis.as_str().into()),
        ("author", r.author.to_string().into()),
        ("time_ms", Json::Int(r.time.ms.into())),
    ];
    if let Some(summary) = &r.summary {
        fields.push(("summary", summary.as_str().into()));
    }
    if let Some(source) = &r.source {
        fields.push(("source", Json::from_cbor(&Value::Map(source.clone()))));
    }
    // More than one branch means offline edits nobody has merged yet; the others are in versions.
    fields.push(("branches", Json::Int(branches as i128)));
    fields.push(("files", Json::Array(files)));
    fields.push(("versions", Json::Array(versions)));
    Json::obj(fields)
}

/// Turns a title or file name into a name that is safe on Windows, macOS and Linux: no
/// separators or reserved characters, no reserved device names (`CON`, `LPT1`, …), no leading
/// dot (hidden on Linux), no trailing dot or space (dropped by Windows), at most 100 characters.
pub fn safe_name(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name: String = replaced.trim().chars().take(MAX_NAME).collect();
    while name.ends_with(['.', ' ']) {
        name.pop();
    }
    if name.starts_with('.') {
        name.replace_range(..1, "_");
    }
    if name.is_empty() {
        return "Untitled".into();
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(0, '_');
    }
    name
}

/// Makes `name` unique among `used`, ignoring case (Windows and macOS file names ignore it):
/// `Guide`, `Guide (2)`, …; for files the number goes before the extension.
fn unique(name: &str, used: &mut HashSet<String>, is_file: bool) -> String {
    if used.insert(name.to_lowercase()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if is_file && dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| used.insert(candidate.to_lowercase()))
        .expect("some number is free")
}

/// Just enough JSON for `meta.json`, so the export needs no extra dependency.
enum Json {
    Null,
    Bool(bool),
    Int(i128),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl Json {
    fn obj<'k>(fields: impl IntoIterator<Item = (&'k str, Json)>) -> Json {
        Json::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Source maps are free-form CBOR. Byte strings, which JSON lacks, become hex text.
    fn from_cbor(value: &Value) -> Json {
        match value {
            Value::Int(n) => Json::Int(*n),
            Value::Bytes(b) => Json::Str(b.iter().map(|x| format!("{x:02x}")).collect()),
            Value::Text(s) => Json::Str(s.clone()),
            Value::Array(items) => Json::Array(items.iter().map(Json::from_cbor).collect()),
            Value::Map(m) => Json::Object(m.iter().map(|(k, v)| (k.to_string(), Json::from_cbor(v))).collect()),
            Value::Bool(b) => Json::Bool(*b),
            Value::Null => Json::Null,
        }
    }

    fn write(&self, out: &mut impl Write, indent: usize) -> io::Result<()> {
        let pad = |n: usize| "  ".repeat(n);
        match self {
            Json::Null => write!(out, "null"),
            Json::Bool(b) => write!(out, "{b}"),
            Json::Int(n) => write!(out, "{n}"),
            Json::Str(s) => write_str(out, s),
            Json::Array(items) if items.is_empty() => write!(out, "[]"),
            Json::Array(items) => {
                writeln!(out, "[")?;
                for (i, item) in items.iter().enumerate() {
                    write!(out, "{}", pad(indent + 1))?;
                    item.write(out, indent + 1)?;
                    writeln!(out, "{}", if i + 1 < items.len() { "," } else { "" })?;
                }
                write!(out, "{}]", pad(indent))
            }
            Json::Object(fields) if fields.is_empty() => write!(out, "{{}}"),
            Json::Object(fields) => {
                writeln!(out, "{{")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    write!(out, "{}", pad(indent + 1))?;
                    write_str(out, k)?;
                    write!(out, ": ")?;
                    v.write(out, indent + 1)?;
                    writeln!(out, "{}", if i + 1 < fields.len() { "," } else { "" })?;
                }
                write!(out, "{}}}", pad(indent))
            }
        }
    }
}

fn write_str(out: &mut impl Write, s: &str) -> io::Result<()> {
    write!(out, "\"")?;
    for c in s.chars() {
        match c {
            '"' => write!(out, "\\\"")?,
            '\\' => write!(out, "\\\\")?,
            '\n' => write!(out, "\\n")?,
            '\r' => write!(out, "\\r")?,
            '\t' => write!(out, "\\t")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => write!(out, "{c}")?,
        }
    }
    write!(out, "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_everywhere() {
        assert_eq!(safe_name("Boiling water"), "Boiling water");
        assert_eq!(safe_name("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(safe_name("  Notes...  "), "Notes");
        assert_eq!(safe_name(".hidden"), "_hidden");
        assert_eq!(safe_name(".."), "Untitled");
        assert_eq!(safe_name(""), "Untitled");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("LPT1"), "_LPT1");
        assert_eq!(safe_name("Console"), "Console");
        assert_eq!(safe_name("tab\there"), "tab_here");
        assert_eq!(safe_name(&"x".repeat(300)).chars().count(), MAX_NAME);
        assert_eq!(safe_name("Réchaud à bois"), "Réchaud à bois");
    }

    #[test]
    fn duplicates_get_numbers() {
        let mut used = HashSet::new();
        assert_eq!(unique("Guide", &mut used, false), "Guide");
        assert_eq!(unique("guide", &mut used, false), "guide (2)");
        assert_eq!(unique("Guide", &mut used, false), "Guide (3)");
        assert_eq!(unique("Dr. Smith", &mut used, false), "Dr. Smith");
        assert_eq!(unique("Dr. Smith", &mut used, false), "Dr. Smith (2)");
        let mut files = HashSet::from([META.to_string()]);
        assert_eq!(unique("meta.json", &mut files, true), "meta (2).json");
        assert_eq!(unique("a.md", &mut files, true), "a.md");
        assert_eq!(unique("A.md", &mut files, true), "A (2).md");
    }

    #[test]
    fn json_escapes() {
        let mut out = Vec::new();
        Json::obj([("t", "a\"b\\c\nd\u{1}".into())]).write(&mut out, 0).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"t\": \"a\\\"b\\\\c\\nd\\u0001\"\n}"
        );
    }
}
