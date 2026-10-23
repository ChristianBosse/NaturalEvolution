//! Asks a model questions and reports load time, speed and memory (memory on Linux only).
//!
//! ```text
//! cargo run --release -p nevo-llm --features llama --example ask -- <model.gguf> <system.txt> <question.txt>...
//! ```
//!
//! NEVO_THREADS and NEVO_CONTEXT override the defaults; NEVO_SHOW=1 prints the answers.

use std::error::Error;
use std::{env, fs, process};

use nevo_llm::{Cancel, Llm, ModelSettings, Request};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: ask <model.gguf> <system.txt> <question.txt>...");
        process::exit(2);
    }
    let mut settings = ModelSettings::new(&args[0]);
    if let Ok(threads) = env::var("NEVO_THREADS") {
        settings.threads = Some(threads.parse()?);
    }
    if let Ok(context) = env::var("NEVO_CONTEXT") {
        settings.context = context.parse()?;
    }
    let show = env::var_os("NEVO_SHOW").is_some();

    let llm = Llm::load(&settings)?;
    let info = llm.info();
    println!(
        "{}: loaded in {:.2} s, {} threads, context {}, thinking off: {}",
        info.name,
        info.load_time.as_secs_f64(),
        info.threads,
        info.context,
        info.thinking_off
    );
    memory("after load");

    let system = fs::read_to_string(&args[1])?;
    let (mut generated, mut generation_secs) = (0, 0.0);
    for path in &args[2..] {
        let request = Request::new(&system, fs::read_to_string(path)?);
        let answer = llm.chat(&request, &Cancel::new(), |_| true)?;
        let s = answer.stats;
        println!(
            "{path}: prompt {} tokens in {:.2} s ({:.0} tok/s), answer {} tokens ({:.1} tok/s), {:?}",
            s.prompt_tokens,
            s.prompt_time.as_secs_f64(),
            s.prompt_tokens_per_second(),
            s.generated_tokens,
            s.generation_tokens_per_second(),
            s.stop
        );
        if show {
            println!("{}\n", answer.text);
        }
        generated += s.generated_tokens;
        generation_secs += s.generation_time.as_secs_f64();
    }
    println!(
        "average answer speed: {:.1} tok/s",
        f64::from(generated) / generation_secs
    );
    memory("end");
    Ok(())
}

/// Peak and current memory of this process, and the peak of its cgroup (the container), which also counts the
/// model file's pages in the file cache.
fn memory(label: &str) {
    let mut parts = Vec::new();
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            let mut words = line.split_whitespace();
            if let (Some(key @ ("VmHWM:" | "VmRSS:" | "RssAnon:" | "RssFile:")), Some(kb)) =
                (words.next(), words.next())
            {
                let mb = kb.parse::<u64>().unwrap_or(0) / 1024;
                parts.push(format!("{} {mb} MB", key.trim_end_matches(':')));
            }
        }
    }
    if let Ok(peak) = fs::read_to_string("/sys/fs/cgroup/memory.peak") {
        let mb = peak.trim().parse::<u64>().unwrap_or(0) / (1024 * 1024);
        parts.push(format!("cgroup peak {mb} MB"));
    }
    println!("memory {label}: {}", parts.join(", "));
}
