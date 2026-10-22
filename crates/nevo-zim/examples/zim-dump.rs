//! Prints a pack's metadata, or one entry.
//!
//!   cargo run -p nevo-zim --example zim-dump -- <file.zim>                 metadata
//!   cargo run -p nevo-zim --example zim-dump -- <file.zim> <path>          clean text and passages
//!   cargo run -p nevo-zim --example zim-dump -- <file.zim> <path> --html   raw bytes

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(file) = args.first() else {
        eprintln!("usage: zim-dump <file.zim> [path [--html]]");
        std::process::exit(2);
    };
    let pack = nevo_zim::Pack::open(file)?;
    let Some(path) = args.get(1) else {
        println!("{:#?}", pack.metadata());
        println!("main page: {:?}", pack.main_page()?.map(|e| e.path));
        return Ok(());
    };
    let entry = pack.find(path)?.ok_or("not found")?;
    let bytes = pack.read(&entry)?;
    if args.get(2).map(String::as_str) == Some("--html") {
        std::io::stdout().write_all(&bytes)?;
        return Ok(());
    }
    eprintln!("{entry:?}");
    let doc = nevo_zim::extract(&String::from_utf8_lossy(&bytes));
    println!("# {}\n", doc.title.as_deref().unwrap_or(""));
    for s in &doc.sections {
        println!("{} {}", "#".repeat(s.level as usize), s.heading);
        for p in &s.paragraphs {
            println!("{p}\n");
        }
    }
    let passages = doc.passages();
    println!("--- {} passages", passages.len());
    for p in &passages {
        println!(
            "[{}] {} chars: {}…",
            p.heading,
            p.text.chars().count(),
            p.text.chars().take(80).collect::<String>()
        );
    }
    Ok(())
}
