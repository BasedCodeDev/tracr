//! Opt-in launch timing. Only stage names, timings, counts and source ordinals leave the process.
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default)]
pub struct Trace {
    state: Option<Arc<State>>,
    source: Option<usize>,
}

struct State {
    launched: Instant,
    file: Mutex<Option<File>>,
    next_span: AtomicU64,
}

#[derive(Serialize)]
struct Event {
    stage: &'static str,
    event: &'static str,
    elapsed_us: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_us: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
}

impl Trace {
    pub fn open(directory: &Path, launched: Instant) -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = directory.join(format!(
            "tracr-startup-{}-{stamp}.jsonl",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        Ok(Self {
            state: Some(Arc::new(State {
                launched,
                file: Mutex::new(Some(file)),
                next_span: AtomicU64::new(0),
            })),
            source: None,
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn for_source(&self, source: usize) -> Self {
        Self {
            state: self.state.clone(),
            source: Some(source),
        }
    }

    pub fn mark(&self, stage: &'static str, count: Option<usize>) {
        self.record(stage, "mark", None, None, count);
    }

    pub(crate) fn finish(&self) {
        self.mark("startup_settled", None);
        if let Some(state) = &self.state {
            let mut file = state.file.lock().unwrap_or_else(|error| error.into_inner());
            *file = None;
        }
    }

    pub(crate) fn span(&self, stage: &'static str) -> Span {
        let active = self.state.as_ref().map(|state| {
            let id = state.next_span.fetch_add(1, Ordering::Relaxed);
            self.record(stage, "start", Some(id), None, None);
            (self.clone(), id, Instant::now())
        });
        Span { stage, active }
    }

    fn record(
        &self,
        stage: &'static str,
        event: &'static str,
        span: Option<u64>,
        duration_us: Option<u128>,
        count: Option<usize>,
    ) {
        let Some(state) = &self.state else { return };
        let mut file = state.file.lock().unwrap_or_else(|error| error.into_inner());
        let Some(output) = file.as_mut() else { return };
        let record = Event {
            stage,
            event,
            elapsed_us: state.launched.elapsed().as_micros(),
            source: self.source,
            span,
            duration_us,
            count,
        };
        let mut line = serde_json::to_vec(&record).expect("serialize startup timing");
        line.push(b'\n');
        // An unbuffered line is visible even if the next stage stalls. Stop
        // tracing on I/O failure without affecting the browser.
        if output.write_all(&line).is_err() {
            *file = None;
        }
    }
}

pub(crate) struct Span {
    stage: &'static str,
    active: Option<(Trace, u64, Instant)>,
}

impl Span {
    pub(crate) fn finish(mut self, outcome: &'static str, count: Option<usize>) {
        self.end(outcome, count);
    }

    fn end(&mut self, outcome: &'static str, count: Option<usize>) {
        if let Some((trace, id, start)) = self.active.take() {
            trace.record(
                self.stage,
                outcome,
                Some(id),
                Some(start.elapsed().as_micros()),
                count,
            );
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        self.end("cancelled", None);
    }
}
