//! Background jobs of the server's machine (a clone, a transfer's export or
//! import, a removal after a move): started by one request, they run on
//! their own thread and are asked about with `status` requests, so a long
//! operation never holds an API connection and goes on when the dialog
//! that started it is closed.

use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// A finished job is forgotten this long after it ended.
const KEEP_FINISHED: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }
}

/// What a job reports: `{state, phase, percent, message, ...extra}`.
#[derive(Debug, Clone)]
pub struct Progress {
    pub state: State,
    pub phase: String,
    pub percent: Option<u8>,
    pub message: Option<String>,
    pub extra: Map<String, Value>,
}

impl Progress {
    pub fn to_json(&self) -> Value {
        let mut out = self.extra.clone();
        out.insert("state".into(), json!(self.state.as_str()));
        out.insert("phase".into(), json!(self.phase));
        out.insert("percent".into(), json!(self.percent));
        out.insert("message".into(), json!(self.message));
        Value::Object(out)
    }
}

pub struct Job {
    progress: Mutex<Progress>,
    ended: Mutex<Option<Instant>>,
    cancelled: AtomicBool,
    /// The process the job waits on, killed by [`Job::cancel`].
    child: Mutex<Option<Child>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Job {
    pub fn progress(&self) -> Progress {
        lock(&self.progress).clone()
    }

    /// Phase and percent of a running job (ignored once it ended).
    pub fn set_phase(&self, phase: &str, percent: Option<u8>) {
        let mut p = lock(&self.progress);
        if p.state == State::Running {
            p.phase = phase.to_string();
            p.percent = percent;
        }
    }

    pub fn set_extra(&self, key: &str, value: Value) {
        lock(&self.progress).extra.insert(key.into(), value);
    }

    fn end(&self, state: State, message: Option<String>) {
        let mut p = lock(&self.progress);
        if p.state != State::Running {
            return;
        }
        p.state = state;
        p.message = message;
        if state == State::Done {
            p.percent = Some(100);
        }
        *lock(&self.ended) = Some(Instant::now());
    }

    pub fn finish(&self, message: Option<String>) {
        self.end(State::Done, message);
    }

    pub fn fail(&self, message: String) {
        if self.is_cancelled() {
            self.end(State::Cancelled, Some("cancelled".into()));
        } else {
            self.end(State::Failed, Some(message));
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        lock(&self.progress).state == State::Running
    }

    /// Asks the job to stop: its process is killed, its thread sees
    /// [`Job::is_cancelled`] and ends as `cancelled`.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    /// Keeps `child` so [`Job::cancel`] can kill it; returns it with
    /// [`Job::take_child`] to wait on it.
    pub fn hold_child(&self, child: Child) {
        *lock(&self.child) = Some(child);
        if self.is_cancelled() {
            self.cancel();
        }
    }

    pub fn take_child(&self) -> Option<Child> {
        lock(&self.child).take()
    }
}

/// The running and recently finished jobs of one kind, by id.
#[derive(Default)]
pub struct Jobs {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl Jobs {
    /// A new running job in `phase`; its id is random.
    pub fn start(&self, phase: &str) -> crate::Result<(String, Arc<Job>)> {
        let id = crate::random_id()?;
        let job = Arc::new(Job {
            progress: Mutex::new(Progress {
                state: State::Running,
                phase: phase.to_string(),
                percent: None,
                message: None,
                extra: Map::new(),
            }),
            ended: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        let mut jobs = lock(&self.jobs);
        jobs.retain(|_, j| {
            lock(&j.ended)
                .map(|t| t.elapsed() < KEEP_FINISHED)
                .unwrap_or(true)
        });
        jobs.insert(id.clone(), job.clone());
        Ok((id, job))
    }

    pub fn get(&self, id: &str) -> crate::Result<Arc<Job>> {
        lock(&self.jobs).get(id).cloned().ok_or_else(|| {
            format!("no job {id} on this machine (it ended long ago or the server restarted)")
        })
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Job>> {
        lock(&self.jobs).remove(id)
    }
}
