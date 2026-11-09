//! The store thread: every file goz-gui writes is written here, so the
//! window never waits on the disk.
//!
//! Jobs are queued per file. A job queued for a file that already has one
//! waiting replaces it (the latest snapshot wins), so a burst of changes
//! costs one write. A job reads what it writes from shared state when it
//! runs, not when it was queued, so a replaced job loses nothing. At exit
//! and at the end of the Windows session the window waits for the queue to
//! drain, for a bounded time.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    queue: VecDeque<(PathBuf, Job)>,
    /// A job is running.
    busy: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when a job is queued and when one has finished.
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The store thread's queue.
pub struct Writer {
    shared: Arc<Shared>,
    /// Without a thread (it could not be started) jobs run on the caller's.
    threaded: bool,
}

impl Writer {
    pub fn spawn() -> Writer {
        let shared = Arc::new(Shared::default());
        let theirs = Arc::clone(&shared);
        let threaded = std::thread::Builder::new()
            .name("goz-store".into())
            .spawn(move || run(&theirs))
            .is_ok();
        Writer { shared, threaded }
    }

    /// Queues `job` for `file`, in place of any job for it still waiting.
    pub fn submit(&self, file: &Path, job: Job) {
        if !self.threaded {
            job();
            return;
        }
        let mut state = self.shared.lock();
        match state.queue.iter_mut().find(|(f, _)| f == file) {
            Some(slot) => slot.1 = job,
            None => state.queue.push_back((file.to_owned(), job)),
        }
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Waits until every queued job has run, at most `timeout`. True when
    /// nothing is left to write.
    pub fn flush(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut state = self.shared.lock();
        while state.busy || !state.queue.is_empty() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
}

fn run(shared: &Shared) {
    loop {
        let mut state = shared.lock();
        let job = loop {
            if let Some((_, job)) = state.queue.pop_front() {
                break job;
            }
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        };
        state.busy = true;
        drop(state);
        job();
        shared.lock().busy = false;
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_waiting_job_is_replaced_by_the_next_for_its_file() {
        let writer = Writer::spawn();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (ran_tx, ran_rx) = mpsc::channel::<&'static str>();
        // Hold the thread on a first job, then queue three for another file
        // and one for a third.
        let tx = ran_tx.clone();
        writer.submit(
            Path::new("a"),
            Box::new(move || {
                gate_rx.recv().ok();
                tx.send("a").ok();
            }),
        );
        for name in ["b1", "b2", "b3"] {
            let tx = ran_tx.clone();
            writer.submit(
                Path::new("b"),
                Box::new(move || {
                    tx.send(name).ok();
                }),
            );
        }
        let tx = ran_tx.clone();
        writer.submit(
            Path::new("c"),
            Box::new(move || {
                tx.send("c").ok();
            }),
        );
        assert!(!writer.flush(Duration::from_millis(50)), "still held");
        gate_tx.send(()).expect("release");
        assert!(writer.flush(Duration::from_secs(5)));
        drop(ran_tx);
        let ran: Vec<&str> = ran_rx.iter().collect();
        assert_eq!(ran, ["a", "b3", "c"]);
    }

    #[test]
    fn flush_with_nothing_queued_returns_at_once() {
        let writer = Writer::spawn();
        assert!(writer.flush(Duration::ZERO));
    }
}
