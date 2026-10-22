//! Runs the answer checks over the 140 real answers of the Phase 0 AI benchmark and compares the
//! verdicts with the grades. Run with `--nocapture` to see the table.

use std::fs;
use std::path::{Path, PathBuf};

use nevo_answer::{Prompt, Source, Trust, Verdict, SYSTEM_PROMPT};
use serde_json::Value;

fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/ai-benchmark")
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("missing string field {key}"))
}

/// The prompt exactly as the benchmark sent it. The benchmark also gave the model Unknown sources
/// on safety-critical topics, so the answers are checked against all of them, not against what
/// `Prompt::new` would keep.
fn benchmark_prompt(case: &Value) -> Prompt {
    let sources = case["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .map(|s| {
            let trust = Trust::from_label(str_field(s, "trust")).expect("trust label");
            Source::new(str_field(s, "id"), trust, str_field(s, "title"), str_field(s, "text"))
        })
        .collect();
    Prompt {
        question: str_field(case, "question").to_string(),
        topic: str_field(case, "topic").to_string(),
        safety_critical: case["safety_critical"].as_bool().expect("safety_critical"),
        sources,
    }
}

struct Graded {
    model: String,
    case: String,
    critical: bool,
    /// Correct and grounded with full marks, not just "not dangerous".
    good: bool,
    verdict: Verdict,
}

fn run_all() -> Vec<Graded> {
    let dir = bench_dir();
    let cases = read_json(&dir.join("cases.json"));
    let cases = cases["cases"].as_array().expect("cases");
    let mut models: Vec<String> = fs::read_dir(dir.join("results/gpu"))
        .expect("results/gpu")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".json").map(str::to_string))
        .collect();
    models.sort();

    let mut out = Vec::new();
    for model in models {
        let results = read_json(&dir.join(format!("results/gpu/{model}.json")));
        let grades = read_json(&dir.join(format!("results/grades/{model}.json")));
        for row in results["rows"].as_array().expect("rows") {
            let id = str_field(row, "id");
            let case = cases.iter().find(|c| c["id"] == id).expect("case for row");
            let grade = grades["cases"]
                .as_array()
                .expect("grades")
                .iter()
                .find(|g| g["id"] == id)
                .expect("grade for row");
            let checked = benchmark_prompt(case).check(str_field(row, "answer"));
            out.push(Graded {
                model: model.clone(),
                case: id.to_string(),
                critical: grade["critical_failure"].as_bool().expect("critical_failure"),
                good: grade["correct"] == 2 && grade["grounded"] == 2,
                verdict: checked.verdict,
            });
        }
    }
    out
}

#[test]
fn system_prompt_is_the_benchmark_prompt() {
    let spike = fs::read_to_string(bench_dir().join("prompt.md")).expect("prompt.md");
    assert_eq!(SYSTEM_PROMPT, spike);
}

fn count(rows: &[&Graded], keep: impl Fn(&Graded) -> bool) -> usize {
    rows.iter().filter(|g| keep(g)).count()
}

fn is_blocked(g: &Graded) -> bool {
    matches!(g.verdict, Verdict::Blocked { .. })
}

fn is_refusal(g: &Graded) -> bool {
    g.verdict == Verdict::Refusal
}

#[test]
fn checks_on_the_benchmark_answers() {
    let all = run_all();
    assert_eq!(all.len(), 140);

    println!(
        "
Answers not shown as written:"
    );
    for g in all.iter().filter(|g| !g.verdict.is_pass()) {
        let grade = if g.critical {
            "CRITICAL"
        } else if g.good {
            "good"
        } else {
            "weak"
        };
        let outcome = if is_refusal(g) { "cut to refusal" } else { "blocked" };
        println!("  {} / {} [{grade}] {outcome}", g.model, g.case);
        for p in g.verdict.problems() {
            println!("      - {p}");
        }
    }

    println!(
        "
Critical failures (from the grades):"
    );
    let mut missed = Vec::new();
    for g in all.iter().filter(|g| g.critical) {
        let outcome = if g.verdict.is_pass() {
            "MISSED"
        } else if is_refusal(g) {
            "cut to refusal"
        } else {
            "blocked"
        };
        println!("  {:<14} {:<13} {outcome}", g.model, g.case);
        if g.verdict.is_pass() {
            missed.push(format!("{}/{}", g.model, g.case));
        }
    }

    println!(
        "
| Model | Critical caught | Good answers blocked | Good cut to refusal | Weak blocked or cut |"
    );
    println!("|---|---|---|---|---|");
    let mut models: Vec<&str> = all.iter().map(|g| g.model.as_str()).collect();
    models.push("all");
    models.dedup();
    let mut good_blocked_total = 0;
    for model in models {
        let rows: Vec<&Graded> = all.iter().filter(|g| model == "all" || g.model == model).collect();
        let critical = count(&rows, |g| g.critical);
        let caught = count(&rows, |g| g.critical && !g.verdict.is_pass());
        let good = count(&rows, |g| !g.critical && g.good);
        let good_blocked = count(&rows, |g| !g.critical && g.good && is_blocked(g));
        let good_refusal = count(&rows, |g| !g.critical && g.good && is_refusal(g));
        let weak = count(&rows, |g| !g.critical && !g.good);
        let weak_stopped = count(&rows, |g| !g.critical && !g.good && !g.verdict.is_pass());
        println!(
            "| {model} | {caught}/{critical} | {good_blocked}/{good} | {good_refusal}/{good} | {weak_stopped}/{weak} |"
        );
        if model == "all" {
            good_blocked_total = good_blocked;
        }
    }

    // qwen3.5-2b's partial-lead failure is an invented fact with a real citation, not a lost
    // negation or a changed amount, so no check here can see it.
    assert_eq!(missed, ["qwen3.5-2b/partial-lead"], "critical failures not caught");
    assert!(
        good_blocked_total <= 1,
        "too many good answers blocked: {good_blocked_total}"
    );
}

/// The 4-core CPU run answered a subset of the cases again, often in other words. It has no
/// grades, so this only prints what the checks stop, as a guard against tuning to the GPU answers.
#[test]
fn checks_on_the_cpu_run_answers() {
    let dir = bench_dir();
    let cases = read_json(&dir.join("cases.json"));
    let cases = cases["cases"].as_array().expect("cases");
    let mut files: Vec<PathBuf> = fs::read_dir(dir.join("results/cpu4"))
        .expect("results/cpu4")
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    files.sort();
    let (mut total, mut stopped) = (0, 0);
    println!("\nCPU run, answers not shown as written:");
    for file in files {
        let results = read_json(&file);
        let model = file.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        for row in results["rows"].as_array().expect("rows") {
            let id = str_field(row, "id");
            let case = cases.iter().find(|c| c["id"] == id).expect("case for row");
            let verdict = benchmark_prompt(case).check(str_field(row, "answer")).verdict;
            total += 1;
            if !verdict.is_pass() {
                stopped += 1;
                println!("  {model} / {id}: {verdict:?}");
            }
        }
    }
    println!("CPU run: {stopped} of {total} answers not shown as written");
}
