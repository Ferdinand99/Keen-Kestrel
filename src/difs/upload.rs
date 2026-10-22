//! Uploading debug files (docs/design.md §4.8).
//!
//! sentry-cli's protocol, so `sentry-cli debug-files upload`, fastlane's
//! Sentry plugin and Xcode build phases work unchanged with
//! `--url <Kestrel> --org <anything> --project <slug or id>`:
//!
//! 1. `GET /api/0/organizations/{org}/chunk-upload/`: the chunk size and
//!    limits;
//! 2. `POST /api/0/projects/{org}/{project}/files/difs/assemble/` with each
//!    file's SHA-1 and its chunks' SHA-1s: `not_found` with the missing
//!    chunks, or the file's state (`assembling`, `ok` with the stored
//!    debug file, `error` with why);
//! 3. `POST /api/0/organizations/{org}/chunk-upload/`: the missing chunks
//!    (multipart, `file` or `file_gzip`, named by their SHA-1);
//! 4. assemble again: the file is joined, checked, and each object in it is
//!    stored with its SymCache. A small file is done within the request;
//!    a large one answers `assembling` until it is.
//!
//! Also the legacy `…/files/dsyms/` endpoint (a zip or an object file as
//! multipart `file`) and `POST /api/v1/projects/{id}/dsyms` (a zip as the
//! body or as multipart `file`).
//!
//! Every upload needs an API token (`Authorization: Bearer kk_…`) that may
//! upload debug files: one with the `difs:write` grant (made on the Debug
//! files page), or a read-and-write token, and its owner must be an editor
//! or admin. The REST endpoint also takes an editor's session.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Extension, Multipart, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};
use symbolic_common::DebugId;
use vole_kit::auth::{AuthMethod, CurrentUser, TokenScope};
use vole_kit::http::rest::{V1Error, V1Result, json_ok, limit, no_content, paginate, parse_cursor};
use vole_kit::state::Core;

use super::Difs;
use super::files::{self, DifError, Stored};
use crate::db::difs::{self as db, DifFile, NewDif};
use crate::db::projects::{self, Project};
use crate::ingest::Ingest;

/// How long an assemble request waits for its files before answering
/// `assembling`.
const ASSEMBLE_WAIT: Duration = Duration::from_secs(10);

/// The protected `/api/0/` routes sentry-cli calls.
pub fn router() -> Router<Core> {
    let chunks = get(chunk_options)
        .post(chunk_upload)
        .layer(DefaultBodyLimit::max(files::MAX_REQUEST + 1024 * 1024));
    let dsyms = get(legacy_list)
        .post(legacy_upload)
        .layer(DefaultBodyLimit::disable());
    Router::new()
        .route("/api/0/", get(auth_info))
        .route("/api/0/organizations/{org}/chunk-upload/", chunks.clone())
        .route("/api/0/organizations/{org}/chunk-upload", chunks)
        .route(
            "/api/0/projects/{org}/{project}/files/difs/assemble/",
            post(assemble),
        )
        .route(
            "/api/0/projects/{org}/{project}/files/difs/assemble",
            post(assemble),
        )
        .route(
            "/api/0/projects/{org}/{project}/files/dsyms/",
            dsyms.clone(),
        )
        .route("/api/0/projects/{org}/{project}/files/dsyms", dsyms)
}

/// Whether `path` is one of sentry-cli's routes (token-only, so exempt from
/// the CSRF check).
pub fn is_sentry_cli_path(path: &str) -> bool {
    path == "/api/0" || path.starts_with("/api/0/")
}

// ---------------------------------------------------------------------------
// Answers and auth
// ---------------------------------------------------------------------------

/// A Sentry-style error: `{"detail": "…"}`.
fn detail(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, axum::Json(json!({ "detail": msg.to_string() }))).into_response()
}

/// Why an upload is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Not signed in with an API token (sentry-cli's routes take no session).
    NeedsToken,
    /// The token (or its owner) may not upload debug files.
    NotAllowed,
}

impl Denied {
    fn message(self) -> &'static str {
        match self {
            Denied::NeedsToken => "use an API token (Authorization: Bearer kk_…)",
            Denied::NotAllowed => {
                "this token can't upload debug files: use one with the difs:write grant \
                 (Debug files page) or a read-and-write token of an editor"
            }
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Denied::NeedsToken => StatusCode::UNAUTHORIZED,
            Denied::NotAllowed => StatusCode::FORBIDDEN,
        }
    }
}

/// Whether the request may upload debug files: a read-and-write token of
/// an editor or admin, a token with the `difs:write` grant whose owner is
/// one, or (when `session_ok`) an editor's session.
pub async fn may_upload(
    core: &Core,
    user: &CurrentUser,
    method: Option<AuthMethod>,
    session_ok: bool,
) -> Result<(), Denied> {
    match method {
        Some(AuthMethod::Token { token_id, scope }) => {
            if scope == TokenScope::Write && user.can_edit() {
                return Ok(());
            }
            let owner = user.id;
            let allowed = core
                .with_conn(move |c| {
                    if !db::token_has(c, token_id, db::DIFS_WRITE)? {
                        return Ok(false);
                    }
                    Ok(vole_kit::db::users::get_user(c, owner)?
                        .is_some_and(|u| u.active && u.role.can_edit()))
                })
                .await
                .unwrap_or(false);
            if allowed {
                Ok(())
            } else {
                Err(Denied::NotAllowed)
            }
        }
        _ if !session_ok => Err(Denied::NeedsToken),
        _ if user.can_edit() => Ok(()),
        _ => Err(Denied::NotAllowed),
    }
}

/// The project a sentry-cli path names: its slug or its numeric id.
async fn project(core: &Core, segment: &str) -> Option<Project> {
    let segment = segment.trim().to_string();
    core.with_conn(move |c| match segment.parse::<i64>() {
        Ok(id) if id > 0 => projects::get(c, id),
        _ => projects::by_slug(c, &segment),
    })
    .await
    .ok()
    .flatten()
}

/// The request's auth method, if `require_auth` set one.
type Method = Option<Extension<AuthMethod>>;

async fn check(core: &Core, user: &CurrentUser, method: Method) -> Option<Response> {
    may_upload(core, user, method.map(|Extension(m)| m), false)
        .await
        .err()
        .map(|d| detail(d.status(), d.message()))
}

/// A debug file as sentry-cli reads it (`DebugInfoFile`).
pub fn dif_json(d: &DifFile) -> Value {
    json!({
        "id": d.id.to_string(),
        "uuid": d.debug_id,
        "debugId": d.debug_id,
        "codeId": d.code_id,
        "objectName": d.object_name,
        "cpuName": d.arch,
        "symbolType": d.kind,
        "size": d.size,
        "sha1": d.sha1,
        "dateCreated": format!("{}Z", d.uploaded_at.replace(' ', "T")),
        "data": {"features": d.features},
    })
}

// ---------------------------------------------------------------------------
// sentry-cli
// ---------------------------------------------------------------------------

/// `GET /api/0/`: who the token belongs to and what it may do.
async fn auth_info(State(core): State<Core>, user: CurrentUser, method: Method) -> Response {
    let method = method.map(|Extension(m)| m);
    let mut scopes = vec!["org:read", "project:read"];
    if may_upload(&core, &user, method, false).await.is_ok() {
        scopes.extend(["project:write", "project:releases"]);
    }
    axum::Json(json!({
        "version": "0",
        "auth": {"scopes": scopes},
        "user": {"id": user.id.to_string(), "name": user.display_name, "username": user.username, "email": ""},
    }))
    .into_response()
}

/// `GET …/chunk-upload/`: the limits sentry-cli works with.
async fn chunk_options(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    UrlPath(org): UrlPath<String>,
) -> Response {
    if let Some(res) = check(&core, &user, method).await {
        return res;
    }
    let org: String = url::form_urlencoded::byte_serialize(org.as_bytes()).collect();
    axum::Json(json!({
        "url": format!("/api/0/organizations/{org}/chunk-upload/"),
        "chunkSize": files::CHUNK_SIZE,
        "chunksPerRequest": files::CHUNKS_PER_REQUEST,
        "maxFileSize": files::MAX_FILE,
        "maxRequestSize": files::MAX_REQUEST,
        "maxWait": 60,
        "concurrency": 4,
        "hashAlgorithm": "sha1",
        "compression": ["gzip"],
        "accept": ["debug_files"],
    }))
    .into_response()
}

/// `POST …/chunk-upload/`: chunks as multipart files named by their SHA-1
/// (`file`, or `file_gzip` gzip-compressed).
async fn chunk_upload(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    method: Method,
    mut form: Multipart,
) -> Response {
    if let Some(res) = check(&core, &user, method).await {
        return res;
    }
    let dir = ingest.difs().chunk_dir();
    let mut count = 0;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return detail(e.status(), e.body_text()),
        };
        count += 1;
        if count > files::CHUNKS_PER_REQUEST {
            return detail(StatusCode::BAD_REQUEST, "too many chunks in one request");
        }
        let gzip = field.name() == Some("file_gzip");
        let Some(checksum) = field.file_name().map(str::to_string) else {
            return detail(
                StatusCode::BAD_REQUEST,
                "a chunk without its SHA-1 as file name",
            );
        };
        let data = match field.bytes().await {
            Ok(d) => d,
            Err(e) => return detail(e.status(), e.body_text()),
        };
        let dir = dir.clone();
        let stored = tokio::task::spawn_blocking(move || {
            let data = if gzip {
                crate::ingest::body::decode(Some("gzip"), &data, files::CHUNK_SIZE)
                    .map_err(|_| DifError::Invalid("a broken or too large gzip chunk".into()))?
            } else {
                data.to_vec()
            };
            files::store_chunk(&dir, &checksum, &data)
        })
        .await;
        match stored {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return detail(StatusCode::BAD_REQUEST, e),
            Err(_) => return detail(StatusCode::INTERNAL_SERVER_ERROR, "storing a chunk failed"),
        }
    }
    StatusCode::OK.into_response()
}

#[derive(Debug, Deserialize)]
struct AssembleItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    chunks: Vec<String>,
}

fn state(state: &str, missing: &[String], detail: Option<String>, dif: Option<Value>) -> Value {
    let mut v = json!({"state": state, "missingChunks": missing, "detail": detail});
    if let Some(dif) = dif {
        v["dif"] = dif;
    }
    v
}

fn ok_state(rows: &[DifFile]) -> Value {
    state("ok", &[], None, rows.first().map(dif_json))
}

/// One file of an assemble request: answered now, or waiting for its
/// assembly.
enum Step {
    Done(Value),
    Wait(super::AssemblyRx),
}

async fn rows_by_id(core: &Core, project_id: i64, ids: Vec<i64>) -> Vec<DifFile> {
    core.with_conn(move |c| {
        let mut out = Vec::new();
        for id in ids {
            out.extend(db::get(c, project_id, id)?);
        }
        Ok(out)
    })
    .await
    .unwrap_or_default()
}

async fn finished(core: &Core, project_id: i64, result: &super::AssemblyResult) -> Value {
    match result {
        Ok(ids) => ok_state(&rows_by_id(core, project_id, ids.clone()).await),
        Err(e) => state("error", &[], Some(e.clone()), None),
    }
}

async fn step(
    core: &Core,
    difs: &Difs,
    project_id: i64,
    checksum: &str,
    item: AssembleItem,
) -> Step {
    if !files::is_sha1(checksum) || !item.chunks.iter().all(|c| files::is_sha1(c)) {
        return Step::Done(state("error", &[], Some("invalid checksum".into()), None));
    }
    let checksum = checksum.to_ascii_lowercase();
    let sha = checksum.clone();
    let rows = core
        .with_conn(move |c| db::by_sha1(c, project_id, &sha))
        .await
        .unwrap_or_default();
    if !rows.is_empty() {
        return Step::Done(ok_state(&rows));
    }
    if let Some(rx) = difs.assembly(project_id, &checksum) {
        let result = rx.borrow().clone();
        return match result {
            Some(r) => Step::Done(finished(core, project_id, &r).await),
            None => Step::Wait(rx),
        };
    }
    let chunk_dir = difs.chunk_dir();
    let missing: Vec<String> = item
        .chunks
        .iter()
        .filter(|c| !files::chunk_path(&chunk_dir, c).is_file())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if item.chunks.is_empty() || !missing.is_empty() {
        return Step::Done(state("not_found", &missing, None, None));
    }
    let Some((tx, rx)) = difs.begin_assembly(project_id, &checksum) else {
        return match difs.assembly(project_id, &checksum) {
            Some(rx) => Step::Wait(rx),
            None => Step::Done(state("assembling", &[], None, None)),
        };
    };
    let (core, difs) = (core.clone(), difs.clone());
    tokio::spawn(async move {
        let name = files::object_name(&item.name);
        let result = assemble_file(&core, &difs, project_id, &checksum, &name, item.chunks).await;
        if let Err(e) = &result {
            tracing::info!(project = project_id, file = %name, error = %e, "a debug file was refused");
        }
        let _ = tx.send(Some(result));
    });
    Step::Wait(rx)
}

/// Joins a file from its chunks, stores its objects and their rows.
async fn assemble_file(
    core: &Core,
    difs: &Difs,
    project_id: i64,
    checksum: &str,
    name: &str,
    chunks: Vec<String>,
) -> super::AssemblyResult {
    let _turn = difs.processing().await;
    let existing = existing(core, project_id).await;
    let (d, sum) = (difs.clone(), checksum.to_string());
    let stored = tokio::task::spawn_blocking(move || {
        let path = files::assemble(&d.chunk_dir(), &d.tmp_dir(), &sum, &chunks)?;
        let stored = files::store_objects(&path, &d.project_dir(project_id), &existing);
        let _ = std::fs::remove_file(&path);
        if stored.is_ok() {
            for c in &chunks {
                let _ = std::fs::remove_file(files::chunk_path(&d.chunk_dir(), c));
            }
        }
        stored
    })
    .await
    .map_err(|_| "processing the file failed".to_string())?
    .map_err(|e| e.to_string())?;
    let rows = save(core, difs, project_id, checksum, name, stored)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(project = project_id, file = %name, objects = rows.len(), "debug file stored");
    Ok(rows.iter().map(|r| r.id).collect())
}

/// Which debug ids of `project_id` already have debug information.
async fn existing(core: &Core, project_id: i64) -> HashMap<DebugId, bool> {
    core.with_conn(move |c| db::list(c, project_id))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| Some((super::parse_debug_id(&d.debug_id)?, d.has_debug_info())))
        .collect()
}

/// Writes the rows of stored objects and adds them to the index. Objects
/// that kept an existing debug file give that one's row.
async fn save(
    core: &Core,
    difs: &Difs,
    project_id: i64,
    sha1: &str,
    name: &str,
    stored: Vec<Stored>,
) -> Result<Vec<DifFile>, vole_kit::http::ApiError> {
    let (sha1, name) = (sha1.to_string(), name.to_string());
    let list = stored.clone();
    let rows = core
        .with_conn(move |c| {
            let mut rows = Vec::new();
            for s in &list {
                let id = s.debug_id.to_string();
                if !s.kept_existing {
                    db::upsert(
                        c,
                        &NewDif {
                            project_id,
                            debug_id: id.clone(),
                            code_id: s.code_id.clone(),
                            object_name: name.clone(),
                            arch: s.arch.clone(),
                            kind: s.kind.clone(),
                            features: s.features.clone(),
                            size: i64::try_from(s.size).unwrap_or(i64::MAX),
                            symcache_size: i64::try_from(s.symcache_size).unwrap_or(i64::MAX),
                            sha1: sha1.clone(),
                        },
                    )?;
                }
                rows.extend(db::by_debug_id(c, project_id, &id)?);
            }
            Ok(rows)
        })
        .await?;
    for s in &stored {
        difs.add(project_id, s.debug_id);
    }
    Ok(rows)
}

/// `POST …/files/difs/assemble/`.
async fn assemble(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    method: Method,
    UrlPath((_org, project_segment)): UrlPath<(String, String)>,
    body: Bytes,
) -> Response {
    if let Some(res) = check(&core, &user, method).await {
        return res;
    }
    let Some(project) = project(&core, &project_segment).await else {
        return detail(StatusCode::NOT_FOUND, "no such project");
    };
    let request: BTreeMap<String, AssembleItem> = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return detail(StatusCode::BAD_REQUEST, format!("invalid request: {e}")),
    };
    if request.len() > files::MAX_ASSEMBLE_FILES {
        return detail(StatusCode::BAD_REQUEST, "too many files in one request");
    }
    let difs = ingest.difs();
    let mut steps = Vec::with_capacity(request.len());
    for (checksum, item) in request {
        let s = step(&core, difs, project.id, &checksum, item).await;
        steps.push((checksum, s));
    }
    let deadline = tokio::time::Instant::now() + ASSEMBLE_WAIT;
    let mut out = serde_json::Map::new();
    for (checksum, s) in steps {
        let value = match s {
            Step::Done(v) => v,
            Step::Wait(mut rx) => {
                let done = tokio::time::timeout_at(deadline, rx.wait_for(Option::is_some))
                    .await
                    .ok()
                    .and_then(|r| r.ok().and_then(|v| v.clone()));
                match done {
                    Some(r) => finished(&core, project.id, &r).await,
                    None => state("assembling", &[], None, None),
                }
            }
        };
        out.insert(checksum, value);
    }
    axum::Json(Value::Object(out)).into_response()
}

// ---------------------------------------------------------------------------
// Whole files: the legacy endpoint and the REST API's zip
// ---------------------------------------------------------------------------

/// Bytes of a stream written to `path`, at most `max`.
async fn write_stream<S, E>(mut stream: S, path: &Path, max: u64) -> Result<u64, DifError>
where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::File::create(path).await?;
    let mut n = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| DifError::Invalid(format!("the upload broke off: {e}")))?;
        n += chunk.len() as u64;
        if n > max {
            drop(file);
            let _ = tokio::fs::remove_file(path).await;
            return Err(DifError::TooLarge("the upload"));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(n)
}

fn temp_name(difs: &Difs, what: &str) -> PathBuf {
    let mut r = [0u8; 8];
    rand::fill(&mut r);
    difs.tmp_dir().join(format!("{what}-{}", hex::encode(r)))
}

/// What a whole-file upload stored, and the files in it that weren't debug
/// files.
#[derive(Debug, Default)]
pub struct Outcome {
    pub stored: Vec<DifFile>,
    pub skipped: Vec<String>,
}

/// Stores the debug files in an uploaded zip, or the uploaded object file
/// itself (`name` names it). Removes `upload`.
pub async fn store_upload(
    core: &Core,
    difs: &Difs,
    project_id: i64,
    upload: PathBuf,
    name: &str,
) -> Result<Outcome, DifError> {
    let _turn = difs.processing().await;
    let existing = existing(core, project_id).await;
    let (d, name) = (difs.clone(), name.to_string());
    let work = tokio::task::spawn_blocking(move || {
        let result = unpack_and_store(&d, project_id, &upload, &name, &existing);
        let _ = std::fs::remove_file(&upload);
        result
    })
    .await
    .map_err(|_| DifError::Invalid("processing the upload failed".into()))??;
    let mut outcome = Outcome {
        skipped: work.skipped,
        ..Outcome::default()
    };
    for (name, sha1, stored) in work.files {
        let rows = save(core, difs, project_id, &sha1, &name, stored)
            .await
            .map_err(|e| DifError::Invalid(e.to_string()))?;
        outcome.stored.extend(rows);
    }
    if outcome.stored.is_empty() {
        return Err(DifError::NoDebugInfo);
    }
    tracing::info!(
        project = project_id,
        objects = outcome.stored.len(),
        "debug files stored"
    );
    Ok(outcome)
}

struct Unpacked {
    /// `(object name, SHA-1 of the file, stored objects)`.
    files: Vec<(String, String, Vec<Stored>)>,
    skipped: Vec<String>,
}

fn sha1_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut sha = sha1_smol::Sha1::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
    }
    Ok(sha.digest().to_string())
}

fn unpack_and_store(
    difs: &Difs,
    project_id: i64,
    upload: &Path,
    name: &str,
    existing: &HashMap<DebugId, bool>,
) -> Result<Unpacked, DifError> {
    let mut head = [0u8; 4];
    let is_zip = {
        use std::io::Read as _;
        let mut f = std::fs::File::open(upload)?;
        f.read(&mut head)? == 4 && head == *b"PK\x03\x04"
    };
    let project_dir = difs.project_dir(project_id);
    let mut out = Unpacked {
        files: Vec::new(),
        skipped: Vec::new(),
    };
    if !is_zip {
        let stored = files::store_objects(upload, &project_dir, existing)?;
        out.files
            .push((files::object_name(name), sha1_file(upload)?, stored));
        return Ok(out);
    }
    let dir = upload.with_extension("unpacked");
    let result = (|| {
        let (entries, skipped) = files::unpack_zip(upload, &dir)?;
        out.skipped.extend(skipped);
        for entry in entries {
            match files::store_objects(&entry.path, &project_dir, existing) {
                Ok(stored) => out.files.push((
                    files::object_name(&entry.name),
                    sha1_file(&entry.path)?,
                    stored,
                )),
                Err(DifError::NotAnObject | DifError::NoDebugInfo) => out.skipped.push(entry.name),
                Err(e) => return Err(e),
            }
            let _ = std::fs::remove_file(&entry.path);
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result.map(|()| out)
}

fn is_multipart(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("multipart/")
        })
}

/// The upload of a request (its `file` field when it is multipart, else
/// its body) in a temporary file, with the name it came with.
async fn receive(
    difs: &Difs,
    headers: &HeaderMap,
    body: Body,
) -> Result<(PathBuf, String), DifError> {
    let path = temp_name(difs, "upload");
    if !is_multipart(headers) {
        write_stream(Box::pin(body.into_data_stream()), &path, files::MAX_ZIP).await?;
        return Ok((path, "upload".to_string()));
    }
    let boundary = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| multer::parse_boundary(v).ok())
        .ok_or_else(|| DifError::Invalid("a multipart body without a boundary".into()))?;
    let mut form = multer::Multipart::new(body.into_data_stream(), boundary);
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| DifError::Invalid(format!("a broken multipart body: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("upload").to_string();
        write_stream(Box::pin(field), &path, files::MAX_ZIP).await?;
        return Ok((path, name));
    }
    Err(DifError::Invalid("the multipart body has no `file`".into()))
}

fn upload_status(e: &DifError) -> StatusCode {
    match e {
        DifError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        DifError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// `POST …/files/dsyms/` (sentry-cli before chunked uploads): a zip of
/// dSYMs, or one object file, as multipart `file`. Answers the stored
/// debug files.
async fn legacy_upload(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    method: Method,
    UrlPath((_org, project_segment)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(res) = check(&core, &user, method).await {
        return res;
    }
    let Some(project) = project(&core, &project_segment).await else {
        return detail(StatusCode::NOT_FOUND, "no such project");
    };
    let difs = ingest.difs();
    let (path, name) = match receive(difs, &headers, body).await {
        Ok(r) => r,
        Err(e) => return detail(upload_status(&e), e),
    };
    match store_upload(&core, difs, project.id, path, &name).await {
        Ok(o) => (
            StatusCode::CREATED,
            axum::Json(Value::Array(o.stored.iter().map(dif_json).collect())),
        )
            .into_response(),
        Err(e) => detail(upload_status(&e), e),
    }
}

/// `GET …/files/dsyms/`: the project's debug files.
async fn legacy_list(
    State(core): State<Core>,
    user: CurrentUser,
    method: Method,
    UrlPath((_org, project_segment)): UrlPath<(String, String)>,
) -> Response {
    if let Some(res) = check(&core, &user, method).await {
        return res;
    }
    let Some(project) = project(&core, &project_segment).await else {
        return detail(StatusCode::NOT_FOUND, "no such project");
    };
    let rows = core
        .with_conn(move |c| db::list(c, project.id))
        .await
        .unwrap_or_default();
    axum::Json(Value::Array(rows.iter().map(dif_json).collect())).into_response()
}

// ---------------------------------------------------------------------------
// REST API v1
// ---------------------------------------------------------------------------

/// A debug file in the REST API.
pub fn v1_json(d: &DifFile) -> Value {
    json!({
        "id": d.id,
        "project_id": d.project_id,
        "debug_id": d.debug_id,
        "code_id": d.code_id,
        "object_name": d.object_name,
        "arch": d.arch,
        "kind": d.kind,
        "features": d.features,
        "size": d.size,
        "sha1": d.sha1,
        "release": d.release,
        "uploaded_at": d.uploaded_at,
        "last_used_at": d.last_used_at,
    })
}

async fn v1_project(core: &Core, id: i64) -> V1Result<Project> {
    core.with_conn(move |c| projects::get(c, id))
        .await?
        .ok_or_else(V1Error::not_found)
}

/// `POST /api/v1/projects/{id}/dsyms`: a zip of dSYMs (or one object file)
/// as the body or as multipart `file`.
pub async fn v1_upload(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    method: Method,
    UrlPath(id): UrlPath<i64>,
    headers: HeaderMap,
    body: Body,
) -> V1Result<Response> {
    if let Err(d) = may_upload(&core, &user, method.map(|Extension(m)| m), true).await {
        return Err(match d {
            Denied::NeedsToken => {
                V1Error::new(StatusCode::UNAUTHORIZED, "unauthorized", d.message())
            }
            Denied::NotAllowed => V1Error::forbidden_msg(d.message()),
        });
    }
    let project = v1_project(&core, id).await?;
    let difs = ingest.difs();
    let v1_err = |e: DifError| {
        let status = upload_status(&e);
        let code = match status {
            StatusCode::PAYLOAD_TOO_LARGE => "too_large",
            StatusCode::INTERNAL_SERVER_ERROR => "internal",
            _ => "invalid_upload",
        };
        V1Error::new(status, code, e.to_string())
    };
    let (path, name) = receive(difs, &headers, body).await.map_err(v1_err)?;
    let outcome = store_upload(&core, difs, project.id, path, &name)
        .await
        .map_err(v1_err)?;
    Ok((
        StatusCode::CREATED,
        axum::Json(json!({
            "data": outcome.stored.iter().map(v1_json).collect::<Vec<_>>(),
            "skipped": outcome.skipped,
        })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `GET /api/v1/projects/{id}/debug-files`.
pub async fn v1_list(
    State(core): State<Core>,
    _user: CurrentUser,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
    vole_kit::http::rest::Params(q): vole_kit::http::rest::Params<ListQuery>,
) -> V1Result<Response> {
    v1_project(&core, id).await?;
    let cursor = parse_cursor(q.cursor.as_deref())?;
    let mut rows = core.with_conn(move |c| db::list(c, id)).await?;
    rows.sort_by_key(|r| r.id);
    let data: Vec<Value> = rows.iter().map(v1_json).collect();
    let page = paginate(
        data,
        |v| v["id"].as_i64().unwrap_or_default(),
        cursor,
        limit(q.limit),
    );
    Ok(json_ok(&headers, &page))
}

/// `DELETE /api/v1/projects/{id}/debug-files/{dif_id}`.
pub async fn v1_delete(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    UrlPath((id, dif_id)): UrlPath<(i64, i64)>,
) -> V1Result<Response> {
    user.require_editor().map_err(V1Error::from)?;
    if !delete(&core, ingest.difs(), id, dif_id).await? {
        return Err(V1Error::not_found());
    }
    Ok(no_content())
}

/// Deletes one debug file (row and files). `false` when there was none.
pub async fn delete(
    core: &Core,
    difs: &Difs,
    project_id: i64,
    dif_id: i64,
) -> Result<bool, vole_kit::http::ApiError> {
    let row = core
        .with_conn(move |c| {
            let row = db::get(c, project_id, dif_id)?;
            if row.is_some() {
                db::delete(c, project_id, dif_id)?;
            }
            Ok(row)
        })
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    if let Some(id) = super::parse_debug_id(&row.debug_id) {
        difs.remove(project_id, &id);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentry_cli_paths() {
        assert!(is_sentry_cli_path("/api/0/"));
        assert!(is_sentry_cli_path("/api/0/organizations/x/chunk-upload/"));
        assert!(!is_sentry_cli_path("/api/01/envelope/"));
        assert!(!is_sentry_cli_path("/api/v1/projects"));
    }
}
