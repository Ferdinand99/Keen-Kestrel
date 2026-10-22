# Development

Keen Kestrel is one Rust crate (`keen-kestrel`, binary `kestrel`) on top of
[vole-kit](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/docs/vole-kit.md), the shared kit that lives in the
Vigilant Vole repository. The kit brings sign-in, users, API tokens, Security, the API reference, i18n, the
database plumbing and the design system; Kestrel adds its own pages, API, migrations and strings, and the
Sentry-compatible ingest ([below](#ingest)).

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
  db/                 # migration set, projects and project_keys, issues (the writer's upsert)
  dsn.rs              # public address, DSNs, SDK snippets
  ingest/             # Sentry endpoints: keys, body decoding and limits, envelopes, rate limits, capture mode
  event/              # normalisation and privacy (scrub.rs), grouping (kestrel:v1), title, culprit, tags
  writer.rs           # the single batch writer
  store.rs            # store.sqlite: event payloads, attachments, sessions, feedback
  pages.rs            # project pages
  api.rs              # REST API v1 and its route table (API reference)
  mcp.rs              # MCP tools
templates/            # Kestrel's pages (extend kit/layout.html)
static/               # favicon, home-screen icons, web manifest
migrations/           # Kestrel's migrations (after the kit's core set); store/ has store.sqlite's own
locales/              # Kestrel's strings, 26 languages; glossaries in locales/glossary/
tests/                # in-process app tests, translation coverage, ingest and grouping
  fixtures/sentry/    # requests recorded from real SDKs, per SDK and version
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
| `--data-dir` | `KK_DATA_DIR` | `./data` | `kestrel.sqlite`, `store.sqlite`, `session.key`, `backups/` |
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
   Luhn-valid card numbers become `[Filtered]`. `user.email` and `user.username` become keyed hashes unless the
   project's `keep_user_details` is on (no setting in the interface yet). The user agent fills the browser and OS
   contexts and is then dropped. Session device ids are hashed with a key that changes daily.
5. **Grouping** (`src/event/grouping.rs`, configuration `kestrel:v1`): fingerprint (with `{{ default }}`),
   exception chain with in-app frames, exception type plus normalised value, message template, crashed thread.
   Native frames without symbols count as image and offset, and the issue is marked unsymbolicated. The
   components are stored in `issue_hashes`, so the issue page can explain them.
6. **The writer** (`src/writer.rs`): ingest puts the jobs on a bounded queue (2000 jobs; full means 429). One task
   writes them every 100 ms or 500 jobs in one IMMEDIATE transaction, on its own connection with `store.sqlite`
   attached: issues (new or updated, counts, first and last seen), releases and environments on first sight, tag
   values (the standard keys plus at most 20 custom ones per issue), the search index. A resolved issue that gets
   an event becomes `regressed`. At most 1000 payloads per issue and day are stored; the counts go on.

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
`locales/glossary/<code>.md`. `tests/i18n.rs` fails when a language misses a key, has an extra one, changes a
placeholder or lacks a plural form its language needs. Core strings (sign-in, users, tokens, Security, the layout)
come from vole-kit; a key may not be in both catalogs.
