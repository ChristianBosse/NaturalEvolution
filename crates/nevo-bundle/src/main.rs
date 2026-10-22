//! `nevo-bundle`: moves a library on a USB stick, or dumps it as plain folders.
//!
//! ```text
//! nevo-bundle export-bundle <store> <out.nevo> [--key <file>] [--node <key>]... [--no-blobs]
//!                           [--topic <topic>]... [--max-size <bytes>] [--note <text>]
//! nevo-bundle import-bundle <store> <file.nevo>
//! nevo-bundle export-folders <store> <dir> [--include-private]
//! ```
//!
//! `<store>` is a library folder, such as `nevo-data/library` for the `nevo` program. A bundle is
//! signed with the node key, by default `node.key` next to the library folder (where `nevo`
//! keeps it).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nevo_bundle::{
    export_bundle_file, export_folders, import_bundle_file, BlobFilter, ExportOptions, FolderOptions, Problems,
};
use nevo_format::Keypair;
use nevo_store::Store;

const USAGE: &str = "usage:
  nevo-bundle export-bundle <store> <out.nevo> [--key <file>] [--node <key>]... [--no-blobs]
                            [--topic <topic>]... [--max-size <bytes>] [--note <text>]
  nevo-bundle import-bundle <store> <file.nevo>
  nevo-bundle export-folders <store> <dir> [--include-private]";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        // Finished, but some parts were skipped (listed above).
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("nevo-bundle: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        println!("{USAGE}");
        return Ok(true);
    }
    let [command, store, target, options @ ..] = args.as_slice() else {
        return Err(format!("missing arguments\n{USAGE}"));
    };
    let (store_path, target) = (Path::new(store), Path::new(target));
    match command.as_str() {
        "export-bundle" => export(&open_existing(store_path)?, store_path, target, options),
        "import-bundle" => {
            no_options(options)?;
            // Checked first, so a mistyped bundle path doesn't leave an empty library behind.
            if !target.is_file() {
                return Err(format!("no bundle file at {}", target.display()));
            }
            let store = Store::open(store_path).map_err(|e| format!("library: {e}"))?;
            let r = import_bundle_file(&store, target).map_err(|e| format!("{}: {e}", target.display()))?;
            println!("bundle {} by {}", r.bundle, r.by);
            if let Some(note) = &r.note {
                println!("note:    {note}");
            }
            println!(
                "added:   {} heads, {} log entries, {} objects, {} blobs ({} bytes); {} already here",
                r.heads, r.entries, r.objects, r.blobs, r.blob_bytes, r.already_had
            );
            Ok(print_problems(&r.problems))
        }
        "export-folders" => {
            let mut include_private = false;
            for option in options {
                match option.as_str() {
                    "--include-private" => include_private = true,
                    other => return Err(format!("unknown option: {other}")),
                }
            }
            let store = open_existing(store_path)?;
            let r = export_folders(&store, target, &FolderOptions { include_private }).map_err(|e| e.to_string())?;
            println!(
                "wrote {} items ({} files) to {}; {} node-only or private items left out",
                r.items,
                r.files,
                target.display(),
                r.skipped_private
            );
            Ok(print_problems(&r.problems))
        }
        other => Err(format!("unknown command: {other}\n{USAGE}")),
    }
}

fn export(store: &Store, store_path: &Path, out: &Path, args: &[String]) -> Result<bool, String> {
    let mut options = ExportOptions::default();
    let mut key_file: Option<PathBuf> = None;
    let mut nodes = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--key" => key_file = Some(value()?.into()),
            "--node" => {
                let v = value()?;
                nodes.push(v.parse().map_err(|_| format!("not a public key: {v}"))?);
            }
            "--no-blobs" => options.blobs = BlobFilter::none(),
            "--topic" => {
                let v = value()?;
                options.blobs.topics.push(v.parse().map_err(|e| format!("{v}: {e}"))?);
            }
            "--max-size" => {
                let v = value()?;
                options.blobs.max_size = Some(v.parse().map_err(|_| format!("not a size in bytes: {v}"))?);
            }
            "--note" => options.note = Some(value()?.clone()),
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if !nodes.is_empty() {
        options.nodes = Some(nodes);
    }
    let key_file = key_file.unwrap_or_else(|| {
        let parent = store_path.parent().unwrap_or(Path::new("."));
        parent.join("node.key")
    });
    let key = load_key(&key_file)?;
    let r = export_bundle_file(store, &key, &options, out).map_err(|e| e.to_string())?;
    println!("bundle {} written to {}", r.bundle, out.display());
    println!(
        "inside:  {} logs, {} log entries, {} objects, {} blobs ({} bytes); {} objects not shareable or missing",
        r.logs, r.entries, r.objects, r.blobs, r.blob_bytes, r.skipped_objects
    );
    Ok(print_problems(&r.problems))
}

/// Opens a library that must already exist: exporting from a mistyped path should fail, not
/// create an empty library.
fn open_existing(path: &Path) -> Result<Store, String> {
    if !path.join("objects.sqlite").is_file() {
        return Err(format!("no library at {}", path.display()));
    }
    Store::open(path).map_err(|e| format!("library: {e}"))
}

fn load_key(path: &Path) -> Result<Keypair, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("node key {}: {e} (use --key <file>)", path.display()))?;
    let secret: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{} must hold exactly 32 bytes", path.display()))?;
    Ok(Keypair::from_secret(secret))
}

fn no_options(options: &[String]) -> Result<(), String> {
    match options.first() {
        Some(other) => Err(format!("unknown option: {other}")),
        None => Ok(()),
    }
}

/// Prints problems and returns whether there were none.
fn print_problems(problems: &Problems) -> bool {
    for p in &problems.listed {
        println!("problem: {p}");
    }
    if problems.more > 0 {
        println!("problem: … and {} more", problems.more);
    }
    problems.is_empty()
}
