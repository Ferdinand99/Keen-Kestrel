# Development

Keen Kestrel is one Rust crate (`keen-kestrel`, binary `kestrel`) on top of
[vole-kit](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/docs/vole-kit.md), the shared kit that lives in the
Vigilant Vole repository. The kit brings sign-in, users, API tokens, Security, the API reference, i18n, the
database plumbing and the design system; Kestrel adds its own pages, API, migrations and strings, the
Sentry-compatible ingest ([below](#ingest)) and debug files with symbolication
([below](#debug-files-and-symbolication)).

## Layout

```
Cargo.toml            # keen-kestrel; vole-kit by git tag
build.rs              # copies the kit's templates into templates/kit/ (git-ignored)
rust-toolchain.toml   # Rust 1.99
src/
  main.rs             # `kestrel`: flags/env, logging, server
  lib.rs              # start() (config, database, state) and router()
  product.rs          # KitConfig: names, cookies, token prefix, nav, locales, static files
  config.rs           # flags and KK_* environment variables
  server.rs           # listener and graceful shutdown
  project.rs          # project and key rules (names, slugs, origins, rate limits)
  db/                 # migration set, projects and project_keys, issues (the writer's upsert),
                      # triage (the issue pages' reads, lifecycle actions, retention deletes)
  dsn.rs              # public address, DSNs, SDK snippets
  ingest/             # Sentry endpoints: keys, body decoding and limits, envelopes, rate limits, capture mode
  event/              # normalisation and privacy (scrub.rs), grouping (kestrel:v1), title, culprit, tags
  difs/               # debug files: on disk (files.rs), sentry-cli upload (upload.rs), symbolication
                      # (symbolicate.rs), the symbolication queue (worker.rs), the Debug files page (pages.rs)
  writer.rs           # the single batch writer
  store.rs            # store.sqlite: event payloads, attachments, sessions, feedback
  pages.rs            # project pages: list, overview, settings (keys, retention)
  issues/             # issue pages: list (search.rs: the search syntax), issue, event (view.rs: the
                      # payload as the pages show it), actions, the live list (live.rs, SSE)
  retention.rs        # the daily retention job and the end of time-limited ignoring
  api.rs              # REST API v1 and its route table (API reference)
  mcp.rs              # MCP tools
templates/            # Kestrel's pages (extend kit/layout.html)
static/               # favicon, home-screen icons, web manifest
migrations/           # Kestrel's migrations (after the kit's core set); store/ has store.sqlite's own
locales/              # Kestrel's strings, 26 languages; glossaries in locales/glossary/
tests/                # in-process app tests, translation coverage, ingest, grouping, issue pages
  fixtures/sentry/    # requests recorded from real SDKs, per SDK and version
  fixtures/dsym/      # the dSYM of the sentry-cocoa fixtures' app (symbolication tests)
  snapshots/          # grouping snapshots (insta)
```

## Build, run, test

```bash
cargo run                      # http://localhost:3002, data in ./data
cargo run -- --help            # every flag and its KK_ variable
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The first start opens `/setup`, which creates the admin account.

| Flag | Variable | Default | What |
|---|---|---|---|
| `--data-dir` | `KK_DATA_DIR` | `./data` | `kestrel.sqlite`, `store.sqlite`, `session.key`, `backups/`, `difs/` (debug files) |
| `--host` | `KK_HOST` | `0.0.0.0` | listen address |
| `--port` | `KK_PORT` | `3002` | listen port |
| `--public-url` | `KK_PUBLIC_URL` | (none) | the address in DSNs and API examples, e.g. `https://kestrel.example.com`; without it each request's own address is used |
| `--ingest-port` | `KK_INGEST_PORT` | (none) | a second port that serves only ingest and `/healthz`, so the admin interface can stay on a LAN or VPN; ingest stays on the main port too |
| `--ingest-ip-rate-limit` | `KK_INGEST_IP_RATE_LIMIT` | `6000` | ingest requests per minute from one IP address, counted in memory only; `0` turns the limit off |
| `--capture-dir` | `KESTREL_CAPTURE_DIR` | (none) | write every ingest request to this directory ([capture mode](#capture-mode-and-sdk-fixtures)); test instances only |
| | `KK_TRUST_PROXY` | off | honour `X-Forwarded-*` from a reverse proxy (also for the ingest IP limit) |
| | `KK_SETUP_TOKEN` | (none) | `/setup` opens only with `?token=…` |
| | `RUST_LOG` | `info` | log filter |

`KK_PUBLIC_URL` is stored as the `base_url` setting at start-up (and removed when unset), so the kit's same-origin
checks, the API reference and the MCP origin check use the same address as the DSNs.

## Ingest

The official Sentry SDKs send to `POST /api/{project_id}/envelope/` (old ones to `/api/{project_id}/store/`) with
the DSN's public key in `X-Sentry-Auth`, `?sentry_key=` or the envelope header's `dsn`. The key must exist, be
enabled and belong to the project (401 or 403 otherwise). Kestrel answers `{"id": "<event_id>"}` without waiting
for the write. To try it against a running instance, take the key from the project's settings:

```bash
KEY=<public key>
printf '{}\n{"type":"event"}\n{"message":"Hello from curl","level":"warning"}\n' |
  curl -sS "http://localhost:3002/api/1/envelope/" \
    -H "X-Sentry-Auth: Sentry sentry_version=7, sentry_key=$KEY" --data-binary @-
```

What happens to a request (docs/design.md §4):

1. **Checks**, cheapest first: the per-IP bucket (`KK_INGEST_IP_RATE_LIMIT`), the project id, the body size, the
   key, the browser's `Origin` against the key's allowed origins, the key's own bucket (`rate_limit_per_minute`).
   Rate limits answer 429 with `Retry-After` and `X-Sentry-Rate-Limits`. The ingest routes are exempt from the CSRF
   check (`csrf_exempt_path` in `src/product.rs`) and answer CORS with `*` or the key's allowed origins.
2. **Decoding**: gzip, deflate, br or zstd, at most 20 MB per envelope, 1 MB per event and 10 MB per attachment
   once decompressed (413 when larger, 400 when broken). Larger attachments are dropped, the event is kept.
3. **Items**: `event` is normalised and grouped; `session`/`sessions`, `user_report`/`feedback` and the
   attachments of stored events are kept for later phases; `transaction`, `span`, `profile`, `replay_*`,
   `check_in`, `statsd`, `metrics`, `log`, `client_report`, minidumps and unknown types are accepted and dropped.
4. **Normalisation and privacy** (`src/event/normalize.rs`, `scrub.rs`): defaults, at most 100 breadcrumbs, 250
   frames per stack and 8 KB per string, `in_app` from the SDK or per-platform rules. No IP address is ever kept
   (`{{auto}}` included). `Authorization` and `Cookie` headers, values under keys matching
   `pass|secret|token|auth|key|card|cookie`, such `name=value` pairs and `Bearer …` credentials in text, and
   Luhn-valid card numbers become `[Filtered]`. `user.id` (on iOS the SDK's install id), `user.email` and
   `user.username` become keyed hashes unless the project's `keep_user_details` is on (no setting in the
   interface yet); the distinct-user counts hash the id either way. The user agent fills the browser and OS
   contexts and is then dropped. Session device ids are hashed with a key that changes daily.
5. **Symbolication** ([below](#debug-files-and-symbolication)): an event with an image the project has a debug
   file for goes through the symbolication queue first; everything else goes straight on.
6. **Grouping** (`src/event/grouping.rs`, configuration `kestrel:v1`): fingerprint (with `{{ default }}`),
   exception chain with in-app frames, exception type plus normalised value, message template, crashed thread.
   Native frames without symbols count as image and offset, and the issue is marked unsymbolicated. The
   components are stored in `issue_hashes`, so the issue page can explain them.
7. **The writer** (`src/writer.rs`): ingest puts the jobs on a bounded queue (2000 jobs; full means 429). One task
   writes them every 100 ms or 500 jobs in one IMMEDIATE transaction, on its own connection with `store.sqlite`
   attached: issues (new or updated, counts, first and last seen), releases and environments on first sight, tag
   values (the standard keys plus at most 20 custom ones per issue), distinct users per issue (`issue_users`),
   events per issue and hour (`issue_hours`), the search index. A resolved issue that gets an event becomes
   `regressed`, an ignored one whose condition is met `unresolved` again; both are noted in `issue_activity`. At
   most `payloads_per_day` (1000 unless the project says otherwise) payloads per issue and day are stored; the
   counts go on.

`kestrel.sqlite` holds the index; `store.sqlite` (own migrations in `migrations/store/`, incremental auto-vacuum,
no backup before migrating) holds payloads, attachments, sessions and feedback, and can be deleted without
losing issues.

### Capture mode and SDK fixtures

With `KESTREL_CAPTURE_DIR` set, every ingest request that gets past the IP limit and the key in its headers (if
it has one) is written to that directory as
`<millis>-<n>.json` (method, path, headers) and `<millis>-<n>.body` (the body as sent, usually compressed). The
files hold everything the SDK sent, keys and personal data included: use it on test instances only.

`tests/fixtures/sentry/<sdk>/<version>/<name>.{json,body}` are such recordings. `tests/ingest.rs` sends each one
and checks the answer, what is stored, and that no IP address, user agent, raw email address or secret ends up in
either database. `tests/grouping.rs` snapshots the title, culprit, tags and grouping of every event into
`tests/snapshots/` (`cargo insta review`). A changed grouping snapshot means changed grouping, which needs a new
grouping configuration rather than an accepted snapshot.

| SDK | Version | Recorded with | Shapes |
|---|---|---|---|
| `sentry-python` | 2.73.0 | a script in `python:3.13-alpine` | plain exception (twice), chained exception, logging message with a template, custom fingerprint, a WSGI request with user email, `Authorization` and `Cookie` headers, IP address, user agent, card number and secrets; sessions; client report |
| `@sentry/node` | 11.8.0 | a script in `node:22-alpine`, Express for the request | the same shapes (`Sentry.parameterize` for the template); session start and end, sessions, client report |
| `sentry` (Rust) | 0.49.3 | a program built in `rust:1.99-alpine` | plain error (twice), chained error (`Error::source`), message (the crate has no templates), custom fingerprint, an event with a request, user email and secrets, a panic |
| `sentry-cocoa` | 9.30.2 | the kestrel-spike iOS simulator run (stripped Release build), real envelopes | handled `NSError` (twice), crashes from `fatalError`, a force-unwrap and an `NSException`, a session |

The personal data and secrets in them are made up (`ada.lovelace@example.com`, `203.0.113.7`, `fixture-…`
values, the test card `4111 1111 1111 1111`). Before they were committed, the test server's kernel and CPU names
and the iOS app's bundle id were replaced, and changed envelopes were rebuilt with the right item lengths.
`@sentry/browser` isn't recorded yet: it needs a browser in a container.

To record another SDK or version: run a Kestrel with `KESTREL_CAPTURE_DIR` and a project with the key
`0123456789abcdef0123456789abcdef` (the tests swap in their own key, so any key works, but that one keeps the
files alike), point the SDK at `http://0123456789abcdef0123456789abcdef@<host>:3002/1`, send the shapes above, and
copy the files to `tests/fixtures/sentry/<sdk>/<version>/` under descriptive names. Build made-up values away from
the code that throws: SDKs send the surrounding source lines along.

### Load check

A small generator (Python, open loop, gzip envelopes of about 1 KB with a 12-frame exception, 20 breadcrumbs,
contexts, tags and a user, fresh event ids) against the release image limited to 2 CPUs and 2 GB on the test
server, data on tmpfs, 5000 kinds of error so that payloads keep being stored:

| Rate | Accepted | p50 | p99 | Max |
|---|---|---|---|---|
| 500 events/s for 30 s | all | 0.8 ms | 1.0 ms | 11 ms |
| 1000 events/s for 20 s | all | 0.7 ms | 1.0 ms | 12 ms |
| 2000 events/s for 20 s | all | 0.7 ms | 1.0 ms | 9 ms |
| 3000 events/s | the queue fills: about 1150 events/s accepted, the rest 429 | 92 ms | 173 ms | 200 ms |

Every accepted event was written. The container used 5 MB at rest and about 40–60 MB under load, apart from the
database files. The targets (500 events/s, p99 under 30 ms, docs/design.md §8) hold with room to spare; tmpfs
leaves out the cost of `fsync` on a real disk.

## Issues

The pages are in `src/issues/` and `templates/` (`issues.html`, `issue.html`, `event.html` and their partials).
Everyone signed in reads them; editors and admins act.

- **Project overview** (`/projects/{id}`): until the first event, the empty state and the DSN with setup snippets;
  then unresolved issues, events in the last 24 hours and 14 days (from `issue_hours`), a 14-day chart, the
  latest and the most frequent unresolved issues, and the setup folded into a disclosure.
- **Issue list** (`/projects/{id}/issues`): status chips (unresolved, resolved, ignored, all), level, release and
  environment filters, sort by last seen, first seen, events or users, 25 per page, 14-day sparklines, badges
  for new (first seen in the last 7 days), regressed and unsymbolicated issues. Filter changes swap the results
  with htmx and push the URL; the status chips come back out of band. While the list is open it listens on
  `/projects/{id}/issues/live` (server-sent events with the same query string): the writer says which project
  changed after each batch and each action, and the list is rendered again at most about once a second. Select
  mode (`?select=1`, editors) resolves, ignores, unresolves or merges the ticked issues.
- **Search** (`src/issues/search.rs`): `is:unresolved|resolved|ignored|all|new|regressed|ongoing|unsymbolicated`,
  `level:`, `release:` (as sent, or only the version: `release:1.4` matches `shop@1.4` and `shop@1.4+12`; `*` is
  a wildcard), `env:`, `os:`/`browser:`/`device:`/`runtime:` (name, or name and version), any other `key:value`
  tag, `!` to negate, double quotes for values and phrases with spaces. Other words search titles and culprits
  through FTS5 as prefixes; an event id finds its issue. Terms that can't be understood are listed above the
  results. An `is:` status in the search wins over the chips until a chip is picked.
- **Issue page** (`/projects/{id}/issues/{issue}`): title, culprit, level, status and badges, counts, first and last
  seen with their releases; the tabs Details (the latest event in full, the issue's facts, the top releases,
  environments, systems, browsers, devices and runtimes, the activity log), Events (50 per page), Tags (top values
  per key) and Grouping (each stored hash with its components and grouping configuration). The event page
  (`…/events/{event_id}`) shows one event with newer and older links.
- **An event** (`src/issues/view.rs`): the exception chain from the main exception to its causes, stack traces
  innermost frame first with the in-app frames shown and runs of other frames folded (`?frames=all` shows
  them all), source lines and variables of a frame, the crashed thread's stack when the exceptions have none,
  breadcrumbs (newest first), the request, contexts, user, tags, extra data, SDK and the raw JSON as stored
  (already scrubbed at ingest).
- **Actions** (`POST …/issues/{issue}/action`, `POST …/issues/bulk`, `src/db/triage.rs`): resolve; resolve in the
  next release (the project's newest release is stored, and only events from a release first seen after it
  regress the issue; without any release it resolves now); unresolve; ignore forever, until 10, 100 or 1000 more
  events, or for 1 hour, 24 hours or 7 days; merge (into the ticked issue with the most events: hashes, events,
  tag values, users and hourly counts move over, the counts add up, the others are deleted); delete (with its
  events and their payloads, attachments and feedback; new events like it start a new issue). They run on the
  writer's connection (`Writer::run`), so they never race ingest, and each is noted in `issue_activity` with the
  user's name. Deleting a project goes through the writer too, so its rows in `store.sqlite` go with it.

### Retention

`src/retention.rs` runs a minute after start and then once a day, in batches through the writer, per project
(Settings → Retention: `retention_event_days` 30, `retention_issue_days` 90, `payloads_per_day` 1000):

1. event details (payloads, attachments, feedback) older than the event retention go; the events keep their index
   rows, so counts, users and the event list stay, and the event page says the details are gone;
2. issues last seen longer ago than the issue retention go, with their events and details;
3. hourly counts older than 30 days go;

then rows in `store.sqlite` of projects that no longer exist go, new issues first seen (and regressed ones last
regressed) more than 7 days ago become `ongoing`, and `store.sqlite` gives up to 10,000 freed pages back
(`incremental_vacuum`). Every minute the same task ends ignoring that ran out by time, so an issue ignored for
an hour comes back without a new event. Session items are left for the release-health rollups (phase 7). Debug
files follow their own rule ([below](#debug-files-and-symbolication)).

## Debug files and symbolication

Native crashes (sentry-cocoa and other SDKs that send `debug_meta.images` and instruction addresses) are
symbolicated from uploaded debug files: dSYMs, and the other object files getsentry's `symbolic` reads (ELF and
DWARF, PE and PDB, Breakpad, WASM). docs/design.md §4.8.

**Storage.** One row per object in `dif_files` (project, debug id, code id, object name, architecture, file
format, features, size, SHA-1 of the uploaded file, the release that last used it, uploaded and last used).
The files are in the data directory: `difs/<project_id>/<debug_id>/object` (one architecture of a dSYM) and
`difs/<project_id>/<debug_id>/symcache`, made once at upload. The object is kept so that a SymCache a newer
`symbolic` can't read is made again on first use. `difs/chunks/` holds chunks until their file is assembled,
`difs/tmp/` files being assembled or unpacked (emptied at start). An object that only has a symbol table (the
stripped app binary) never replaces one with debug information for the same debug id.

**Upload** (`src/difs/upload.rs`): sentry-cli's protocol, so `sentry-cli debug-files upload`, fastlane's Sentry
plugin and Xcode build phases work unchanged.

```bash
export SENTRY_AUTH_TOKEN=kk_…   # an upload token (Debug files page) or an editor's read-and-write token
sentry-cli --url https://kestrel.example.com debug-files upload --org any --project <slug or id> --wait App.app.dSYM
```

| Request | What |
|---|---|
| `GET /api/0/` | the token's user and scopes (`project:releases` when it may upload) |
| `GET /api/0/organizations/{org}/chunk-upload/` | chunks of 8 MiB, 64 and 32 MiB per request, files up to 1 GiB, SHA-1, gzip, accepts `debug_files` |
| `POST /api/0/organizations/{org}/chunk-upload/` | multipart chunks (`file`, or `file_gzip` gzip-compressed), each named by its SHA-1, which is checked |
| `POST /api/0/projects/{org}/{project}/files/difs/assemble/` | `{sha1: {name, debug_id, chunks}}` → per file `not_found` with `missingChunks`, `assembling`, `ok` with the stored `dif`, or `error` with `detail` |
| `POST /api/0/projects/{org}/{project}/files/dsyms/` | the older upload: a zip (or one object file) as multipart `file`; answers the stored files |
| `POST /api/v1/projects/{id}/dsyms` | the REST API: a zip as the body (`Content-Type: application/zip`) or as multipart `file` |
| `GET /api/v1/projects/{id}/debug-files`, `DELETE …/debug-files/{dif_id}` | list and delete |

Any organization slug is accepted; the project is Kestrel's slug or numeric id. sentry-cli asks for its files
first (`not_found`, every chunk missing), sends the missing chunks and asks again: the file is joined, its SHA-1
checked, and each object in it stored with its SymCache. A small file is done within that request (`ok`); one
that takes longer than 10 seconds answers `assembling` until it is (`--wait` polls). The outcome is remembered
for ten minutes. At most two files are processed at once.

The `/api/0/` routes take API tokens only (no sessions, so they are exempt from the CSRF check). A token may
upload when it is a read-and-write token of an editor or admin, or when it has Kestrel's `difs:write` grant
(`token_grants`) and its owner is an editor or admin. Upload tokens are made on a project's Debug files page: a
read-only kit token plus the grant, so a build machine's token can't change anything else. The kit has no
product scopes yet, so the API tokens page shows them as read-only tokens. The zip endpoint also takes an
editor's session.

Limits: chunks of at most 8 MiB (after gzip too), 64 per request, request bodies of 33 MiB; files of at most
1 GiB; zip uploads of at most 1 GiB with at most 10,000 entries and 2 GiB unpacked in all, and no entry over
16 MiB that unpacks to more than 100 times its packed size (zip bombs). Only entries that look like object files
are unpacked; the rest (Info.plist, Swift interfaces, relocation maps) are listed as `skipped`.

**Symbolication at ingest** (`src/difs/symbolicate.rs`, `worker.rs`). Ingest keeps an in-memory index of
`(project, debug id)`. An event whose `debug_meta.images` names a debug file of its project is normalised on the
request as usual, then handed with the rest of its envelope to the symbolication queue (256 envelopes, two
workers; full means 429) instead of the writer, and ingest answers at once. A worker matches each frame to its
image (`image_addr`, else the address range), moves return addresses back into their call instruction
(`symbolic_common::InstructionInfo`; the crashing frame and its `pc` stay as they are), looks the offset up in
the memory-mapped SymCache, and replaces the frame with the function and its inlined callees, caller first:
`function` (Swift demangled the way Sentry shows it, `CrashPipeline.Stage.detonate(CrashKind)`), `symbol` (as
mangled), `filename`, `abs_path`, `lineno`, `symbol_addr`, `inlined: true` on the inlined ones and
`data.symbolicator_status: "symbolicated"`; a name the SDK sent moves to `raw_function`. Frames of images
without a debug file (system frameworks) stay as the SDK sent them. Then the event is grouped, so grouping uses
the symbolicated frames (file and function instead of image and offset), and the jobs go to the writer in order.
The writer notes `last_used_at` and the event's release on the debug files (at most every ten minutes per file).

Events without a matching debug file stay unsymbolicated as before: grouped by image and offset, the issue
marked. A debug file applies to events that arrive after it; symbolicating stored events again is planned for
v1.1. The event page names the debug id of each in-app frame whose dSYM was missing and links to the Debug files
page; symbolicated frames show their function, file and line, inlined ones with a badge.

Measured with the release image limited to 2 CPUs and 2 GB on the test server (data on tmpfs, the sentry-cocoa
`fatalError` crash, gzip, about 5 KB, sent open-loop with `oha`; its 100-odd frames resolved from the dSYM):

| Rate | Accepted | p50 | p99 | Max |
|---|---|---|---|---|
| 500 crashes/s for 30 s, symbolicated | all | 1.0 ms | 1.3 ms | 12 ms |
| 500 crashes/s for 30 s, no dSYM (grouped on the request) | all | 1.5 ms | 2.0 ms | 4 ms |
| 1000 crashes/s for 30 s, symbolicated | 97 %, the rest 429 (the queue fills) | 1.0 ms | 179 ms | 281 ms |

The container used about 35 MB at 500/s and 54 MB at 1000/s.

**The Debug files page** (`/projects/{id}/debug-files`): the files (object, architecture, debug id, size, debug
information or only symbols, uploaded, last used and by which release), deleting for editors, how to upload
(sentry-cli, an Xcode Run Script phase, fastlane, a zip with curl, all with this instance's address and the
project's slug), and the upload tokens.

**Retention** (Settings → Retention): a debug file goes when it was neither uploaded nor used for
`retention_dif_days` (180), unless one of the project's `retention_dif_releases` (5) newest releases used it.
The daily job also removes chunks older than a day, directories without a row and the files of deleted projects.

**Tests.** `tests/fixtures/dsym/KestrelSpike.app.dSYM.zip` (1.9 MB) is the dSYM of the kestrel-spike iOS app
(Swift, a stripped Release build for the simulator, arm64 and x86_64) the sentry-cocoa fixtures crashed in; the
kestrel-spike CI run 39665575186 produced both. `tests/difs.rs` uploads it and replays the crashes: function,
file and line of `CrashSite.swift`'s call chain for the `fatalError`, force-unwrap and `NSException` crashes,
the inlined `trigger(_:depth:)`, demangled Swift, system frames unchanged, the caller adjustment (line 43 for
`detonate`'s return address), grouping by function (and by image and offset without the dSYM). It also runs the
chunk-upload protocol in-process (chunks plain and gzip-compressed, missing chunks, wrong checksums, oversized
chunks), the legacy and zip endpoints, a zip bomb, which tokens may upload, the page and retention.

The real sentry-cli was checked too (3.8.2, the `getsentry/sentry-cli` image: `info` and `debug-files upload
--wait` of the dSYM, both architectures `OK`). To repeat it on the test server:

```bash
docker run --rm -v <dir with the dSYM>:/work -e SENTRY_AUTH_TOKEN=kk_… getsentry/sentry-cli \
  --url http://<kestrel>:3002 debug-files upload --org any --project <slug> --wait /work/KestrelSpike.app.dSYM
```

then send the sentry-cocoa fixtures with the project's key and open the issues.

## Working on vole-kit at the same time

Cargo.toml uses the kit's release tag. To try a kit change, point Cargo at a checkout of the Vigilant Vole
repository next to this one with a local, uncommitted `.cargo/config.toml` (it is in `.gitignore`):

```toml
[patch."https://github.com/Ferdinand99/Vigilant-Vole"]
vole-kit = { path = "../Vigilant-Vole/kit" }
vole-kit-build = { path = "../Vigilant-Vole/kit/kit-build" }
```

Remove it (or move the tag in Cargo.toml once the kit is released) before you commit `Cargo.lock`. Edit the kit's
templates in the Vigilant Vole repository; `templates/kit/` is a build-time copy and is overwritten.

## Docker

```bash
docker build -t keen-kestrel:latest .
docker run -d --name keen-kestrel -p 3002:3002 -v ./data:/data keen-kestrel:latest
docker compose up -d           # the same with docker-compose.yml
```

The image is a static musl binary on Alpine. The container starts as root only to hand `/data` to the `kestrel`
user (uid 1000, or `PUID`/`PGID`), then runs as that user. `HEALTHCHECK` calls `/healthz`.

## CI and releases

- `.github/workflows/ci.yml`: `cargo fmt --check`, clippy with `-D warnings` and `cargo test` on Rust 1.99; on pull
  requests also a Docker build for amd64 and arm64 (cache only).
- `.github/workflows/docker-publish.yml`: multi-arch images on GHCR, `:edge` from main and `:X.Y.Z`, `:X.Y`, `:X`,
  `:latest` for releases.
- `.github/workflows/release-please.yml`: one release PR for `keen-kestrel` (tags `vX.Y.Z`, `CHANGELOG.md`),
  starting from 0.0.0, so the first `feat` releases 0.1.0. Merging it publishes the image.

## Translations

`locales/en.toml` holds Kestrel's own English strings; the other 25 files translate every key, with the words from
`locales/glossary/<code>.md`. `tests/i18n.rs` calls vole-kit's `assert_full_coverage()`, which fails when a language
misses a key, has an extra one, changes a placeholder or lacks a plural form its language needs. Core strings
(sign-in, users, tokens, Security, the layout) come from vole-kit; a key may not be in both catalogs. The few
texts the kit's pages take from the product (`vole_kit::i18n::catalog::PRODUCT_KEYS`: the sign-in subtitle, the
setup page's placeholders, what roles and tokens may do, the setup-token hint naming `KK_SETUP_TOKEN`, the error
page's link) are at the end of each of Kestrel's files.
