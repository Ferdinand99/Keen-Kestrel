//! Debug files and symbolication (docs/design.md §4.8, §9): sentry-cli's
//! chunk-upload protocol, the legacy and zip endpoints, auth and limits, the
//! Debug files page, retention, and the real sentry-cocoa crashes of
//! `tests/fixtures/sentry/sentry-cocoa/9.30.2/` symbolicated with the dSYM
//! of the same build (`tests/fixtures/dsym/KestrelSpike.app.dSYM.zip`, from
//! the kestrel-spike iOS test app: stripped Release build, arm64 and x86_64).

mod common;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Request, StatusCode, header};
use common::{App, Res};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use vole_kit::auth::{TokenScope, tokens};

const PASSWORD: &str = "Kestrel-Owner-Pass-2026!";
const HOST: &str = "kestrel.test:3002";
/// The app's arm64 slice: the simulator build the crashes came from.
const APP_DEBUG_ID: &str = "74b62df4-771f-3338-8a76-efff43e57038";
const X86_DEBUG_ID: &str = "9edde443-7191-3405-95ab-d142975169de";

fn dsym_zip() -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/dsym/KestrelSpike.app.dSYM.zip"),
    )
    .unwrap()
}

/// The DWARF file inside the dSYM (fat: arm64 and x86_64).
fn dwarf() -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(dsym_zip())).unwrap();
    let mut f = zip
        .by_name("KestrelSpike.app.dSYM/Contents/Resources/DWARF/KestrelSpike")
        .unwrap();
    let mut out = Vec::new();
    f.read_to_end(&mut out).unwrap();
    out
}

fn sha1(data: &[u8]) -> String {
    sha1_smol::Sha1::from(data).digest().to_string()
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Adds a user with `role`; its id.
async fn user(app: &App, name: &'static str, role: &'static str) -> i64 {
    let hash = vole_kit::auth::hash_password(PASSWORD);
    app.kestrel
        .core
        .with_conn(move |c| {
            c.execute(
                "INSERT INTO users (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params![name, hash, role],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
        .unwrap()
}

/// A token of `user_id` with `scope`, and the `difs:write` scope if asked.
async fn token(app: &App, user_id: i64, scope: TokenScope, upload: bool) -> String {
    let token = tokens::generate();
    let (hash, prefix) = (tokens::hash(&token), tokens::display_prefix(&token));
    app.kestrel
        .core
        .with_conn(move |c| {
            let id =
                vole_kit::db::api_tokens::insert(c, user_id, "t", &prefix, &hash, scope, None)?;
            if upload {
                vole_kit::db::api_tokens::add_scope(c, id, keen_kestrel::product::DIFS_WRITE)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    token
}

/// An editor's upload token (the Debug files page's kind).
async fn upload_token(app: &App) -> String {
    let id = user(app, "ed", "editor").await;
    token(app, id, TokenScope::Read, true).await
}

async fn bearer(app: &App, method: &str, path: &str, token: &str, body: Body, ct: &str) -> Res {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if !ct.is_empty() {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    app.send(req.body(body).unwrap()).await
}

/// A multipart body with `files` as `(field, file name, data)`.
fn multipart(files: &[(&str, &str, Vec<u8>)]) -> (String, Vec<u8>) {
    let boundary = "kestrel-test-boundary-7f3a";
    let mut body = Vec::new();
    for (field, name, data) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload_zip(app: &App, token: &str) -> Res {
    bearer(
        app,
        "POST",
        &format!("/api/v1/projects/{}/dsyms", app.project_id),
        token,
        Body::from(dsym_zip()),
        "application/zip",
    )
    .await
}

fn cocoa(name: &str) -> common::Fixture {
    common::fixtures()
        .into_iter()
        .find(|f| f.sdk == "sentry-cocoa" && f.name == name)
        .unwrap()
}

/// The stored payload of `event_id`.
async fn payload(app: &App, event_id: &str) -> Value {
    let id = event_id.to_string();
    let conn = app.kestrel.ingest.store().get().await.unwrap();
    let data: Vec<u8> = conn
        .interact(move |c| {
            c.query_row("SELECT data FROM payloads WHERE event_id = ?1", [id], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap()
        .unwrap();
    serde_json::from_slice(&keen_kestrel::store::decompress(&data, 64 << 20).unwrap()).unwrap()
}

/// The app's frames of the event's first exception (caller first):
/// `(function, filename, lineno, inlined)`.
fn app_frames(event: &Value) -> Vec<(String, String, i64, bool)> {
    event["exception"]["values"][0]["stacktrace"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            f["package"]
                .as_str()
                .is_some_and(|p| p.ends_with("/KestrelSpike"))
        })
        .map(|f| {
            (
                f["function"].as_str().unwrap_or("").to_string(),
                f["filename"].as_str().unwrap_or("").to_string(),
                f["lineno"].as_i64().unwrap_or(0),
                f["inlined"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

/// `(filename, lineno, inlined)` of a frame.
fn at(f: &(String, String, i64, bool)) -> (String, i64, bool) {
    (f.1.clone(), f.2, f.3)
}

fn find<'a>(
    frames: &'a [(String, String, i64, bool)],
    function: &str,
) -> &'a (String, String, i64, bool) {
    frames
        .iter()
        .find(|f| f.0.contains(function))
        .unwrap_or_else(|| panic!("no frame {function} in {frames:#?}"))
}

#[tokio::test]
async fn the_zip_upload_stores_each_architecture_with_a_symcache() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    let res = upload_zip(&app, &token).await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let body = res.json();
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["debug_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "{body}");
    assert!(ids.contains(&APP_DEBUG_ID) && ids.contains(&X86_DEBUG_ID));
    let arm = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["debug_id"] == APP_DEBUG_ID)
        .unwrap();
    assert_eq!(arm["arch"], "arm64");
    assert_eq!(arm["kind"], "macho");
    assert_eq!(arm["object_name"], "KestrelSpike");
    assert!(
        arm["features"]
            .as_array()
            .unwrap()
            .contains(&json!("debug"))
    );
    // Info.plist, the Swift interfaces and the relocation maps aren't debug files.
    let skipped = body["skipped"].as_array().unwrap();
    assert!(
        skipped
            .iter()
            .any(|s| s.as_str().unwrap().ends_with("Info.plist")),
        "{skipped:?}"
    );
    for id in [APP_DEBUG_ID, X86_DEBUG_ID] {
        let dir = app
            .dir
            .join("difs")
            .join(app.project_id.to_string())
            .join(id);
        assert!(dir.join("object").is_file() && dir.join("symcache").is_file());
    }
    assert!(app.kestrel.ingest.difs().has(
        app.project_id,
        &keen_kestrel::difs::parse_debug_id(APP_DEBUG_ID).unwrap()
    ));
    // Listed in the API, newest first on the page.
    let list = bearer(
        &app,
        "GET",
        &format!("/api/v1/projects/{}/debug-files", app.project_id),
        &token,
        Body::empty(),
        "",
    )
    .await;
    assert_eq!(list.json()["data"].as_array().unwrap().len(), 2);
    // Garbage and a zip without debug files are refused.
    let res = bearer(
        &app,
        "POST",
        &format!("/api/v1/projects/{}/dsyms", app.project_id),
        &token,
        Body::from("not a zip"),
        "application/zip",
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    let mut empty = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    empty
        .start_file(
            "App.dSYM/Contents/Info.plist",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    empty.write_all(b"<plist/>").unwrap();
    let empty = empty.finish().unwrap().into_inner();
    let res = bearer(
        &app,
        "POST",
        &format!("/api/v1/projects/{}/dsyms", app.project_id),
        &token,
        Body::from(empty),
        "application/zip",
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    // Multipart works too, and the same build again replaces the rows.
    let (ct, body) = multipart(&[("file", "KestrelSpike.app.dSYM.zip", dsym_zip())]);
    let res = bearer(
        &app,
        "POST",
        &format!("/api/v1/projects/{}/dsyms", app.project_id),
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    assert_eq!(app.count("SELECT COUNT(*) FROM dif_files").await, 2);
}

#[tokio::test]
async fn a_zip_bomb_is_refused() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    // 40 MB that packs to almost nothing, behind a Mach-O header.
    let mut data = vec![0u8; 40 << 20];
    data[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    data[4..8].copy_from_slice(&[0x0c, 0x00, 0x00, 0x01]);
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file(
        "Bomb.dSYM/Contents/Resources/DWARF/Bomb",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9)),
    )
    .unwrap();
    w.write_all(&data).unwrap();
    let zipped = w.finish().unwrap().into_inner();
    assert!(zipped.len() < 200_000, "{}", zipped.len());
    let res = bearer(
        &app,
        "POST",
        &format!("/api/v1/projects/{}/dsyms", app.project_id),
        &token,
        Body::from(zipped),
        "application/zip",
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    assert!(res.body.contains("zip bomb"), "{}", res.body);
    assert_eq!(app.count("SELECT COUNT(*) FROM dif_files").await, 0);
    // Nothing left behind in tmp/.
    let tmp: Vec<_> = std::fs::read_dir(app.dir.join("difs/tmp"))
        .unwrap()
        .collect();
    assert!(tmp.is_empty(), "{tmp:?}");
}

#[tokio::test]
async fn sentry_clis_chunk_upload_and_assemble() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    let slug_path = "/api/0/projects/any-org/fixtures/files/difs/assemble/";

    // Who am I, and the chunk-upload options.
    let me = bearer(&app, "GET", "/api/0/", &token, Body::empty(), "").await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert!(
        me.json()["auth"]["scopes"]
            .as_array()
            .unwrap()
            .contains(&json!("project:releases"))
    );
    let opts = bearer(
        &app,
        "GET",
        "/api/0/organizations/any-org/chunk-upload/",
        &token,
        Body::empty(),
        "",
    )
    .await;
    assert_eq!(opts.status, StatusCode::OK, "{}", opts.body);
    let opts = opts.json();
    assert_eq!(opts["hashAlgorithm"], "sha1");
    assert_eq!(opts["chunkSize"], 8 * 1024 * 1024);
    assert_eq!(opts["url"], "/api/0/organizations/any-org/chunk-upload/");
    assert!(
        opts["accept"]
            .as_array()
            .unwrap()
            .contains(&json!("debug_files"))
    );
    assert!(
        opts["compression"]
            .as_array()
            .unwrap()
            .contains(&json!("gzip"))
    );

    // The DWARF file in two chunks.
    let file = dwarf();
    let (a, b) = file.split_at(file.len() / 2);
    let (ca, cb, cf) = (sha1(a), sha1(b), sha1(&file));
    let request = json!({ cf.clone(): {"name": "KestrelSpike", "debug_id": APP_DEBUG_ID, "chunks": [ca, cb]} });
    let assemble = |body: Value| {
        let t = token.clone();
        let app = &app;
        async move {
            bearer(
                app,
                "POST",
                slug_path,
                &t,
                Body::from(body.to_string()),
                "application/json",
            )
            .await
        }
    };
    // Nothing uploaded yet: both chunks are missing.
    let res = assemble(request.clone()).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.json()[&cf]["state"], "not_found");
    assert_eq!(res.json()[&cf]["missingChunks"], json!([ca, cb]));

    // A chunk whose name isn't its SHA-1, and one over 8 MB, are refused.
    let (ct, body) = multipart(&[("file", &ca, b.to_vec())]);
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any-org/chunk-upload/",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    let big = vec![7u8; 9 << 20];
    let (ct, body) = multipart(&[("file", &sha1(&big), big.clone())]);
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any-org/chunk-upload/",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
    assert!(res.body.contains("too large"), "{}", res.body);

    // One chunk plain, then: only the other is missing.
    let (ct, body) = multipart(&[("file", &ca, a.to_vec())]);
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any-org/chunk-upload/",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let res = assemble(request.clone()).await;
    assert_eq!(res.json()[&cf]["state"], "not_found");
    assert_eq!(res.json()[&cf]["missingChunks"], json!([cb]));

    // The other gzip-compressed (as sentry-cli sends them); then it assembles.
    let (ct, body) = multipart(&[("file_gzip", &cb, gzip(b))]);
    let res = bearer(
        &app,
        "POST",
        "/api/0/organizations/any-org/chunk-upload",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let res = assemble(request.clone()).await;
    let state = &res.json()[&cf];
    assert_eq!(state["state"], "ok", "{state}");
    assert_eq!(state["missingChunks"], json!([]));
    let dif = &state["dif"];
    assert_eq!(dif["objectName"], "KestrelSpike");
    assert_eq!(dif["sha1"], cf);
    assert!(
        dif["debugId"] == APP_DEBUG_ID || dif["debugId"] == X86_DEBUG_ID,
        "{dif}"
    );
    assert!(
        dif["data"]["features"]
            .as_array()
            .unwrap()
            .contains(&json!("debug"))
    );
    // The chunks are gone; asking again answers from what is stored.
    assert_eq!(
        std::fs::read_dir(app.dir.join("difs/chunks"))
            .unwrap()
            .count(),
        0
    );
    let res = assemble(request.clone()).await;
    assert_eq!(res.json()[&cf]["state"], "ok");
    assert_eq!(app.count("SELECT COUNT(*) FROM dif_files").await, 2);

    // A file whose chunks don't add up to its checksum fails; bad names too.
    let x = b"neither here nor there".to_vec();
    let (ct, body) = multipart(&[("file", &sha1(&x), x.clone())]);
    bearer(
        &app,
        "POST",
        "/api/0/organizations/o/chunk-upload/",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    let wrong = sha1(b"something else");
    let res = assemble(json!({ wrong.clone(): {"name": "x", "chunks": [sha1(&x)]}, "nope": {"name": "y", "chunks": []} })).await;
    assert_eq!(res.json()[&wrong]["state"], "error", "{}", res.body);
    assert!(
        res.json()[&wrong]["detail"]
            .as_str()
            .unwrap()
            .contains("SHA-1")
    );
    assert_eq!(res.json()["nope"]["state"], "error");
    // Not an object file.
    let res = assemble(json!({ sha1(&x): {"name": "x", "chunks": [sha1(&x)]} })).await;
    assert_eq!(res.json()[&sha1(&x)]["state"], "error", "{}", res.body);

    // The project by its numeric id works; an unknown one doesn't.
    let by_id = format!("/api/0/projects/o/{}/files/difs/assemble/", app.project_id);
    let res = bearer(
        &app,
        "POST",
        &by_id,
        &token,
        Body::from(request.to_string()),
        "application/json",
    )
    .await;
    assert_eq!(res.json()[&cf]["state"], "ok");
    let res = bearer(
        &app,
        "POST",
        "/api/0/projects/o/nope/files/difs/assemble/",
        &token,
        Body::from(request.to_string()),
        "application/json",
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_legacy_dsym_endpoint_takes_a_zip() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    let (ct, body) = multipart(&[("file", "dsyms.zip", dsym_zip())]);
    let res = bearer(
        &app,
        "POST",
        "/api/0/projects/acme/fixtures/files/dsyms/",
        &token,
        Body::from(body),
        &ct,
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
    let difs = res.json();
    assert_eq!(difs.as_array().unwrap().len(), 2);
    assert!(
        difs.as_array()
            .unwrap()
            .iter()
            .any(|d| d["uuid"] == APP_DEBUG_ID && d["cpuName"] == "arm64")
    );
    let list = bearer(
        &app,
        "GET",
        "/api/0/projects/acme/fixtures/files/dsyms/",
        &token,
        Body::empty(),
        "",
    )
    .await;
    assert_eq!(list.json().as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn only_tokens_that_may_upload_debug_files_can() {
    let app = App::new().await;
    let editor = user(&app, "eve", "editor").await;
    let viewer = user(&app, "vic", "viewer").await;
    let upload = token(&app, editor, TokenScope::Read, true).await;
    let write = token(&app, editor, TokenScope::Write, false).await;
    let read = token(&app, editor, TokenScope::Read, false).await;
    let viewer_write = token(&app, viewer, TokenScope::Write, false).await;
    let viewer_upload = token(&app, viewer, TokenScope::Read, true).await;
    let options = "/api/0/organizations/x/chunk-upload/";
    for (t, status) in [
        (&upload, StatusCode::OK),
        (&write, StatusCode::OK),
        (&read, StatusCode::FORBIDDEN),
        (&viewer_write, StatusCode::FORBIDDEN),
        (&viewer_upload, StatusCode::FORBIDDEN),
    ] {
        let res = bearer(&app, "GET", options, t, Body::empty(), "").await;
        assert_eq!(res.status, status, "{}", res.body);
    }
    // No token: 401; a token that isn't one: 401.
    let res = app
        .send(Request::get(options).body(Body::empty()).unwrap())
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let res = bearer(&app, "GET", options, "kk_nope", Body::empty(), "").await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    // The scope doesn't make the token write anything else.
    let res = bearer(
        &app,
        "POST",
        "/api/v1/projects",
        &upload,
        Body::from(r#"{"name":"X"}"#),
        "application/json",
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    let res = bearer(&app, "GET", "/api/v1/projects", &upload, Body::empty(), "").await;
    assert_eq!(res.status, StatusCode::OK);
    // A demoted owner's upload token stops working.
    app.exec(format!(
        "UPDATE users SET role = 'viewer' WHERE id = {editor}"
    ))
    .await;
    let res = bearer(&app, "GET", options, &upload, Body::empty(), "").await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    // The zip endpoint checks the same.
    let res = upload_zip(&app, &read).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
}

/// A database as Kestrel 0.2.0 (on vole-kit 0.1) left it, with an upload
/// token granted in its own `token_grants`: after the upgrade the grant is
/// the kit's `difs:write` token scope and the token still uploads.
#[tokio::test]
async fn upload_tokens_keep_working_after_the_upgrade() {
    let app = App::new().await;
    let editor = user(&app, "uma", "editor").await;
    let upload = token(&app, editor, TokenScope::Read, false).await;
    let plain = token(&app, editor, TokenScope::Read, false).await;
    // Back to 0.2.0's schema: no api_token_scopes (kit/0002_token_scopes.sql)
    // and no 0008, but 0004's token_grants with the first token's grant.
    app.exec(format!(
        "DROP TABLE api_token_scopes;
         DELETE FROM schema_migrations
             WHERE name IN ('kit/0002_token_scopes.sql', '0008_token_scopes.sql');
         CREATE TABLE token_grants (
             token_id INTEGER NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
             scope TEXT NOT NULL CHECK (scope IN ('difs:write')),
             PRIMARY KEY (token_id, scope)
         ) WITHOUT ROWID;
         INSERT INTO token_grants (token_id, scope)
             SELECT MIN(id), 'difs:write' FROM api_tokens WHERE user_id = {editor};"
    ))
    .await;

    // Kestrel starts again on the same data: the kit's migration, then
    // Kestrel's, after a copy of the database.
    let upgraded = keen_kestrel::start(&app.dir, None).await.unwrap();
    let (old_table, scopes) = upgraded
        .core
        .with_conn(move |c| {
            let old: i64 = c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'token_grants'",
                [],
                |r| r.get(0),
            )?;
            let scopes: Vec<(String, String)> = c
                .prepare(
                    "SELECT t.name, s.scope FROM api_token_scopes s
                     JOIN api_tokens t ON t.id = s.token_id ORDER BY t.id",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok((old, scopes))
        })
        .await
        .unwrap();
    assert_eq!(old_table, 0, "token_grants is gone");
    assert_eq!(scopes, [("t".to_string(), "difs:write".to_string())]);
    let backups: Vec<String> = std::fs::read_dir(app.dir.join("backups"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups
            .iter()
            .any(|b| b.starts_with("kestrel-before-kit-0002_token_scopes")),
        "{backups:?}"
    );

    let router = keen_kestrel::router(upgraded);
    let get = |token: &str| {
        Request::get("/api/0/organizations/x/chunk-upload/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let res = common::send_to(&router, &mut get(&upload)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let res = common::send_to(&router, &mut get(&plain)).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    let mut zip = Request::post(format!("/api/v1/projects/{}/dsyms", app.project_id))
        .header(header::AUTHORIZATION, format!("Bearer {upload}"))
        .header(header::CONTENT_TYPE, "application/zip")
        .body(Body::from(dsym_zip()))
        .unwrap();
    let res = common::send_to(&router, &mut zip).await;
    assert_eq!(res.status, StatusCode::CREATED, "{}", res.body);
}

/// The kit's API tokens page offers the upload scope and shows it as the
/// token's access; such a token uploads and otherwise only reads.
#[tokio::test]
async fn the_tokens_page_makes_upload_tokens() {
    let app = App::new().await;
    let mut ed = Browser::new(&app, "tess", "editor").await;
    let page = ed.get("/settings/tokens").await;
    assert!(page.body.contains(r#"value="difs:write""#), "{}", page.body);
    assert!(page.body.contains("Read and upload debug files"));
    assert!(
        page.body
            .contains("Reads everything and uploads debug files, but changes nothing else")
    );
    let res = ed
        .post(
            "/settings/tokens",
            &[
                ("name", "Build Mac"),
                ("scope", "difs:write"),
                ("expires", "30"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let token = res
        .body
        .split("id=\"created-token\">")
        .nth(1)
        .and_then(|s| s.split('<').next())
        .unwrap()
        .to_string();
    assert!(token.starts_with("kk_"), "{token}");
    assert_eq!(upload_zip(&app, &token).await.status, StatusCode::CREATED);
    let res = bearer(
        &app,
        "POST",
        "/api/v1/projects",
        &token,
        Body::from(r#"{"name":"X"}"#),
        "application/json",
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    // Listed with the scope's label, here and on the Debug files page.
    let page = ed.get("/settings/tokens").await;
    assert!(
        page.body
            .contains(r#"<span class="badge">Read and upload debug files</span>"#),
        "{}",
        page.body
    );
    let difs = ed
        .get(&format!("/projects/{}/debug-files", app.project_id))
        .await;
    assert!(difs.body.contains("Build Mac"), "{}", difs.body);
}

#[tokio::test]
async fn crashes_are_symbolicated_with_the_dsym() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    assert_eq!(upload_zip(&app, &token).await.status, StatusCode::CREATED);
    for name in [
        "crash-fatalerror",
        "crash-force-unwrap",
        "crash-nsexception",
        "handled-nserror",
    ] {
        let res = app.send_fixture(&cocoa(name)).await;
        assert_eq!(res.status, StatusCode::OK, "{name}: {}", res.body);
    }
    app.flush().await;
    assert_eq!(
        app.kestrel
            .ingest
            .symbolicator()
            .stats()
            .events
            .load(std::sync::atomic::Ordering::Relaxed),
        4
    );

    // fatalError(): the call chain from CrashSite.swift, with file, line and
    // the inlined trigger(_:depth:) (it only exists inlined).
    let fatal = payload(&app, "0ae194b18c7448da976fa84bca4edea7").await;
    let frames = app_frames(&fatal);
    // The dSYM's paths are the build machine's: stored without its home
    // directory or CI workspace, the file name kept.
    let abs: Vec<String> = fatal["exception"]["values"][0]["stacktrace"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["abs_path"].as_str().map(str::to_string))
        .collect();
    assert!(!abs.is_empty());
    for p in &abs {
        assert!(
            !p.starts_with("/Users/") && !p.starts_with("/home/") && !p.contains("runner"),
            "{p}"
        );
    }
    assert!(
        abs.iter().any(|p| p.ends_with("CrashSite.swift")),
        "{abs:?}"
    );
    assert_eq!(
        at(find(&frames, "CrashSite.crash(")),
        ("CrashSite.swift".into(), 59, false)
    );
    assert_eq!(
        at(find(&frames, "CrashPipeline.run(")),
        ("CrashSite.swift".into(), 51, false)
    );
    // detonate at line 43: the return address moved back into the call.
    assert_eq!(
        at(find(&frames, "Stage.detonate(")),
        ("CrashSite.swift".into(), 43, false)
    );
    let trigger = find(&frames, "trigger(");
    assert_eq!(trigger.0, "trigger(_: CrashKind, depth: Int)");
    assert_eq!(at(trigger), ("CrashSite.swift".into(), 21, true));
    // Innermost last: trigger comes right after detonate.
    let pos = |f: &str| frames.iter().position(|x| x.0.contains(f)).unwrap();
    assert_eq!(pos("trigger("), pos("Stage.detonate(") + 1);
    // Swift is demangled, and the raw symbol kept.
    assert!(
        frames
            .iter()
            .all(|f| !f.0.starts_with("$s") && !f.0.starts_with("_$s")),
        "{frames:#?}"
    );
    let raw = fatal["exception"]["values"][0]["stacktrace"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["function"]
                .as_str()
                .is_some_and(|n| n.contains("Stage.detonate("))
        })
        .unwrap()
        .clone();
    assert!(raw["symbol"].as_str().unwrap().starts_with("$s"), "{raw}");
    assert_eq!(raw["data"]["symbolicator_status"], "symbolicated");
    assert_eq!(raw["instruction_addr"], "0x0000000104c36048");
    // The SDK's frames of system images are as they were: no function, the package kept.
    let system: Vec<&Value> = fatal["exception"]["values"][0]["stacktrace"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            f["package"]
                .as_str()
                .is_some_and(|p| p.contains("SwiftUI.framework"))
        })
        .collect();
    assert!(!system.is_empty());
    assert!(
        system
            .iter()
            .all(|f| f.get("function").is_none() && f.get("data").is_none())
    );

    // The force-unwrap and the NSException: the same chain, their own lines.
    let unwrap = app_frames(&payload(&app, "c6fe5340c3f046168448cf0dc1086895").await);
    assert_eq!(
        at(find(&unwrap, "trigger(")),
        ("CrashSite.swift".into(), 24, true)
    );
    assert!(find(&unwrap, "Unexpectedly found nil").3, "{unwrap:#?}");
    let nsexc = app_frames(&payload(&app, "a88bf5e53f8b4737affa4dd13ccb88a8").await);
    assert_eq!(
        at(find(&nsexc, "trigger(")),
        ("CrashSite.swift".into(), 30, true)
    );
    assert_eq!(
        at(find(&nsexc, "Stage.detonate(")),
        ("CrashSite.swift".into(), 43, false)
    );

    // Grouped by the functions: not unsymbolicated, the culprit names the code.
    let (unsym, culprit, components): (bool, String, String) = app
        .kestrel
        .core
        .with_conn(|c| {
            c.query_row(
                "SELECT i.unsymbolicated, i.culprit, h.components FROM issues i
                 JOIN issue_hashes h ON h.issue_id = i.id
                 JOIN events e ON e.issue_id = i.id WHERE e.event_id = '0ae194b18c7448da976fa84bca4edea7'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .await
        .unwrap();
    assert!(!unsym);
    assert_eq!(
        culprit,
        "CrashSite.swift in trigger(_: CrashKind, depth: Int)"
    );
    let components: Value = serde_json::from_str(&components).unwrap();
    let parts: Vec<&Value> = components["values"][0]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert!(
        parts.iter().all(|p| p.get("image").is_none()),
        "{components}"
    );
    assert!(
        parts
            .iter()
            .any(|p| p["function"] == "CrashPipeline.Stage.detonate(CrashKind)"),
        "{components}"
    );
    assert!(
        parts.iter().any(|p| p["module"] == "CrashSite.swift"
            && p["function"] == "trigger(_: CrashKind, depth: Int)"),
        "{components}"
    );
    // Three crashes, three issues; the same crash again joins its issue.
    assert_eq!(
        app.count("SELECT COUNT(*) FROM issues WHERE level = 'fatal'")
            .await,
        3
    );
    let again = cocoa("crash-fatalerror");
    let mut req = again.request(app.project_id, &app.key);
    // A new event id, so it isn't a duplicate.
    let env = String::from_utf8(again.decoded()).unwrap().replace(
        "0ae194b18c7448da976fa84bca4edea7",
        "0ae194b18c7448da976fa84bca4edea8",
    );
    *req.body_mut() = Body::from(env);
    req.headers_mut().remove(header::CONTENT_ENCODING);
    assert_eq!(app.send(req).await.status, StatusCode::OK);
    app.flush().await;
    assert_eq!(
        app.count("SELECT COUNT(*) FROM issues WHERE level = 'fatal'")
            .await,
        3
    );
    assert_eq!(
        app.count("SELECT MAX(times_seen) FROM issues WHERE level = 'fatal'")
            .await,
        2
    );

    // The debug file was used, by this release.
    let (used, release): (Option<String>, Option<String>) = app
        .kestrel
        .core
        .with_conn(|c| {
            c.query_row(
                "SELECT last_used_at, release FROM dif_files WHERE debug_id = ?1",
                [APP_DEBUG_ID],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert!(used.is_some());
    assert_eq!(release.as_deref(), Some("com.example.kestrelspike@1.0+1"));
}

#[tokio::test]
async fn without_a_dsym_crashes_stay_unsymbolicated() {
    let app = App::new().await;
    app.send_fixture(&cocoa("crash-fatalerror")).await;
    app.flush().await;
    let event = payload(&app, "0ae194b18c7448da976fa84bca4edea7").await;
    assert!(app_frames(&event).iter().all(|f| f.0.is_empty()));
    let (unsym, culprit): (bool, String) = app
        .kestrel
        .core
        .with_conn(|c| {
            c.query_row("SELECT unsymbolicated, culprit FROM issues", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
        })
        .await
        .unwrap();
    assert!(unsym);
    assert_eq!(culprit, "KestrelSpike +0x2048");
    assert_eq!(
        app.kestrel
            .ingest
            .symbolicator()
            .stats()
            .events
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

/// A signed-in browser.
struct Browser<'a> {
    app: &'a App,
    cookies: BTreeMap<String, String>,
}

impl<'a> Browser<'a> {
    async fn new(app: &'a App, name: &'static str, role: &'static str) -> Browser<'a> {
        user(app, name, role).await;
        let mut b = Browser {
            app,
            cookies: BTreeMap::new(),
        };
        let res = b
            .post("/login", &[("username", name), ("password", PASSWORD)])
            .await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
        b
    }

    async fn send(&mut self, mut req: Request<Body>) -> Res {
        let h = req.headers_mut();
        h.insert(header::HOST, HeaderValue::from_static(HOST));
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            h.insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());
        }
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        let res = self.app.router.clone().oneshot(req).await.unwrap();
        for v in res.headers().get_all(header::SET_COOKIE) {
            let v = v.to_str().unwrap();
            if let Some((k, val)) = v.split(';').next().unwrap().split_once('=') {
                if val.is_empty() || v.contains("Max-Age=0") {
                    self.cookies.remove(k);
                } else {
                    self.cookies.insert(k.into(), val.into());
                }
            }
        }
        let (status, headers) = (res.status(), res.headers().clone());
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        Res {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    async fn get(&mut self, path: &str) -> Res {
        self.send(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Res {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let req = Request::post(path)
            .header(header::ORIGIN, format!("http://{HOST}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }
}

#[tokio::test]
async fn the_debug_files_page_lists_explains_and_deletes() {
    let app = App::new().await;
    let p = app.project_id;
    let mut ed = Browser::new(&app, "edda", "editor").await;
    let page = ed.get(&format!("/projects/{p}/debug-files")).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(page.body.contains("No debug files yet"));
    assert!(
        page.body
            .contains("sentry-cli --url http://kestrel.test:3002 debug-files upload")
    );
    assert!(page.body.contains("--org any --project fixtures"));
    assert!(page.body.contains("DWARF_DSYM_FOLDER_PATH"));
    assert!(page.body.contains(&format!("/api/v1/projects/{p}/dsyms")));
    assert!(page.body.contains("applies to events that arrive after it"));
    // An upload token, shown once, that works with sentry-cli's endpoints.
    let res = ed
        .post(
            &format!("/projects/{p}/debug-files/tokens"),
            &[("name", "CI")],
        )
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let token = res
        .body
        .split("id=\"created-token\">")
        .nth(1)
        .and_then(|s| s.split('<').next())
        .unwrap()
        .to_string();
    assert!(token.starts_with("kk_"), "{token}");
    assert_eq!(upload_zip(&app, &token).await.status, StatusCode::CREATED);
    let page = ed.get(&format!("/projects/{p}/debug-files")).await;
    assert!(page.body.contains(APP_DEBUG_ID) && page.body.contains("arm64"));
    assert!(page.body.contains("Debug info"));
    assert!(page.body.contains("CI"));
    // Sessions can't use sentry-cli's endpoints.
    let res = ed.get("/api/0/organizations/x/chunk-upload/").await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
    // Viewers read but don't delete or make tokens.
    let mut viewer = Browser::new(&app, "vera", "viewer").await;
    let page = viewer.get(&format!("/projects/{p}/debug-files")).await;
    assert!(page.body.contains(APP_DEBUG_ID) && !page.body.contains("/delete\""));
    let id = app.count("SELECT MIN(id) FROM dif_files").await;
    let res = viewer
        .post(&format!("/projects/{p}/debug-files/{id}/delete"), &[])
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = viewer
        .post(
            &format!("/projects/{p}/debug-files/tokens"),
            &[("name", "x")],
        )
        .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    // An editor deletes: row, files and index entry go.
    let debug_id: String = app
        .kestrel
        .core
        .with_conn(move |c| {
            c.query_row("SELECT debug_id FROM dif_files WHERE id = ?1", [id], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    let res = ed
        .post(&format!("/projects/{p}/debug-files/{id}/delete"), &[])
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    assert_eq!(app.count("SELECT COUNT(*) FROM dif_files").await, 1);
    assert!(
        !app.dir
            .join("difs")
            .join(p.to_string())
            .join(&debug_id)
            .exists()
    );
    assert!(
        !app.kestrel
            .ingest
            .difs()
            .has(p, &keen_kestrel::difs::parse_debug_id(&debug_id).unwrap())
    );
}

#[tokio::test]
async fn the_issue_page_shows_symbolicated_and_missing_frames() {
    let app = App::new().await;
    let p = app.project_id;
    let mut ed = Browser::new(&app, "edda", "editor").await;
    // Without a dSYM: the missing hint with the app's debug id.
    app.send_fixture(&cocoa("crash-force-unwrap")).await;
    app.flush().await;
    let issue = app.count("SELECT id FROM issues").await;
    let page = ed.get(&format!("/projects/{p}/issues/{issue}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("dSYM missing"), "missing hint");
    assert!(page.body.contains(APP_DEBUG_ID));
    assert!(page.body.contains(&format!("/projects/{p}/debug-files")));
    // With it: function, file:line and the inlined badge.
    let token = upload_token(&app).await;
    upload_zip(&app, &token).await;
    app.send_fixture(&cocoa("crash-nsexception")).await;
    app.flush().await;
    let issue = app
        .count("SELECT id FROM issues WHERE title LIKE 'NSInternalInconsistencyException%'")
        .await;
    let page = ed.get(&format!("/projects/{p}/issues/{issue}")).await;
    assert!(
        page.body
            .contains("CrashPipeline.Stage.detonate(CrashKind)"),
        "{}",
        page.body
    );
    assert!(page.body.contains("CrashSite.swift:43"));
    assert!(page.body.contains("CrashSite.swift:30"));
    assert!(page.body.contains("Inlined"));
    assert!(!page.body.contains("dSYM missing"));
}

#[tokio::test]
async fn retention_removes_old_debug_files_but_keeps_recent_releases() {
    let app = App::new().await;
    let token = upload_token(&app).await;
    upload_zip(&app, &token).await;
    let p = app.project_id;
    // Both uploaded a year ago; the arm64 one used by the newest release.
    app.exec(format!(
        "UPDATE dif_files SET uploaded_at = datetime('now', '-365 days');
         INSERT INTO releases (project_id, version, first_seen, last_seen) VALUES ({p}, 'app@1', datetime('now'), datetime('now'));
         UPDATE dif_files SET release = 'app@1', last_used_at = datetime('now', '-300 days') WHERE debug_id = '{APP_DEBUG_ID}';"
    ))
    .await;
    // A chunk from an upload that never finished, two days old.
    let chunk = app.dir.join("difs/chunks").join(sha1(b"left over"));
    std::fs::write(&chunk, b"left over").unwrap();
    let f = std::fs::File::options().write(true).open(&chunk).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400))
        .unwrap();
    drop(f);
    let now = keen_kestrel::issues::now();
    let ingest = &app.kestrel.ingest;
    let n = keen_kestrel::retention::prune_debug_files(ingest.writer(), ingest.difs(), &now)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(app.count("SELECT COUNT(*) FROM dif_files").await, 1);
    let dir = app.dir.join("difs").join(p.to_string());
    assert!(dir.join(APP_DEBUG_ID).exists() && !dir.join(X86_DEBUG_ID).exists());
    assert!(!chunk.exists());
    // With no release kept, it goes too.
    app.exec(format!(
        "UPDATE projects SET retention_dif_releases = 0 WHERE id = {p}"
    ))
    .await;
    let n = keen_kestrel::retention::prune_debug_files(ingest.writer(), ingest.difs(), &now)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(!dir.join(APP_DEBUG_ID).exists());
    // Deleting the project removes its directory.
    upload_zip(&app, &token).await;
    let mut ed = Browser::new(&app, "edda", "editor").await;
    ed.post(&format!("/projects/{p}/delete"), &[]).await;
    assert!(!dir.exists());
}

#[tokio::test]
async fn the_retention_form_takes_debug_file_settings() {
    let app = App::new().await;
    let p = app.project_id;
    let mut ed = Browser::new(&app, "edda", "editor").await;
    let page = ed.get(&format!("/projects/{p}/settings")).await;
    assert!(page.body.contains(r#"name="dif_days""#) && page.body.contains(r#"value="180""#));
    let res = ed
        .post(
            &format!("/projects/{p}/retention"),
            &[
                ("event_days", "30"),
                ("issue_days", "90"),
                ("per_day", "1000"),
                ("dif_days", "60"),
                ("dif_releases", "2"),
            ],
        )
        .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER);
    let r = app
        .kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::triage::retention(c, p))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((r.dif_days, r.dif_releases), (60, 2));
    // A form without them keeps them.
    ed.post(
        &format!("/projects/{p}/retention"),
        &[
            ("event_days", "7"),
            ("issue_days", "90"),
            ("per_day", "1000"),
        ],
    )
    .await;
    let r = app
        .kestrel
        .core
        .with_conn(move |c| keen_kestrel::db::triage::retention(c, p))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((r.event_days, r.dif_days, r.dif_releases), (7, 60, 2));
}
