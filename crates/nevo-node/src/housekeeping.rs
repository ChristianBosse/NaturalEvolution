//! Background storage housekeeping: deletes files that no version uses.

use std::sync::Arc;
use std::time::Duration;

use nevo_store::{GcReport, Store, GC_GRACE};

use crate::pages;

const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Runs garbage collection now and then every 6 hours, on a blocking thread so the web server
/// keeps serving. Starting it doesn't wait for the first run, so startup isn't delayed.
pub fn spawn(store: Arc<Store>) {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(EVERY);
        // A Pi that was asleep or busy runs once when it can, not several times in a row.
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await; // the first tick is immediate
            let store = store.clone();
            match tokio::task::spawn_blocking(move || store.collect_garbage(GC_GRACE)).await {
                Ok(Ok(report)) => println!("{}", summary(&report)),
                Ok(Err(e)) => eprintln!("cleanup: {e}"),
                Err(e) => eprintln!("cleanup: {e}"),
            }
        }
    });
}

fn summary(r: &GcReport) -> String {
    let mut line = format!(
        "cleanup: checked {} files, removed {} unused ({})",
        r.scanned,
        r.removed,
        pages::size(r.bytes_freed)
    );
    if r.failed > 0 {
        line += &format!(", {} errors, first: {}", r.failed, r.errors.first().map_or("", |e| e));
    }
    line
}
