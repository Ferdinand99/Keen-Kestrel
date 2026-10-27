//! The symbolication queue: events that can be symbolicated (their project
//! has a debug file for one of their images) leave ingest's hot path here.
//! Ingest answers the SDK at once; a few workers symbolicate, group and
//! hand the envelope's jobs to the writer in order. A full queue makes
//! ingest answer 429, like a full writer queue; it is full when it holds
//! `queue_capacity` envelopes or `queue_max_bytes` of their payloads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::{Mutex, Notify, mpsc};

use super::Difs;
use super::symbolicate;
use crate::event::{self, Prepared};
use crate::writer::{DifUse, Job, Writer};

/// One item of an envelope on its way to the writer.
#[derive(Debug)]
pub enum Work {
    /// Ready as it is.
    Job(Job),
    /// An event to symbolicate, then group.
    Event(Box<Prepared>),
}

/// The queue is full: answer 429.
#[derive(Debug, PartialEq, Eq)]
pub struct QueueFull;

/// How the queue runs.
#[derive(Debug, Clone, Copy)]
pub struct WorkerSettings {
    /// Envelopes waiting at most.
    pub queue_capacity: usize,
    /// Bytes of event payloads waiting at most (the parsed events are a few
    /// times as big).
    pub queue_max_bytes: usize,
    pub workers: usize,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        WorkerSettings {
            queue_capacity: 256,
            queue_max_bytes: 32 * 1024 * 1024,
            workers: 2,
        }
    }
}

/// Counters, for tests and logs.
#[derive(Debug, Default)]
pub struct Stats {
    pub events: AtomicU64,
    pub frames_symbolicated: AtomicU64,
}

struct InFlight {
    n: AtomicUsize,
    /// Payload bytes of the envelopes waiting or being worked on.
    bytes: AtomicUsize,
    idle: Notify,
}

/// One envelope's work and what it weighs.
struct Queued {
    items: Vec<Work>,
    bytes: usize,
}

/// The handle ingest submits through.
#[derive(Clone)]
pub struct Symbolicator {
    tx: mpsc::Sender<Queued>,
    max_bytes: usize,
    in_flight: Arc<InFlight>,
    stats: Arc<Stats>,
}

impl Symbolicator {
    /// Starts the workers.
    pub fn start(difs: Difs, writer: Writer, settings: WorkerSettings) -> Symbolicator {
        let (tx, rx) = mpsc::channel(settings.queue_capacity.max(1));
        let rx = Arc::new(Mutex::new(rx));
        let in_flight = Arc::new(InFlight {
            n: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            idle: Notify::new(),
        });
        let stats = Arc::new(Stats::default());
        for _ in 0..settings.workers.max(1) {
            tokio::spawn(run(
                rx.clone(),
                difs.clone(),
                writer.clone(),
                in_flight.clone(),
                stats.clone(),
            ));
        }
        Symbolicator {
            tx,
            max_bytes: settings.queue_max_bytes.max(1),
            in_flight,
            stats,
        }
    }

    /// Whether the queue has room for another envelope.
    pub fn has_room(&self) -> bool {
        self.tx.capacity() > 0 && self.in_flight.bytes.load(Ordering::Acquire) < self.max_bytes
    }

    /// Queues one envelope's items, which hold `bytes` of event payloads.
    /// Refused when the queue is full or would go over its byte budget (an
    /// empty queue takes any envelope).
    pub fn submit(&self, items: Vec<Work>, bytes: usize) -> Result<(), QueueFull> {
        let before = self.in_flight.bytes.fetch_add(bytes, Ordering::AcqRel);
        if before > 0 && before + bytes > self.max_bytes {
            self.in_flight.bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err(QueueFull);
        }
        self.in_flight.n.fetch_add(1, Ordering::SeqCst);
        if self.tx.try_send(Queued { items, bytes }).is_err() {
            self.in_flight.bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.done();
            return Err(QueueFull);
        }
        Ok(())
    }

    fn done(&self) {
        done(&self.in_flight);
    }

    /// Waits until everything queued so far went to the writer.
    pub async fn flush(&self) {
        loop {
            let idle = self.in_flight.idle.notified();
            if self.in_flight.n.load(Ordering::SeqCst) == 0 {
                return;
            }
            idle.await;
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }
}

fn done(in_flight: &InFlight) {
    if in_flight.n.fetch_sub(1, Ordering::SeqCst) == 1 {
        in_flight.idle.notify_waiters();
    }
}

async fn run(
    rx: Arc<Mutex<mpsc::Receiver<Queued>>>,
    difs: Difs,
    writer: Writer,
    in_flight: Arc<InFlight>,
    stats: Arc<Stats>,
) {
    loop {
        let next = rx.lock().await.recv().await;
        let Some(Queued { items, bytes }) = next else {
            return;
        };
        let (d, s) = (difs.clone(), stats.clone());
        match tokio::task::spawn_blocking(move || process(&d, items, &s)).await {
            Ok(jobs) => {
                if writer.submit_wait(jobs).await.is_err() {
                    tracing::error!("the writer has stopped; dropped symbolicated events");
                }
            }
            Err(err) => tracing::error!(%err, "symbolication panicked; dropped an envelope"),
        }
        in_flight.bytes.fetch_sub(bytes, Ordering::AcqRel);
        done(&in_flight);
    }
}

/// Symbolicates and groups the events of one envelope.
pub fn process(difs: &Difs, items: Vec<Work>, stats: &Stats) -> Vec<Job> {
    let mut jobs = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Work::Job(job) => jobs.push(job),
            Work::Event(mut p) => {
                let project_id = p.project_id;
                let report =
                    symbolicate::symbolicate(&mut p.event, &|id| difs.symcache(project_id, id));
                stats.events.fetch_add(1, Ordering::Relaxed);
                stats
                    .frames_symbolicated
                    .fetch_add(report.symbolicated as u64, Ordering::Relaxed);
                let release = p
                    .event
                    .get("release")
                    .and_then(|r| r.as_str())
                    .map(str::to_string);
                let due = difs.note_used(project_id, &report.used, release.as_deref());
                if !due.is_empty() {
                    jobs.push(Job::DifUse(DifUse {
                        project_id,
                        debug_ids: due.iter().map(|d| d.to_string()).collect(),
                        release,
                        at: event::normalize::sql_time(event::normalize::ts_seconds(p.received)),
                    }));
                }
                jobs.push(Job::Event(Box::new(event::finish(*p))));
            }
        }
    }
    jobs
}
