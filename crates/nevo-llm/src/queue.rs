//! Requests take turns, first come first served.
//!
//! A 4 GB node has room for the model and one context, not two (D-013, D-033), so only one request runs at a time.
//! Waiting requests learn how many are ahead of them, so the page can say "2 people ahead of you".

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::Cancel;

/// How often a waiting request checks whether it was cancelled.
const POLL: Duration = Duration::from_millis(100);

pub(crate) struct Queue {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// Waiting requests, oldest first.
    waiting: VecDeque<u64>,
    running: bool,
    next_id: u64,
}

/// The right to run. The next request may start when it is dropped.
pub(crate) struct Turn<'a> {
    queue: &'a Queue,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.queue.lock().running = false;
        self.queue.changed.notify_all();
    }
}

impl Queue {
    pub(crate) fn new() -> Queue {
        Queue {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    // A panic while holding the lock leaves the state consistent (every change is a single step), so a poisoned
    // lock is still usable.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Number of requests waiting, not counting the one running.
    pub(crate) fn waiting(&self) -> usize {
        self.lock().waiting.len()
    }

    pub(crate) fn busy(&self) -> bool {
        self.lock().running
    }

    /// Waits for this request's turn.
    ///
    /// `on_ahead` is called with the number of requests ahead (running or waiting) whenever that number changes; it
    /// isn't called if the turn comes at once. Returns `None` if `cancel` fires or `on_ahead` returns false first.
    pub(crate) fn wait_turn(&self, cancel: &Cancel, mut on_ahead: impl FnMut(usize) -> bool) -> Option<Turn<'_>> {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.waiting.push_back(id);
        let mut told = None;
        loop {
            let place = state
                .waiting
                .iter()
                .position(|&w| w == id)
                .expect("a request stays queued until it leaves");
            let ahead = place + usize::from(state.running);
            if ahead == 0 {
                state.waiting.pop_front();
                state.running = true;
                return Some(Turn { queue: self });
            }
            if cancel.is_cancelled() {
                return self.leave(state, id);
            }
            if told != Some(ahead) {
                told = Some(ahead);
                // The callback may block (sending to a slow client), so it runs without the lock.
                drop(state);
                let go_on = on_ahead(ahead);
                state = self.lock();
                if !go_on {
                    return self.leave(state, id);
                }
                continue;
            }
            state = self
                .changed
                .wait_timeout(state, POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn leave(&self, mut state: MutexGuard<'_, State>, id: u64) -> Option<Turn<'_>> {
        state.waiting.retain(|&w| w != id);
        drop(state);
        // Everyone behind moves up one place.
        self.changed.notify_all();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;

    #[derive(Debug, PartialEq)]
    enum Seen {
        Ahead(&'static str, usize),
        Started(&'static str),
        Left(&'static str),
    }

    /// Starts a request on another thread. It reports what it sees and holds its turn until `release` gets a value.
    fn request(
        queue: &Arc<Queue>,
        name: &'static str,
        cancel: Cancel,
        seen: mpsc::Sender<Seen>,
    ) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let (release, hold) = mpsc::channel();
        let queue = Arc::clone(queue);
        let handle = thread::spawn(move || {
            let turn = queue.wait_turn(&cancel, |ahead| seen.send(Seen::Ahead(name, ahead)).is_ok());
            match turn {
                Some(_turn) => {
                    seen.send(Seen::Started(name)).unwrap();
                    let _ = hold.recv();
                }
                None => seen.send(Seen::Left(name)).unwrap(),
            }
        });
        (release, handle)
    }

    #[test]
    fn idle_queue_runs_at_once() {
        let queue = Queue::new();
        let turn = queue.wait_turn(&Cancel::new(), |_| panic!("nobody is ahead"));
        assert!(turn.is_some());
        assert!(queue.busy());
        assert_eq!(queue.waiting(), 0);
        drop(turn);
        assert!(!queue.busy());
    }

    #[test]
    fn requests_run_in_order_and_learn_their_place() {
        let queue = Arc::new(Queue::new());
        let first = queue.wait_turn(&Cancel::new(), |_| true).unwrap();
        let (tx, seen) = mpsc::channel();

        let (release_a, a) = request(&queue, "a", Cancel::new(), tx.clone());
        assert_eq!(seen.recv().unwrap(), Seen::Ahead("a", 1));
        let (release_b, b) = request(&queue, "b", Cancel::new(), tx.clone());
        assert_eq!(seen.recv().unwrap(), Seen::Ahead("b", 2));
        assert_eq!(queue.waiting(), 2);

        drop(first);
        let mut events = vec![seen.recv().unwrap(), seen.recv().unwrap()];
        events.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(events, [Seen::Ahead("b", 1), Seen::Started("a")]);
        assert_eq!(queue.waiting(), 1);

        release_a.send(()).unwrap();
        assert_eq!(seen.recv().unwrap(), Seen::Started("b"));
        release_b.send(()).unwrap();
        a.join().unwrap();
        b.join().unwrap();
        assert!(!queue.busy());
    }

    #[test]
    fn cancelled_request_leaves_and_others_move_up() {
        let queue = Arc::new(Queue::new());
        let first = queue.wait_turn(&Cancel::new(), |_| true).unwrap();
        let (tx, seen) = mpsc::channel();

        let cancel_a = Cancel::new();
        let (_release_a, a) = request(&queue, "a", cancel_a.clone(), tx.clone());
        assert_eq!(seen.recv().unwrap(), Seen::Ahead("a", 1));
        let (release_b, b) = request(&queue, "b", Cancel::new(), tx.clone());
        assert_eq!(seen.recv().unwrap(), Seen::Ahead("b", 2));

        cancel_a.cancel();
        let mut events = vec![seen.recv().unwrap(), seen.recv().unwrap()];
        events.sort_by_key(|e| format!("{e:?}"));
        assert_eq!(events, [Seen::Ahead("b", 1), Seen::Left("a")]);
        a.join().unwrap();

        drop(first);
        assert_eq!(seen.recv().unwrap(), Seen::Started("b"));
        release_b.send(()).unwrap();
        b.join().unwrap();
    }

    #[test]
    fn callback_can_give_up_the_place() {
        let queue = Queue::new();
        let _first = queue.wait_turn(&Cancel::new(), |_| true).unwrap();
        assert!(queue.wait_turn(&Cancel::new(), |_| false).is_none());
        assert_eq!(queue.waiting(), 0);
    }
}
