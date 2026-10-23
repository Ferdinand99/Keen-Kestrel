# Development

Keen Kestrel is one Rust crate (`keen-kestrel`, binary `kestrel`) on top of
[vole-kit](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/docs/vole-kit.md), the shared kit that lives in the
Vigilant Vole repository (tag `vole-kit-v0.3.0`). The kit brings sign-in, users, API tokens (with Kestrel's
`difs:write` scope), Security, General settings, the API reference, i18n, the database plumbing, request budgets,
live updates, the notification channels' forms and the design system; Kestrel adds its own pages, API, migrations
and strings, the
Sentry-compatible ingest ([below](#ingest)), debug files with symbolication
([below](#debug-files-and-symbolication)), alerts ([below](#alerts)), release health
([below](#releases-and-release-health)), the REST API and MCP tools ([below](#rest-api-and-mcp)), web analytics
([below](#web-analytics)) and app analytics ([below](#app-analytics)).

## Layout

```
Cargo.toml            # keen-kestrel; vole-kit by git tag
build.rs              # copies the kit's templates into templates/kit/ (git-ignored)
rust-toolchain.toml   # Rust 1.99
src/
  main.rs             # `kestrel`: flags/env, logging, server
  lib.rs              # start() (config, database, state) and router()
  product.rs          # KitConfig: names, cookies, token prefix and scope, CSRF exemptions, nav, settings
                      # tabs (General), the webhook's words, locales, static files
  config.rs           # flags and KK_* environment variables
  server.rs           # listener and graceful shutdown
  project.rs          # project and key rules (names, slugs, origins, rate limits)
  db/                 # migration set, projects and project_keys, issues (the writer's upsert),
                      # triage (the issue pages' reads, lifecycle actions, retention deletes)
  dsn.rs              # public address, DSNs, SDK snippets
  ingest/             # Sentry endpoints: keys, body decoding and limits, envelopes, capture mode;
                      # web.rs: POST /ingest/v1/event; signals.rs: POST /ingest/v1/signals
  event/              # normalisation and privacy (scrub.rs, paths.rs), grouping (kestrel:v2), title, culprit, tags
  difs/               # debug files: on disk (files.rs), sentry-cli upload (upload.rs), symbolication
                      # (symbolicate.rs), the symbolication queue (worker.rs), the Debug files page (pages.rs)
  writer.rs           # the single batch writer
  store.rs            # store.sqlite: event payloads, attachments, sessions, feedback (its pool and migrations)
  pages.rs            # project pages: list, overview, settings (keys, retention)
  issues/             # issue pages: list (search.rs: the search syntax), issue, event (view.rs: the
                      # payload as the pages show it), actions, the live list (live.rs, SSE)
  alerts/             # rules, evaluation and delivery (mod.rs), message wording (message.rs), the Alerts tab
                      # (pages.rs), Settings → Notifications (channels.rs)
  releases/           # the releases pages (mod.rs), release-health rollup (health.rs), sentry-cli's release
                      # commands (cli.rs)
  analytics/          # web analytics: k.js and its handler (kjs.rs), what a pageview keeps (clean.rs, ua.rs),
                      # the rollups (rollup.rs), the queries (query.rs), the Web tab (pages.rs), what the
                      # live streams wake for (live.rs); app/: app signals (what a signal keeps, hashes,
                      # rollup, queries, the App tab and explorer)
  retention.rs        # the daily retention job; every minute the end of time-limited ignoring and the rollups
  api.rs, api/        # REST API v1 and its route table (API reference); issues and events, releases, alert rules,
                      # web and app analytics
  mcp.rs              # MCP tools
templates/            # Kestrel's pages (extend kit/layout.html)
static/               # favicon, home-screen icons, web manifest, the Web and App tabs' few own styles
migrations/           # Kestrel's migrations (after the kit's core set); store/ has store.sqlite's own
locales/              # Kestrel's strings, 26 languages; glossaries in locales/glossary/
tests/                # in-process app tests, translation coverage, ingest, grouping, issue pages, debug files,
                      # alerts, releases, the API and MCP for errors, web analytics, app analytics
  kjs/                # k.js's behaviour in Node (behaviour.mjs)
  fixtures/kestrelkit/ # KestrelKit's golden request bodies
tools/loadtest/       # load generators: web.py (web analytics), signals.py (app signals)
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
| `--public-url` | `KK_PUBLIC_URL` | (none) | the address in DSNs and API examples, e.g. `https://kestrel.example.com`; stored as the public URL at every start (Settings → General otherwise); without either, each request's own address is used |
| `--ingest-port` | `KK_INGEST_PORT` | (none) | a second port that serves only ingest and `/healthz`, so the admin interface can stay on a LAN or VPN; ingest stays on the main port too |
| `--ingest-ip-rate-limit` | `KK_INGEST_IP_RATE_LIMIT` | `6000` | ingest requests per minute from one IP address, counted in memory only; `0` turns the limit off |
| `--capture-dir` | `KESTREL_CAPTURE_DIR` | (none) | write every ingest request to this directory ([capture mode](#capture-mode-and-sdk-fixtures)); test instances only |
| | `KK_TRUST_PROXY` | off | honour `X-Forwarded-*` from a reverse proxy (also for the ingest IP limit) |
| | `KK_SETUP_TOKEN` | (none) | `/setup` opens only with `?token=…` |
| | `RUST_LOG` | `info` | log filter |

The public URL is the kit's `base_url` setting, which admins set on Settings → General (the kit's page: site name,
public URL, time zone, language). `KK_PUBLIC_URL`, when set, is stored there at every start and so wins over the
page; when it is unset, the saved value stays. The kit's same-origin checks, the API reference and the MCP origin
check use the same address as the DSNs. The site language chosen there is also the language of notification
channels that have none of their own.

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
   The buckets are the kit's token buckets (`vole_kit::http::ratelimit::TokenBuckets`, in memory, per IP address
   and per key); rate limits answer 429 with `Retry-After` and `X-Sentry-Rate-Limits` and Kestrel's Sentry-style
   body. The ingest routes are exempt from the CSRF check (`csrf_exempt_prefixes` in `src/product.rs`:
   `/api/*/envelope`, `/api/*/store`, `/ingest/v1/event`, `/ingest/v1/signals`), carry the kit's machine header
   policy (`headers::machine`: a CSP that allows nothing, no frame rules) and answer CORS with `*` or the key's
   allowed origins. `GET /js/k.js` is a public asset (`headers::public_asset`: no CSP, `Cross-Origin-Resource-Policy:
   cross-origin`).
2. **Decoding**: gzip, deflate, br or zstd, at most 20 MB per envelope, 1 MB per event and 10 MB per attachment
   once decompressed (413 when larger, 400 when broken). Larger attachments are dropped, the event is kept.
3. **Items**: `event` is normalised and grouped; `session`/`sessions` are kept for the release-health rollup
   ([below](#releases-and-release-health)); `user_report`/`feedback` and the attachments of stored events are kept; `transaction`, `span`, `profile`, `replay_*`,
   `check_in`, `statsd`, `metrics`, `log`, `client_report`, minidumps and unknown types are accepted and dropped.
4. **Normalisation and privacy** (`src/event/normalize.rs`, `scrub.rs`): defaults, at most 100 breadcrumbs, 250
   frames per stack and 8 KB per string, `in_app` from the SDK or per-platform rules. No IP address is ever kept
   (`{{auto}}` included). `Authorization` and `Cookie` headers, values under keys matching
   `pass|secret|token|auth|key|card|cookie`, such `name=value` pairs and `Bearer …` credentials in text, and
   Luhn-valid card numbers become `[Filtered]`. `user.id` (on iOS the SDK's install id), `user.email` and
   `user.username` become keyed hashes unless the project's `keep_user_details` is on (no setting in the
   interface yet); the distinct-user counts hash the id either way. The user agent fills the browser and OS
   contexts and is then dropped. Session device ids are hashed with a key that changes daily. Paths from build
   machines lose their home directory (`src/event/paths.rs`): `/Users/<name>/…`, `/home/<name>/…`,
   `C:\Users\<name>\…` and CI workspaces (GitHub Actions, Xcode Cloud, Bitrise, CircleCI, GitLab, Jenkins …)
   become a path relative to the project (or at least without the home directory), in frames (`abs_path`,
   `filename`, `package`) and loaded images (`code_file`, `debug_file`), for what the SDKs send and what
   symbolication takes from a dSYM alike. The file names stay, so grouping doesn't change.
5. **Symbolication** ([below](#debug-files-and-symbolication)): an event with an image the project has a debug
   file for goes through the symbolication queue first; everything else goes straight on.
6. **Grouping** (`src/event/grouping.rs`, configuration `kestrel:v2`): fingerprint (with `{{ default }}`),
   exception chain with in-app frames, exception type plus normalised value, message template, crashed thread.
   Native frames without symbols count as image and offset, and the issue is marked unsymbolicated. The
   components are stored in `issue_hashes`, so the issue page can explain them. v2 is v1 plus one rule: the
   Sentry SDKs' own frames (Cocoa `Sentry…`, `SentryCrash…`, `-[Sentry…]`, `Sentry.framework`; `@sentry/…`;
   `sentry_sdk`; the Rust crates and sentry-native's `sentry_…`) are never in-app, so an app that links the SDK
   statically isn't grouped by it. Each event is grouped by both: an event whose v2 hash is unknown joins the
   issue its v1 hash points at, which keeps `kestrel:v1` and gets the v2 hash too (both show on the Grouping tab).
   A crash (fatal, or an exception the SDK says nothing handled) notes its release's first crash.
7. **The writer** (`src/writer.rs`): ingest puts the jobs on a bounded queue (2000 jobs; full means 429). One task
   writes them every 100 ms or 500 jobs in one IMMEDIATE transaction, on its own connection with `store.sqlite`
   attached: issues (new or updated, counts, first and last seen), releases and environments on first sight, tag
   values (the standard keys plus at most 20 custom ones per issue), distinct users per issue (`issue_users`),
   events per issue and hour (`issue_hours`), the search index. A resolved issue that gets an event becomes
   `regressed`, an ignored one whose condition is met `unresolved` again; both are noted in `issue_activity`. At
   most `payloads_per_day` (1000 unless the project says otherwise) payloads per issue and day are stored; the
   counts go on. After each committed batch the writer hands the alerts what may set them off (new issues,
   regressions, first crashes, issues with events) without waiting ([below](#alerts)).

`kestrel.sqlite` holds the index; `store.sqlite` (own migrations in `migrations/store/`, run by the kit's
`migrate_with` without a backup; incremental auto-vacuum, set by the kit's pool through
`PoolOptions::new_database_pragmas` before the first table exists) holds payloads, attachments, sessions, feedback
and raw analytics events, and can be deleted without losing issues or analytics rollups.

### Capture mode and SDK fixtures

With `KESTREL_CAPTURE_DIR` set, every ingest request that gets past the IP limit and the key in its headers (if
it has one) is written to that directory as
`<millis>-<n>.json` (method, path, headers) and `<millis>-<n>.body` (the body as sent, usually compressed). The
files hold everything the SDK sent, keys and personal data included: use it on test instances only.

`tests/fixtures/sentry/<sdk>/<version>/<name>.{json,body}` are such recordings. `tests/ingest.rs` sends each one
and checks the answer, what is stored, and that no IP address, user agent, raw email address, secret or user name
in a build machine's path ends up in either database. `tests/grouping.rs` snapshots the title, culprit, tags and grouping of every event into
`tests/snapshots/` (`cargo insta review`). A changed grouping snapshot means changed grouping, which needs a new
grouping configuration rather than an accepted snapshot. The move to `kestrel:v2` changed every snapshot's
configuration and hash (the configuration's name is part of the hash) and nothing else, because no fixture has a
Sentry SDK frame marked in-app; the rule has its own tests in `src/event/grouping.rs`.

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
  `/projects/{id}/issues/live` (server-sent events with the same query string, built on the kit's `http::live`):
  the writer says which project changed after each batch and each action, and the list is rendered again at most
  about once a second; lists open on the same view share one render. Select
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
4. raw analytics events and analytics rollups older than the project's analytics retention go ([below](#web-analytics));

then rows in `store.sqlite` of projects that no longer exist go, new issues first seen (and regressed ones last
regressed) more than 7 days ago become `ongoing`, release-health rollups older than 400 days go, so does the
alert history older than 90 days, and `store.sqlite` gives up to 10,000 freed pages back (`incremental_vacuum`).
Every minute the same task ends ignoring that ran out by time, so an issue ignored for an hour comes back without
a new event, and rolls the session items up into release health (which deletes them). Debug files follow their
own rule ([below](#debug-files-and-symbolication)).

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
upload when it has the `difs:write` token scope and its owner is an editor or admin
(`user.has_scope("difs:write") && user.may_act_as(Role::Editor)`); a read-and-write token has every scope.
`difs:write` is a kit token scope (`KitConfig::token_scopes`, stored in the kit's `api_token_scopes`): a
read-only token that also uploads, so a build machine's token can't change anything else. Upload tokens are made
on a project's Debug files page or on the API tokens page ("Read and upload debug files", which is also the
token's access there). `migrations/0008_token_scopes.sql` moved the grants of Kestrel 0.2 (`token_grants`) into
the kit's table, so existing upload tokens kept working. The zip endpoint also takes an editor's session.

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

## Alerts

docs/design.md §4.7. `src/alerts/`, tables in `migrations/0005_alerts_releases.sql`.

**Channels** (Settings → Notifications, `/settings/notifications`, editors and admins) are the instance's, with
vole-kit's ~40 providers: the provider picker, the provider's form rendered from its schema, secrets never shown
again (an empty secret keeps the stored one; changing a destination asks for its secrets again), "Send test",
pause and delete. Each channel has a message language ("" means the instance's: the site language from Settings →
General); new channels start in the language of the person adding them. The provider picker and form are the
kit's (`vole_kit::notify::form` and its `kit/partials/notification_*.html`); the list, the language and the
toggles are Kestrel's (`src/alerts/channels.rs`). "Send test" sends the kit's test alert (`Alert::test`) in the
channel's language (`notify::send_in`). The webhook provider speaks of an issue (`KitConfig::webhook`): its default
body has `issue`, `issue_id`, `issue_type` and `duration_seconds`, its template the `{{issue_name}}` …
placeholders.

**Rules** (a project's Alerts tab, `/projects/{id}/alerts`; editors change them, everyone reads them):

| Kind | Fires when |
|---|---|
| `new_issue` | an issue is made |
| `regression` | a resolved issue gets an event (and becomes regressed) |
| `spike` | the issue's events in the last hour are more than `max(threshold, multiplier × the hourly average of the 168 hours before)`; the last hour is this hour's events plus the share of the previous hour the 60 minutes still cover (`issue_hours`); checked at most once a minute per issue, unresolved issues only |
| `first_crash` | a release gets its first crash: a `fatal` event, or an exception with `mechanism.handled: false` (`releases.first_crash_at`, once per release) |

`environment` limits the other kinds to one environment. An issue sets a rule off at most once per
`cooldown_minutes` (default 60, `alert_cooldowns`). With `digest_minutes`, a channel gets one message with
everything its digest rules raised in that many minutes.

**Delivery never waits for anyone.** The writer hands its triggers over a bounded queue it never waits for (a full
queue drops the batch with a warning). A task evaluates the rules on the read pool and writes one `alert_log` entry
per channel (`pending`, or `digest` with the channel's `due_at`); a paused channel gets none. Another task, woken
by new entries and every 15 seconds, sends what is due, at most eight at a time, through `vole_kit::notify::send`,
retries after 1, 5 and 15 minutes (then `failed`), and notes every result, which the Alerts tab and
`GET /api/v1/projects/{id}/alerts` show. The queue is in SQLite, so a restart loses nothing.

**Messages** (`src/alerts/message.rs`) are built when they are sent, in the channel's language, as vole-kit
`Alert`s: "New issue in Shop — ValueError: bad input", "Issue regressed in Shop", "Spike in Shop: 512 events in
the last hour", "First crash in shop@1.4.1 (Shop)", "3 alerts from Keen Kestrel" (a digest), with the project,
level, release, environment, culprit, events and users as facts. Kind and level follow the issue: a new issue,
regression or first crash is a `Problem`, a spike an `Escalation`; fatal and error are `Critical`, warning
`Warning`, the rest `Info` (first crashes are always `Critical`). The link is the issue on the public URL; without
one the message says to set it.

To try it on a test instance, run a receiver nobody else uses, for example
`docker run --rm -p 8099:8080 mendhak/http-https-echo` on a test machine, add a Webhook channel pointing at it, a
"New issue" rule on a project, and send an event with `curl` ([above](#ingest)).

## Releases and release health

docs/design.md §4.9. A release is made on first sight (an event or a session names it) or by
`sentry-cli releases new`; `releases.events` counts every event of it.

**The rollup** (`src/releases/health.rs`, every minute on the writer's connection) reads `store.sqlite`'s
`session_items`, adds them to `release_health` (per project, release, environment and UTC day) and deletes them.
A session counts on the day it started, clamped to the day it arrived or the one before. `session` updates:
`init: true` counts a session; `errors > 0` or an `errored`, `crashed` or `abnormal` status counts it as errored,
and `crashed`/`abnormal` as such, each session once by its id; the `did` (already hashed with the day's key at
ingest) counts distinct users and crashed users. `sessions` aggregates (server SDKs) add their counts as they are.
The hashes counted on a day are kept in `release_health_seen` until the day can't get sessions any more (two days),
then deleted. Crash-free sessions are `1 − crashed / sessions`, crash-free users `1 − crashed users / users`; over
several days the daily user counts add up. Rollups are kept 400 days.

**Pages**: the Releases tab (`/projects/{id}/releases`: version, crash-free sessions and users, adoption as the
share of the last 7 days' sessions, new issues, events, first and last seen) and a release's page (crash-free
rates, sessions, crashes, errors and users, charts of sessions and crash-free sessions per day over 30 days, the
issues first seen in the release, the issues it brought back, and the debug files its crashes were symbolicated
with). The issue and event pages link their releases (`/projects/{id}/releases/find?version=…`).

**sentry-cli** (`src/releases/cli.rs`): `releases new`, `finalize`, `info`, `list`, `set-commits` and
`deploys new` get answers in Sentry's shape (`/api/0/organizations/{org}/releases/…`,
`/api/0/projects/{org}/{project}/releases/…`, `/api/0/organizations/{org}/repos/` is empty). Releases are made in
the named projects and `finalize` notes `dateReleased`; commits, refs and deploys are accepted and not kept.
Reading takes any API token; changing takes one that may upload debug files.

## REST API and MCP

The REST API v1 (`src/api.rs`, `src/api/`; reference at `/api-docs`, `/api-docs.md`, `/llms.txt` and
`/api/v1/openapi.json`, all from the same route table) has, besides projects, keys and debug files:

- issues: `GET …/issues?query=` with the issue list's search syntax (`status`, `sort`, opaque `cursor`), one issue
  with its grouping, `PATCH` its status (`resolved`, `in_next_release`, `unresolved`, `ignored` with
  `ignore_events` or `ignore_hours`), `POST …/issues/merge`, `DELETE`;
- events: an issue's events, its latest event with the payload, an event by its id;
- releases with their all-time health and 7-day adoption, a release's health per day (`?days=&environment=`);
- alert rules (list, create, change, delete, with validation per field), the alerts sent, and the notification
  channels by name and kind (never their settings, editors only);
- web analytics: a time series with totals and a breakdown by one dimension ([below](#web-analytics));
- app analytics: a time series, breakdowns, signal types, a type's payload and version adoption
  ([below](#app-analytics)), and each release's share of the app's users;
- the ingest endpoints (group "Ingest", access "a project's DSN key", with `POST /ingest/v1/event`, `GET /js/k.js` and
  `POST /ingest/v1/signals`)
  and sentry-cli's (group "sentry-cli", "any API token").

Read tokens act as viewers; changing needs a read-and-write token of an editor or admin, and is noted in the
issue's activity under the token owner's name.

The MCP server (`/mcp`) has `list_projects`, `list_issues` (the search syntax), `get_issue` (the issue and its
latest event as text for a model: the exception chain, in-app frames innermost first with `file:line` and the
line of code, folded runs of other frames, the last ten breadcrumbs, the request line and tags; never the user
section), `release_health`, `analytics_summary` and `analytics_breakdown` (web, or app with `source: app`), and for a
read-and-write token of an editor
`resolve_issue` and `ignore_issue`.

## Web analytics

docs/design.md §5; the user guide is [web-analytics.md](web-analytics.md). `src/analytics/`, `src/ingest/web.rs`,
`migrations/0006_analytics.sql` (rollups and per-project settings) and `migrations/store/0002_analytics.sql` (raw
events).

**The snippet** (`src/analytics/k.js`, served by `kjs.rs` at `GET /js/k.js`): readable source with comments, served
with comment lines, indentation and blank lines removed (a rule that can't change what it does, because the source has
no comment after code and no multi-line string; a unit test holds it to that), gzip-compressed when the browser takes
it, `Cache-Control: public, max-age=86400`, an ETag (304 on `If-None-Match`), `Access-Control-Allow-Origin: *` and
`Cross-Origin-Resource-Policy: cross-origin`. Served: about 2.1 KB, 1.0 KB gzip-compressed (a unit test and
`tests/analytics.rs` keep it under 2 KB). It sends a pageview on load (after prerendering), on `pushState`,
`replaceState` and `popstate` when the path changed, and on `hashchange` with `data-hash`; it skips `localhost`,
`127.x`, `[::1]` and `file:` unless `data-localhost`, paths matching `data-exclude`, `navigator.webdriver` and
`localStorage.kestrel_ignore = 'true'`, and sends nothing when `navigator.globalPrivacyControl` is true.
`window.kestrel('event', name, props)` sends a custom event (calls queued in
`kestrel.q` before it loaded are sent too). It posts with `fetch(…, {keepalive: true, credentials: 'omit'})`, so no
cookie of the Kestrel domain goes along, and falls back to `navigator.sendBeacon`; the body is a JSON string, so the
browser sends `text/plain` and needs no preflight.

`tests/kjs/behaviour.mjs` runs k.js in Node with a small fake DOM, as written and as served (58 checks: the payload,
SPA navigation, hash mode, localhost, exclusions, opt-out, webdriver, Global Privacy Control, custom and queued events,
`data-api`, `sendBeacon`, prerendering, no cookie or storage writes):

```bash
tar -cf - src/analytics/k.js tests/kjs | docker run --rm -i -w /w --tmpfs /w:exec node:22-alpine \
  sh -c 'tar -xf - && node tests/kjs/behaviour.mjs src/analytics/k.js'
# or against a running instance: node tests/kjs/behaviour.mjs http://<kestrel>:3002/js/k.js
```

**Ingest** (`POST /ingest/v1/event`, also on the ingest port; exempt from the CSRF check): in order, the per-IP bucket
(`KK_INGEST_IP_RATE_LIMIT`), Global Privacy Control (`Sec-GPC: 1`: answered 202 and dropped; `DNT` is ignored), bots (user agents that aren't Mozilla-compatible or name a crawler, preview, monitor,
script, headless or automated browser; `Sec-Purpose: prefetch`; client hints naming HeadlessChrome) are answered 202 and
dropped, the media type (`text/plain`, `application/json` or none, else 415), the body (64 KB, 413), JSON (400), the key
(`X-Kestrel-Key`, the body's `k`/`key` or `?key=`: 401 when missing or unknown, 403 when disabled), the project's
`web_analytics` switch (403 when off), the key's allowed origins against `Origin`, else the `Referer`'s origin (a key
limited to origins needs one; 403), the key's bucket (`rate_limit_per_minute`, 429 with `Retry-After`). The event name
is `pageview` or a custom name (400 when it can't be one), the page URL is required (http or https, 400). Then one job
goes to the writer (429 when the queue is full) and the answer is `202 Accepted`, empty, `Cache-Control: no-store`, no
cookie. Web analytics is on by default for every project and can be switched off in the project's settings (Settings →
Web analytics); it uses the project's keys, the same as errors.

**What is kept** (`clean.rs`, `ua.rs`): the host (lowercase, no port) and the percent-decoded path without query (the
fragment's route in hash mode, without its query), at most 512 characters; `utm_source`, `utm_medium`, `utm_campaign`,
`utm_term`, `utm_content` (200 characters each); the referrer's domain without `www.` (nothing for a self-referral or a
non-web referrer); browser, OS (names only) and device class (desktop, mobile, tablet) from the user agent; the
screen-width bucket (`0-575`, `576-991`, `992-1439`, `1440-1919`, `1920+`); for custom events the name (100 characters)
and up to 20 properties as text (200 characters), without keys that look secret or name an IP address or user agent.
Paths, UTM values, names and properties have email addresses, IP addresses, phone numbers (9–15 digits written with `+`
or `00`, or in at least three groups), Luhn-valid card numbers and credentials replaced by `[Filtered]`. The visitor id
is `trunc64(sha256(salt ‖ project ‖ IP ‖ user agent ‖ host))` (`analytics::visitor_id`), with a salt that lives in
memory only (`DailySalt`) and changes when the UTC day does and at every start. The IP address and user agent are
used for that and for the browser, OS and device, then dropped.

**Storage and rollups.** Raw events go to `store.analytics_events` (one row each, Unix seconds, `source = 'web'`; app
signals will add `'app'`). Every minute (the retention task's tick) `rollup.rs` reads the events after
`store.analytics_rollup_state.last_id` in batches of 5,000, each batch one writer turn, and adds them to
`analytics_hours`, `analytics_days` and `analytics_breakdown` (per day, dimension and value: page, entry, referrer,
`utm_*`, browser, os, device, screen, event) in `kestrel.sqlite`: counts are added, visitors counted once per day (or
hour) and value through `store.analytics_seen`, which keeps the hashes of today and yesterday (UTC) and drops older
days. An entry page is a visitor's first pageview of the day. An event of an earlier day (*late*: the server was down,
the rollup fell behind) has its whole day rolled up again from the raw events, as long as the project's raw retention
still has the complete day; older ones aren't counted. Unique visitors over a range are the sum of the days'. After a
rollup the writer tells the Web tabs (`Writer::subscribe_web`).

**Retention** (daily job; Settings → Retention): raw events older than `retention_analytics_raw_days` (30, at most
400) and rollups older than `retention_analytics_days` (400) go; deleting a project takes both along.

**The Web tab** (`/projects/{id}/web`, `pages.rs`, `templates/web.html`, `partials/web_*.html`,
`static/analytics.css`): a period (today by hour; 7 or 30 days or a custom range of up to 400 days by day; `?period=`,
`from`, `to`), visitors, pageviews, views per visitor and custom events, a chart of visitors and pageviews, and cards
with tabs: top and entry pages; referrers and UTM sources, mediums and campaigns; browsers, operating systems, device
types, screen sizes; custom events. Clicking a value adds a filter (`?page=`, `referrer=`, `utm_*=`, `browser=`, `os=`,
`device=`, `screen=`); chips remove them. Without filters everything reads the rollups; with filters the raw events (so
at most the raw retention, which the page says when the period starts earlier). The chart is the design system's
line chart (`components::LineChart`, `m::line_chart`), the lists its bar list styles. `…/web/live` (the kit's
`http::live`, `analytics/live.rs`) streams `web-live` (current
visitors: distinct visitors in the last five minutes, from the raw events; after new events, at most every two seconds,
and every 30 seconds) and `web-updated` (the numbers and chart again, after a rollup, while the period includes today).
Before the first pageview the tab shows the snippet with an enabled key and how to use it. The overview shows the last
7 days once the project has data.

**API and MCP**: `GET /api/v1/projects/{id}/analytics/timeseries` and `…/breakdown?dimension=` (period, `from`, `to`,
filters as above, `limit`); MCP `analytics_summary` (totals, the period before, current visitors, the series, top 5 of
pages, referrers, UTM sources, browsers, devices and events) and `analytics_breakdown`. Counts only, never a visitor id.

**Tests**: `tests/analytics.rs`: the ingest contract (text/plain and JSON, the three ways to give the key, 202 with an
empty body and no cookie, the preflight, keys, the project switch, media types, sizes, origins via `Origin` and
`Referer`, bots and headless browsers, both rate limits), what a pageview keeps, visitor ids, rollups (incremental
uniques, entry pages, hours, ranges, the raw-event filters), late events, retention, the Web tab (empty and with data,
periods, filters, live stream, the switch, retention fields), the API and MCP, k.js (size, caching, ETag, ingest port)
and a privacy scan of every text and blob column of both databases after events with a known IP address, user agent,
email address, phone and card number. Unit tests cover parsing, scrubbing, user agents, the salt and the periods.

### Load check (web)

`tools/loadtest/web.py` (standard library only; open loop, k.js-style pageviews with 400 paths, 10 referrers, 12
user agents and a random client address each, 3 % custom events) against the release image limited to 2 CPUs and
2 GB, data on tmpfs, `KK_TRUST_PROXY=1` and `KK_INGEST_IP_RATE_LIMIT=0`:

| Rate | Accepted | p50 | p99 | Max |
|---|---|---|---|---|
| 1000 events/s for 20 s | all | 1.0 ms | 1.6 ms | 18 ms |
| 3000 events/s for 30 s | all | 0.8 ms | 1.6 ms | 25 ms |
| 3000 events/s for 120 s (two rollups during it) | all | 0.8 ms | 1.6 ms | 24 ms |
| 6000 events/s for 20 s | all | 0.8 ms | 1.4 ms | 31 ms |
| 10,000 events/s for 15 s | all | 0.7 ms | 1.5 ms | 58 ms |

All 741,000 events were rolled up (the rollup's totals equal the raw count) in batches that took 2–3 seconds a minute
at 3000 events/s; the container used about 40 MB at 3000/s and 220 MB with 741,000 raw events and 3.7 million seen-set
rows on tmpfs (every request a new visitor, the worst case for the seen-set). With that many raw events, a filtered
breakdown takes about 220 ms and the current-visitor count (410,000 visitors in five minutes) about 0.8 s; unfiltered
views read the rollups and stay fast.

## App analytics

docs/design.md §5; the user guide is [app-analytics.md](app-analytics.md). `src/analytics/app/` (`mod.rs`: the wire
format and what a signal keeps, `rollup.rs`, `query.rs`, `pages.rs`), `src/ingest/signals.rs`, `src/api/app.rs`,
`migrations/0007_app_analytics.sql` (the switches, the project secret, the rollups' new columns) and
`migrations/store/0003_app_signals.sql` (the app columns of the raw events, `app_users`).

**The protocol** is KestrelKit's (`docs/protocol.md` in the KestrelKit repository, v1 and its proposals P1–P13):
`POST /ingest/v1/signals` (also on the ingest port; exempt from the CSRF check) with the project's public key in
`X-Kestrel-Key`. In order: the per-IP bucket (429), the key (401 missing or unknown, 403 disabled), the project's
`app_analytics` switch (403 when off), the key's allowed origins when the request has an `Origin` (apps send none; 403),
a project secret (503 without), the key's bucket (`rate_limit_per_minute`, per request, 429), the body (256 KB, 413),
`Content-Encoding` (gzip, also deflate, br, zstd) undone with the same 256 KB cap (413; 400 when broken), JSON with a
`signals` array (400), at most 100 signals (413). Unknown fields are ignored; `sent_at` and `sdk` are optional. Each
signal on its own: an object with a valid `type` (1–100 characters from `[A-Za-z0-9._-]`, longer ones cut) and an
`install_id` (cut to 64 characters), else dropped; `time` corrected by `now − sent_at` when they are more than 5
seconds apart, the arrival time without a readable `time`, never in the future, dropped when older than the project's
raw retention. The text fields are cut to 200 characters without control characters; an unknown platform is kept as
sent; flags that aren't `true` are false; the payload keeps at most 20 string, number or boolean values (40-character
keys, 200-character values, scrubbed like web events: email addresses, phone and card numbers, credentials, IP
addresses), without keys that name an IP address or user agent. The answer is `202 {"accepted": n}` (with
`"dropped": m` when signals were dropped; `Ingest::dropped_signals` counts them), `Cache-Control: no-store`, no
cookie. The whole batch is one writer job (a full queue answers 503 with `Retry-After`).

**Users and sessions** (`analytics::app::Keys`): `trunc64(HMAC-SHA256(secret, "install" ‖ 0 ‖ install id))` and
the same with `"session"`, with the project's 32-byte `app_secret` (made by `randomblob(32)` in the migration and a
trigger on new projects; never read by any page or the API). Strict mode (`app_strict`) keys with
`HMAC(secret, "day" ‖ 0 ‖ day)` of the signal's UTC day. HMAC is written out in `app/mod.rs` (RFC 4231 vectors in its
tests) rather than adding a crate.

**Storage and rollups.** Raw signals go to `store.analytics_events` with `source = 'app'`: the type in `name`, the
user in `visitor`, the platform in `os`, the payload in `props`, and `bundle_id`, `app_version`, `build`,
`os_version`, `model`, `locale`, `flags` (1 debug, 2 TestFlight, 4 simulator) and `session` of their own. The rollup
(`rollup.rs`) reads both sources in the same id order and hands the app's to `app::rollup::add`: test signals are
skipped; the rest are added to `analytics_days`/`analytics_hours` with `source = 'app'` (signals in `events`, users in
`visitors`, `sessions`; and per day `wau` and `mau`) and to `analytics_breakdown` (dimensions `type`, `version`,
`build`, `release`, `platform`, `os`, `model`, `locale` with signals and users, and `payload` with signals per
`type ␟ key ␟ value`, at most 50 values per type, key and day, the rest as `\u{1}`). Hourly and per-value users and
daily sessions use the seen-set with their own codes (100–117). **Weekly and monthly users**: `store.app_users` holds
each day's users for 31 days. A day's row is made with the distinct users of the 6 and 29 days before; a user new to the
day (`INSERT OR IGNORE` into `app_users`) is looked up once (`MAX(day)` in the 29 days before, through
`idx_app_users_user`) and counts for the week and month when absent there; a late user of yesterday counts in today's
windows when today has no other day of theirs. Today's row is made at the first rollup of the day for any project with
users in the month before, so quiet days have their weekly and monthly users. Older days that get signals late are
rolled up again from the raw signals (`recompute_day`), and so are the windows of the 29 days after. In strict mode
the same arithmetic gives the sum of the days.

**Queries** (`query.rs`): without filters the rollups; with a version, platform or type filter, or with test signals,
the raw signals (30 days), where weekly and monthly users per day come from the distinct `(day, user)` pairs of the
period and the 29 days before. Release adoption (`release_adoption`) sums the release's daily users over the last 7
days and divides by the days' active users; `db::releases` adds it to every release (`adoption_users`).

**Pages** (`pages.rs`, `templates/app.html`, `app_explorer.html`, `partials/app_*.html`, `static/analytics.css`):
the App tab (`/projects/{id}/app`: period, the filter bar, daily/weekly/monthly users, sessions and signals, two
charts, version adoption, the lists, a test-signal note; live current users and numbers from `…/app/live`:
`app-live`, `app-updated`), the signal explorer (`/projects/{id}/app/signals?type=`), Settings → App analytics (the
switch and strict mode, `POST …/app/settings`, editors), the overview's app summary, and the adoption on the
Releases tab and release pages. Before the first signal the tab shows KestrelKit's setup with an enabled key.

**API and MCP**: `GET /api/v1/projects/{id}/analytics/app/timeseries`, `…/breakdown?dimension=`, `…/signal-types`,
`…/payload?type=`, `…/adoption` (period, `from`, `to`, `version`, `platform`, `type`, `test=1`, `limit`), and
`adoption_users_7d` on releases; MCP `analytics_summary` and `analytics_breakdown` with `source: app` (or an app
dimension, or a project with only app analytics). Counts only, never a user hash.

**Retention** is the web's (Settings → Retention): raw signals with `retention_analytics_raw_days`, rollups with
`retention_analytics_days`; deleting a project deletes its `app_users` too.

**Tests**: `tests/app_analytics.rs`: the protocol contract with KestrelKit's own golden requests
(`tests/fixtures/kestrelkit/`, copied from its `WireTests`: the encoder's exact bytes and the protocol document's
example), gzip, every answer (400, 401, 403, 413 for size, decompressed size and 101 signals, 429 for the key and the
IP, 503 for a full queue), lenient signals, unknown fields, clock skew, origins, the ingest port; a privacy scan of
every text and blob column of both databases for the install and session ids, the IP address, the user agent, an
email address, a phone and a card number; keyed and strict hashes; daily, weekly and monthly users checked against a
brute-force count over 45 days with churn, late signals for yesterday and an older day; hours, breakdowns, bounded
payloads, test signals and recomputation; retention and project deletion; the App tab (empty and with data, periods,
filters, test signals), the explorer, the settings; release adoption; the API, the reference and MCP. Unit tests in
`app/` cover HMAC, the golden batch, clock correction, payloads and KestrelKit's scrubbing cases.

### Load check (app signals)

`tools/loadtest/signals.py` (standard library only; open loop; batches of 50–100 signals built like KestrelKit's and
gzip-compressed, from 20,000 installs on five versions, 20 types, payloads on a third, 5 % TestFlight; `--setup` makes
the admin and a project on a fresh instance and checks the rollup afterwards) against the release image limited to 2
CPUs and 2 GB, data on tmpfs, `KK_TRUST_PROXY=1` and `KK_INGEST_IP_RATE_LIMIT=0`. Latency is per request (one batch):

| Rate | Accepted | p50 | p99 | Max |
|---|---|---|---|---|
| 3000 signals/s for 30 s | all | 5.0 ms | 7.5 ms | 15 ms |
| 3000 signals/s for 120 s (two rollups during it) | all | 5.0 ms | 7.3 ms | 16 ms |
| 6000 signals/s for 20 s | all | 4.4 ms | 6.6 ms | 16 ms |
| 10,000 signals/s for 15 s | all | 4.6 ms | 7.0 ms | 16 ms |
| 20,000 signals/s for 15 s | all | 4.3 ms | 6.6 ms | 17 ms |

Every accepted signal was stored, and the rollup counted every production signal (the TestFlight ones stay raw). A
minute's worth at 3000 signals/s (180,000 signals, 20,000 users) took 2.3–2.5 seconds to roll up; 300,000 took 4.1
seconds. The container used 55–110 MB. With 300,000 signals and 20,000 users in one day, the App tab, the overview and
the explorer read the rollups in about 50 ms; filtered views read the raw signals in one pass: 0.6–0.7 s for a version
or platform filter, 1.4 s with test signals included (every signal of the day).

**KestrelKit end to end**: KestrelKit's own test suite (56 tests) passes on Linux (`swift:6.2`), and a small Swift
program using the package (`Kestrel.configure`, `Kestrel.signal`, `await Kestrel.flush()`) sent its gzip batch
(KestrelKit's own deflate encoder) of 43 signals to a test Kestrel: all stored, `App.launched` sent by the SDK, the
email address in a payload filtered, the unknown platform `Linux` kept as sent.

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
(sign-in, users, tokens, Security, General, the layout, number, size and percentage formats, the provider forms)
come from vole-kit; a key may not be in both catalogs. The few texts the kit's pages take from the product
(`vole_kit::i18n::catalog::PRODUCT_KEYS`: the sign-in subtitle, the setup page's placeholders, what roles and
tokens may do, the setup-token hint naming `KK_SETUP_TOKEN`, the error page's link; `GENERAL_PRODUCT_KEYS`: the
General page's subtitle and what the public URL is used for) are at the end of each of Kestrel's files; the
`difs:write` scope's title and description are `difs.scope` and `difs.scope_help`.
