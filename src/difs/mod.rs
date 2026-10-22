//! Debug information files and symbolication (docs/design.md §4.8).
//!
//! - [`files`]: object files on disk: each object (one per debug id and
//!   architecture) with the SymCache made from it, chunks, zip archives;
//! - [`upload`]: sentry-cli's chunk-upload protocol, the legacy dSYM
//!   endpoint and the zip endpoint of the REST API, with their limits and
//!   the `difs:write` grant;
//! - [`symbolicate`]: a native event's frames mapped through
//!   `debug_meta.images` to SymCaches, inlined frames expanded, Swift
//!   demangled;
//! - [`worker`]: the bounded queue that symbolicates events off ingest's
//!   hot path before they are grouped and written;
//! - [`pages`]: the project's Debug files page.
//!
//! Layout under `<data>/difs/`:
//!
//! ```text
//! chunks/<sha1>                    uploaded chunks (gone once assembled, or after a day)
//! tmp/                             files being assembled or unpacked
//! <project_id>/<debug_id>/object   the object (one architecture of a dSYM)
//! <project_id>/<debug_id>/symcache its SymCache, rebuilt from the object
//!                                  when symbolic's format changes
//! ```
//!
//! The rows are in `dif_files` ([`crate::db::difs`]); an in-memory index of
//! `(project, debug id)` lets ingest tell in a hash lookup whether an event
//! can be symbolicated at all.

pub mod files;
pub mod pages;
pub mod symbolicate;
pub mod upload;
pub mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use symbolic_common::{ByteView, DebugId};
use tokio::sync::watch;

/// The debug files' directory in the data directory.
pub const DIR: &str = "difs";
/// SymCaches kept open (memory-mapped) at once.
const OPEN_CACHES: usize = 16;
/// How often a debug file's `last_used_at` is written at most.
const USE_EVERY: Duration = Duration::from_secs(600);
/// How long a finished assembly's outcome is reported (an error before a
/// retry may start; success for an upload that only confirmed a debug file
/// already stored).
const FINISHED_KEPT: Duration = Duration::from_secs(600);
/// Debug files processed at once (SymCache conversion of a large dSYM takes
/// memory).
const PROCESSING: usize = 2;

/// The outcome of assembling one uploaded file: the `dif_files` ids, or why
/// it failed.
pub type AssemblyResult = Result<Vec<i64>, String>;
/// Where an assembly's outcome arrives.
pub type AssemblyRx = watch::Receiver<Option<AssemblyResult>>;
type AssemblyTx = watch::Sender<Option<AssemblyResult>>;
/// Memory-mapped SymCaches by project and debug id, most recently used last.
type OpenCaches = Vec<((i64, DebugId), Arc<ByteView<'static>>)>;
/// When each debug file's use was last written, and for which release.
type Used = HashMap<(i64, DebugId), (Instant, Option<String>)>;

/// An assembly in progress (or recently failed).
struct Assembly {
    started: Instant,
    result: AssemblyRx,
}

/// The debug files' shared state.
#[derive(Clone)]
pub struct Difs {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    index: RwLock<HashMap<i64, HashSet<DebugId>>>,
    open: Mutex<OpenCaches>,
    assembling: Mutex<HashMap<(i64, String), Assembly>>,
    used: Mutex<Used>,
    processing: tokio::sync::Semaphore,
}

/// A debug id as stored (lowercase, as symbolic writes it).
pub fn parse_debug_id(raw: &str) -> Option<DebugId> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    DebugId::from_str(raw).ok().filter(|d| !d.is_nil())
}

impl Difs {
    /// Opens `<data_dir>/difs` (creating it), empties its `tmp/` and loads
    /// the index from `ids` (`dif_files`).
    pub fn open(data_dir: &Path, ids: Vec<(i64, String)>) -> std::io::Result<Difs> {
        let root = data_dir.join(DIR);
        std::fs::create_dir_all(root.join("chunks"))?;
        let tmp = root.join("tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let mut index: HashMap<i64, HashSet<DebugId>> = HashMap::new();
        for (project, id) in ids {
            if let Some(id) = parse_debug_id(&id) {
                index.entry(project).or_default().insert(id);
            }
        }
        Ok(Difs {
            inner: Arc::new(Inner {
                root,
                index: RwLock::new(index),
                open: Mutex::new(Vec::new()),
                assembling: Mutex::new(HashMap::new()),
                used: Mutex::new(HashMap::new()),
                processing: tokio::sync::Semaphore::new(PROCESSING),
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.inner.root.join("tmp")
    }

    pub fn chunk_dir(&self) -> PathBuf {
        self.inner.root.join("chunks")
    }

    pub fn project_dir(&self, project_id: i64) -> PathBuf {
        self.inner.root.join(project_id.to_string())
    }

    pub fn dif_dir(&self, project_id: i64, debug_id: &DebugId) -> PathBuf {
        self.project_dir(project_id).join(debug_id.to_string())
    }

    /// Whether `project_id` has a debug file for `debug_id`.
    pub fn has(&self, project_id: i64, debug_id: &DebugId) -> bool {
        self.inner
            .index
            .read()
            .map(|i| i.get(&project_id).is_some_and(|s| s.contains(debug_id)))
            .unwrap_or(false)
    }

    /// Whether `project_id` has any debug file.
    pub fn has_any(&self, project_id: i64) -> bool {
        self.inner
            .index
            .read()
            .map(|i| i.get(&project_id).is_some_and(|s| !s.is_empty()))
            .unwrap_or(false)
    }

    pub(crate) fn add(&self, project_id: i64, debug_id: DebugId) {
        if let Ok(mut i) = self.inner.index.write() {
            i.entry(project_id).or_default().insert(debug_id);
        }
        self.close(project_id, &debug_id);
    }

    /// Forgets one debug file and removes its directory.
    pub fn remove(&self, project_id: i64, debug_id: &DebugId) {
        if let Ok(mut i) = self.inner.index.write()
            && let Some(set) = i.get_mut(&project_id)
        {
            set.remove(debug_id);
        }
        self.close(project_id, debug_id);
        let dir = self.dif_dir(project_id, debug_id);
        if let Err(err) = std::fs::remove_dir_all(&dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%err, dir = %dir.display(), "can't remove a debug file");
        }
    }

    /// Forgets a deleted project's debug files and removes them from disk.
    pub fn forget_project(&self, project_id: i64) {
        if let Ok(mut i) = self.inner.index.write() {
            i.remove(&project_id);
        }
        if let Ok(mut open) = self.inner.open.lock() {
            open.retain(|((p, _), _)| *p != project_id);
        }
        let dir = self.project_dir(project_id);
        if let Err(err) = std::fs::remove_dir_all(&dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%err, dir = %dir.display(), "can't remove a project's debug files");
        }
    }

    /// Project directories on disk whose project is not in `known`.
    pub fn stray_projects(&self, known: &HashSet<i64>) -> Vec<i64> {
        let Ok(entries) = std::fs::read_dir(&self.inner.root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok())
            .filter(|id| !known.contains(id))
            .collect()
    }

    /// Removes `project_id`'s debug file directories that have no row (an
    /// upload cut short) and are older than `min_age`.
    pub fn remove_unlisted(&self, project_id: i64, min_age: Duration) {
        let Ok(entries) = std::fs::read_dir(self.project_dir(project_id)) else {
            return;
        };
        for e in entries.flatten() {
            let Some(id) = e.file_name().to_str().and_then(parse_debug_id) else {
                continue;
            };
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > min_age);
            if old && !self.has(project_id, &id) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    fn close(&self, project_id: i64, debug_id: &DebugId) {
        if let Ok(mut open) = self.inner.open.lock() {
            open.retain(|(k, _)| *k != (project_id, *debug_id));
        }
    }

    /// The SymCache of `project_id`'s `debug_id`, memory-mapped. A SymCache
    /// symbolic can no longer read (a newer format) is made again from the
    /// stored object.
    pub fn symcache(&self, project_id: i64, debug_id: &DebugId) -> Option<Arc<ByteView<'static>>> {
        if !self.has(project_id, debug_id) {
            return None;
        }
        let key = (project_id, *debug_id);
        if let Ok(mut open) = self.inner.open.lock()
            && let Some(pos) = open.iter().position(|(k, _)| *k == key)
        {
            let entry = open.remove(pos);
            let view = entry.1.clone();
            open.push(entry);
            return Some(view);
        }
        let dir = self.dif_dir(project_id, debug_id);
        let view = match files::open_symcache(&dir) {
            Ok(view) => view,
            Err(err) => {
                tracing::warn!(%err, %debug_id, "a SymCache can't be read; making it again");
                match files::rebuild_symcache(&dir, debug_id) {
                    Ok(view) => view,
                    Err(err) => {
                        tracing::error!(%err, %debug_id, "can't make the SymCache again");
                        return None;
                    }
                }
            }
        };
        let view = Arc::new(view);
        if let Ok(mut open) = self.inner.open.lock() {
            if open.len() >= OPEN_CACHES {
                open.remove(0);
            }
            open.push((key, view.clone()));
        }
        Some(view)
    }

    /// Which of `debug_ids` (used by an event of `release`) should have
    /// their `last_used_at` written now: at most every ten minutes per file,
    /// or when the release changed.
    pub fn note_used(
        &self,
        project_id: i64,
        debug_ids: &[DebugId],
        release: Option<&str>,
    ) -> Vec<DebugId> {
        let now = Instant::now();
        let Ok(mut used) = self.inner.used.lock() else {
            return Vec::new();
        };
        if used.len() > 10_000 {
            used.retain(|_, (at, _)| now.duration_since(*at) < USE_EVERY);
        }
        debug_ids
            .iter()
            .filter(|id| {
                let key = (project_id, **id);
                let due = match used.get(&key) {
                    Some((at, rel)) => {
                        now.duration_since(*at) >= USE_EVERY || rel.as_deref() != release
                    }
                    None => true,
                };
                if due {
                    used.insert(key, (now, release.map(str::to_string)));
                }
                due
            })
            .copied()
            .collect()
    }

    /// The assembly of `(project_id, checksum)`: `None` when none is running
    /// or finished recently; otherwise its receiver.
    pub(crate) fn assembly(&self, project_id: i64, checksum: &str) -> Option<AssemblyRx> {
        let mut map = self.inner.assembling.lock().ok()?;
        map.retain(|_, a| a.result.borrow().is_none() || a.started.elapsed() < FINISHED_KEPT);
        map.get(&(project_id, checksum.to_string()))
            .map(|a| a.result.clone())
    }

    /// A turn to process a debug file (at most two at once).
    pub(crate) async fn processing(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.inner.processing.acquire().await.ok()
    }

    /// Registers a new assembly; `None` when one is already running.
    pub(crate) fn begin_assembly(
        &self,
        project_id: i64,
        checksum: &str,
    ) -> Option<(AssemblyTx, AssemblyRx)> {
        let mut map = self.inner.assembling.lock().ok()?;
        let key = (project_id, checksum.to_string());
        if map.get(&key).is_some_and(|a| a.result.borrow().is_none()) {
            return None;
        }
        let (tx, rx) = watch::channel(None);
        map.insert(
            key,
            Assembly {
                started: Instant::now(),
                result: rx.clone(),
            },
        );
        Some((tx, rx))
    }
}
