//! Runs one extraction on its own thread with a deadline, turning panics into errors.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use crate::ExtractError;

/// Stack for the worker. Parsers recurse (nested PDF objects, form XObjects); a stack overflow
/// aborts the process instead of panicking, so give them room. Only touched pages use memory.
const STACK_BYTES: usize = 64 * 1024 * 1024;

/// Tells a running extraction to stop: set by the caller on timeout, or when the deadline passes.
pub(crate) struct Cancel {
    flag: Arc<AtomicBool>,
    deadline: Instant,
}

impl Cancel {
    pub(crate) fn stopped(&self) -> bool {
        self.flag.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }

    pub(crate) fn check(&self) -> Result<(), ExtractError> {
        if self.stopped() {
            Err(ExtractError::TimedOut)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn run<T, F>(timeout: Duration, f: F) -> Result<T, ExtractError>
where
    T: Send + 'static,
    F: FnOnce(&Cancel) -> Result<T, ExtractError> + Send + 'static,
{
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = Cancel {
        flag: flag.clone(),
        deadline: Instant::now() + timeout,
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("nevo-extract".into())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| f(&cancel)))
                .unwrap_or_else(|payload| Err(ExtractError::Panicked(panic_message(payload.as_ref()))));
            // The caller may have given up already; nobody is listening then.
            let _ = tx.send(result);
        });
    if let Err(e) = worker {
        return Err(ExtractError::Panicked(format!("could not start worker thread: {e}")));
    }
    // The worker watches the deadline itself; the extra second covers a worker stuck where it
    // can't look (inside a parser). It is then left running, with its flag set.
    match rx.recv_timeout(timeout + Duration::from_secs(1)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            flag.store(true, Ordering::Relaxed);
            Err(ExtractError::TimedOut)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(ExtractError::Panicked("worker thread died".into())),
    }
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}
