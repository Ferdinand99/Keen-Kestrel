# Keen Kestrel – design

Status: approved design. Phases 1–7 are built and released as v0.1 "errors"; phase 8 (web analytics) and the
server side of phase 9 (app analytics) are built; the phase table in §11 has the state. This document is the reference for everyone building Kestrel, people and
agents alike; change it when a decision changes.

## 1. What and why

Keen Kestrel is a small, self-hosted tool for people who build and run their own apps and services. It
answers two questions in one place:

- **What broke?** Error and crash tracking that speaks the Sentry protocol, so the official Sentry SDKs
  (iOS/Swift, JavaScript, Rust, Python, Go, …) work unchanged. Crashes from iOS apps are symbolicated from
  uploaded dSYMs.
- **How is it used?** Privacy-friendly product analytics: signals from apps (TelemetryDeck-style) and
  pageviews from websites (Plausible-style), without cookies or stored IP addresses.

It belongs to the same family as [Vigilant Vole](https://github.com/Ferdinand99/Vigilant-Vole) (uptime
monitoring) and Nimble Newt (text editor): one Rust binary, SQLite, the same Apple-inspired design
language, 26 interface languages, easy to run on Docker, Unraid, Proxmox or bare Linux.

The first users are the owner's own projects (Sylo, Tofa, Subtrack, Vigilant Vole, Nimble Newt and their
websites): it has to be good enough to replace a hosted Sentry and a hosted analytics service for them.

## 2. Goals, non-goals, anti-goals

**Goals (v1)**
- Receive errors and crashes from the official Sentry SDKs; group them into issues; alert on new issues,
  regressions and spikes; show stack traces readably.
- Symbolicate iOS/macOS crashes from dSYMs uploaded with the official `sentry-cli` (Xcode build phase and
  fastlane keep working).
- Release health: crash-free sessions and users per release.
- App analytics (signals) and web analytics (pageviews) with dashboards and filters.
- One project = one app or site, with both error tracking and analytics.
- Runs comfortably on 2 vCPU / 2 GB with a single SQLite-backed binary.

**Non-goals (v1)**
- Performance monitoring, traces, spans, profiling, session replay, cron monitoring, logs, metrics.
- JavaScript source maps (planned for v1.1), Android/NDK minidumps, Apple `.ips`/MetricKit import.
- Funnels, retention cohorts, custom queries, geo-IP countries, live view.
- Multi-tenant SaaS features (organisations, billing, SSO). One instance serves one owner or team.

**Anti-goals (would be a failure even if it works)**
- Needing Kafka, ClickHouse, Redis, Postgres or several containers.
- Storing IP addresses, using cookies for analytics, or fingerprinting people across days without their
  app's own pseudonymous id.
- Breaking compatibility with the official SDKs or `sentry-cli` (users must never need a fork).
- A UI that looks like a different product than Vigilant Vole.

## 3. Shape of the system

```
SDKs (sentry-cocoa, sentry-javascript, sentry-rust, …)  ──►  /api/{project_id}/envelope/  ┐
sentry-cli (dSYM upload)                                ──►  /api/0/… chunk upload        │
KestrelKit (Swift), k.js (web)                          ──►  /ingest/v1/…                 │
                                                                                          ▼
                                           ingest  ─► bounded queue ─► single batch writer
                                                                       │
                       ┌───────────────────────────────────────────────┴──────────────┐
                       ▼                                                              ▼
           kestrel.sqlite (metadata, issues,                         store.sqlite (event payloads,
           releases, rollups, users, settings)                       raw analytics signals; disposable)
                       │
                       ▼
       web UI (askama + htmx + SSE) · REST API v1 + OpenAPI · MCP · alerts via vole-kit notify
```

- **Language and stack:** Rust (edition 2024), axum, askama, htmx + SSE, rusqlite with deadpool-sqlite,
  rustls. Static musl binary in an Alpine image, multi-arch (amd64, arm64).
- **Shared foundation:** the `vole-kit` crate, extracted from Vigilant Vole and living in the Vigilant Vole
  repository (Cargo workspace member `kit/`). Kestrel depends on it by git tag; locally a non-committed
  `.cargo/config.toml` `[patch]` points at `../Vigilant-Vole/kit`. vole-kit provides: core state and
  `KitConfig` (product name, slug, cookie and token prefixes, branding, nav), auth (sessions, 2FA, API
  tokens with scopes, CSRF, rate limiting, user/token pages), i18n engine and core catalogs, db pool and a
  migration runner with separate core and app migration sets, settings, security headers and CSP, static
  asset serving, the RouteDoc → OpenAPI / Markdown / llms.txt generator, the MCP server scaffold, the
  notification providers (generic `Alert`), and the design system (CSS tokens, `app.js`, icons, Inter,
  layouts and macros).
- **Kestrel identifiers:** slug `kestrel`, cookie `kk_session`, API token prefix `kk_`, user agent
  `KeenKestrel/<version>`, data directory `/data` in containers.

## 4. Errors module

### 4.1 Data model (`kestrel.sqlite`, app migrations)
- `projects` (integer id – SDK DSNs need a numeric project id), `project_keys` (public key, enabled,
  per-minute rate limit, allowed origins).
- `environments`, `releases` (version string, first/last seen, adoption from analytics).
- `issues`: project, title, culprit, level, platform, status `unresolved | resolved | ignored`, substatus
  `new | regressed | ongoing`, first/last seen, times seen, first/last release, `resolved_in_release`,
  ignore conditions, grouping config version.
- `issue_hashes` (grouping hash → issue; allows merging), `events` (small index row: event id, issue,
  timestamp, level, release, environment, pseudonymous user hash, payload reference),
  `issue_tag_values` (issue, key, value, count – whitelisted keys plus at most 20 custom keys),
  `issues_fts` (FTS5 over title and culprit), `dif_files` (debug files), `alert_rules`.

`store.sqlite` holds the normalised event payloads (JSON, zstd-compressed) and raw analytics signals. It
has its own small migration set, incremental auto-vacuum and no pre-migration backup: it can be thrown
away without losing issues, rollups or settings.

All writes go through **one writer task** fed by a bounded channel; it batches inserts in one IMMEDIATE
transaction every 100 ms or 500 items. A full queue makes ingest answer 429.

### 4.2 Ingest (Sentry protocol)
- `POST /api/{project_id}/envelope/` and the legacy `POST /api/{project_id}/store/`.
- Auth: `X-Sentry-Auth` header, `?sentry_key=` query parameter, or the envelope header `dsn`.
- Encodings: gzip, deflate, br, zstd. Decompression limits: 20 MB per envelope, 1 MB per event, 10 MB per
  attachment.
- Rate limiting: 429 with `Retry-After` and `X-Sentry-Rate-Limits` per category (the SDKs respect it).
- Envelope items:
  - **Kept:** `event`, `session` / `sessions` (release health), `user_report` / `feedback`, and
    `attachment` when it belongs to a kept event (screenshots and view hierarchy are shown inline).
  - **Accepted and dropped (200):** `transaction`, `span`, `profile`, `replay_*`, `check_in`,
    `statsd` / `metrics`, `log`, `client_report`, minidumps.
- Answer `{"id": "<event_id>"}`.

### 4.3 Normalisation and privacy
Fill defaults; trim breadcrumbs (100) and long strings; derive `in_app` (SDK value, else per-platform
rules); never derive an IP (`{{auto}}` is ignored); scrub `Authorization` and `Cookie` headers, keys
matching `pass|secret|token|auth|key|card` in extra data, contexts, breadcrumbs and request data, and card
numbers (Luhn); hash `user.email` and `user.username` unless the project opts in.

### 4.4 Grouping (versioned config, v1)
1. SDK `fingerprint` (with `{{ default }}` expanded) when present.
2. Otherwise the exception chain: exception type plus in-app frames, each contributing the normalised
   module or file basename and function (addresses, generics, closures and hash suffixes stripped; Swift
   demangled), at most the last 50 frames; all frames when none are in-app.
3. Exception without a stack: exception type plus the value with numbers, hex, UUIDs and quoted strings
   replaced.
4. No exception: the `logentry` message template; failing that, the crashed thread's stack.

Native frames are grouped after symbolication. Without a dSYM the fallback is image name plus offset and
the issue is marked "unsymbolicated". The grouping components are stored so the UI can explain why
events were grouped. Changing the algorithm means a new config version; old issues keep theirs. `kestrel:v2` (phase 7) adds one
rule: frames of the Sentry SDKs themselves (`SentrySDK…`, `SentryCrash…`, `-[Sentry…]`, `@sentry/…`, `sentry_sdk`, the
Rust crates) are never in-app. Each event is grouped by both versions; an event whose v2 hash is unknown joins the issue
its v1 hash points at, which keeps `kestrel:v1` and gets the v2 hash as well.

### 4.5 Issue lifecycle and search
- Resolve, resolve in the next release, ignore (forever, until a count, until a time), merge.
- **Regression:** a resolved issue that receives a new event (or one from a release newer than
  `resolved_in_release`) becomes `regressed` and raises an alert.
- Releases are created on first sight; `sentry-cli releases new/finalize` works against a stub.
- Search syntax: `is:unresolved level:error release:1.4 env:production os:iOS free text`.

### 4.6 Retention
Event payloads 30 days; at most 1000 stored events per issue per day (counters keep counting); issues are
removed 90 days after they were last seen; rollups 400 days; debug files for the last N releases or 180
days. All configurable per project.

### 4.7 Alerts
Per-project rules: new issue, regression, spike (events in one hour above
`max(threshold, k × 7-day hourly baseline)`), first crash in a new release. Per-issue cooldown and a
digest mode. Delivery through the vole-kit notification providers (the same ~40 as Vigilant Vole). Channels
are the instance's (Settings → Notifications), each with its message language; rules are evaluated after the writer
commits and messages are sent by a task of their own, with retries and a per-project history.

### 4.8 iOS / macOS symbolication
- getsentry `symbolic` (`debuginfo` with Mach-O and DWARF, `symcache`, `demangle` with Swift).
- Upload with the official `sentry-cli debug-files upload` (endpoints `GET /api/0/`,
  `/api/0/organizations/{org}/chunk-upload/`, `/api/0/projects/{org}/{project}/files/difs/assemble/`,
  legacy `/api/0/projects/{org}/{project}/files/dsyms/`), with any org/project slug and a Kestrel API
  token with the `difs:write` scope; also a plain `POST /api/v1/projects/{id}/dsyms` taking a zip.
- Each dSYM is converted once to a SymCache per debug id. Frames are mapped through
  `debug_meta.images` (uuid, `image_addr`), inlined frames are expanded and Swift is demangled. System
  frames keep the names sentry-cocoa resolved on the device.
- Rust and other languages that symbolicate in-process only need display (module, function, file:line,
  in-app highlighting).

### 4.9 Release health
`session` items are rolled up per project, release, environment and day: total, errored, crashed,
abnormal, and distinct crashed users. The SDK's `did` is hashed with the day's salt and discarded after
the rollup. This gives crash-free sessions and crash-free users per release.

## 5. Analytics module

### 5.1 Signals and pageviews
- **App signal:** `type` (e.g. `App.launched`, `Paywall.shown`), app version, build, OS and version,
  platform, device model, locale, debug/TestFlight/simulator flags, session id, and a payload of at most
  20 keys × 200 characters.
- **Web pageview:** hostname, path (query removed except `utm_*`), referrer domain, `utm_*`, screen-width
  bucket. Browser, OS and device class are derived from the user agent on the server; the user agent
  itself is not stored.

### 5.2 Privacy model
- No IP addresses stored and no cookies, ever.
- **Web visitors:** `trunc64(sha256(daily_salt ‖ project ‖ ip ‖ user_agent ‖ host))`. The salt lives only
  in memory and rotates at midnight UTC (the same approach as Vigilant Vole's status-page analytics).
  Unique visitors over a range are the sum of daily uniques.
- **App users:** the SDK creates a random install id (never IDFA or IDFV); the server stores
  `HMAC(project_secret, install_id)`, truncated. This is pseudonymous and stable, which makes daily,
  weekly and monthly active users possible. A strict per-project mode rotates it daily.
  As built (phase 9): `trunc64(HMAC-SHA256(secret, "install" ‖ 0 ‖ install_id))`, the session id likewise with
  `"session"`; the secret is 32 random bytes per project (`projects.app_secret`, made by a trigger for every new
  project), never shown or in the API. Strict mode keys with `HMAC(secret, "day" ‖ 0 ‖ UTC day)` of the signal's day.
  No IP address or user agent is read for app signals beyond the in-memory per-IP rate limit.
- Payload values that look like an email address, phone number or card number are scrubbed.

### 5.3 Ingest, rollups, dashboards
- `POST /ingest/v1/signals` (JSON batch of at most 100, key in `X-Kestrel-Key`);
  `POST /ingest/v1/event` for the web (accepts `text/plain` JSON from `sendBeacon`, no CORS preflight);
  `GET /js/k.js` (under 2 KB, single-page-app aware). Raw signals are kept 30 days.
- Rollups: hourly counts per signal type; daily counts and uniques per dimension (version, OS, device,
  locale, path, referrer, UTM, browser); daily active users. Kept 400 days.
- Web analytics (phase 8, as built): any enabled key of a project sends, its allowed origins checked against `Origin`
  or `Referer`; a per-project switch turns pageviews off. Raw events and a seen-set of today's and yesterday's visitor
  hashes are in `store.sqlite`; a minute's rollup adds counts and distinct visitors per hour, day and dimension (page,
  entry page, referrer, UTM source/medium/campaign, browser, OS, device class, screen bucket, custom event). Events of
  older days roll their day up again from the raw events. Dashboards read the rollups; filters combine dimensions,
  which one-dimensional rollups can't, so filtered views read the raw events (the raw retention, 30 days). Custom web
  events are named events with up to 20 properties, stored with the raw event. Global Privacy Control is respected
  (decided after phase 8): k.js sends nothing when `navigator.globalPrivacyControl` is true and ingest drops events
  with `Sec-GPC: 1` (still 202); `DNT` is ignored.
- App analytics (phase 9, as built): `POST /ingest/v1/signals` follows KestrelKit's `docs/protocol.md` v1 and its
  proposals (256 KB after decompression; lenient per signal, strict per batch; unknown fields ignored; 202/400/401/403/
  413/429/503 with `Retry-After` in seconds; clock skew corrected by `sent_at`; scrubbing on both sides). A separate
  per-project switch (`projects.app_analytics`) turns signals off, apart from web analytics'. Raw signals share
  `store.analytics_events` (`source = 'app'`, with app columns); one request's batch is one writer job. The minute's
  rollup adds them to the same `analytics_*` tables with `source = 'app'`: signals (`events`), daily users
  (`visitors`), sessions, and `wau`/`mau` per day; breakdowns by type, version, build, release
  (`bundle@version+build`), platform, OS version, model and locale, and payload values per type and key (at most 50
  per day, the rest as one). Weekly and monthly users are exact distinct counts: `store.app_users` keeps 31 days of
  daily user sets; a day's row starts from the distinct users of the days before (two counts per project and day),
  and each user new to the day is looked up once in the days before. Test signals (debug, TestFlight, simulator)
  stay raw and out of the rollups; the dashboard's "include test signals" toggle reads the raw signals like any other
  filter.
- Dashboards: apps (users, sessions, signals over time, top signal types, version adoption, OS, devices,
  locales) and web (visitors, pageviews, top pages, referrers, UTM, browsers, OS, devices), with one
  filter bar (date range, version, platform, signal type).

### 5.4 SDKs
- **KestrelKit** – Swift package for iOS and macOS, Swift 6, no dependencies: disk-backed offline queue,
  batches every 30 s and when the app moves to the background, default parameters, session ids, debug
  gate.
- **k.js** – the web snippet.
- Errors always use the official Sentry SDKs; Kestrel ships no error SDK of its own.

### 5.5 Errors and analytics together
Same project and keys. A Sentry release `bundle@version+build` matches an analytics app version + build,
so the release page shows adoption next to the crash-free rate and new issues, and the project overview
shows both modules. As built: adoption is the release's share of the app's daily active users over the last 7 days
(user-days); a release without `+build` matches every build of the version; releases without app signals keep the
share of the sessions.

## 6. Interface

All pages follow the Vigilant Vole design system (`docs/design-system.md` in the Vigilant Vole repository,
which also hosts vole-kit): dark-first, Inter, pill buttons, glass navigation, Apple-like spacing, light
and dark themes, keyboard and screen reader support, WCAG AA contrast. Brand accent: amber `#ff9f0a` (logo
in `assets/brand/`), used sparingly.

Pages: sign-in, setup, 2FA, users, API tokens, settings (vole-kit); projects; project overview (errors +
analytics); issues (search, bulk actions); issue (stack trace with an in-app toggle, breadcrumbs, tags,
events, why grouped, raw JSON, activity); event; releases and release detail (health and adoption); app
analytics; web analytics; signal explorer; debug files; alerts; project settings (DSN and copy-paste
setup snippets for Swift, JavaScript, Rust and Python, allowed origins, scrubbing, retention, rate
limits). Live updates with SSE and htmx.

**REST API v1** (documented with OpenAPI, `/api-docs.md` and `/llms.txt`): projects, keys, issues (list,
get, update status, merge, delete), issue events, events, releases and health, debug files, alert rules,
analytics time series, breakdowns and signal types.

**MCP tools:** `list_projects`, `list_issues`, `get_issue` (latest event with the stack trace formatted for
an LLM), `resolve_issue`, `ignore_issue`, `release_health`, `analytics_summary`, `analytics_breakdown`.

**Languages:** 26, with a test that keeps every language at 100 %; core strings come from vole-kit. Terms
are fixed per language in `locales/glossary/` (see Glossary below).

### Glossary

[`locales/glossary/`](../locales/glossary/) holds one file per language (the 25 translations; English is the
source) with the style rules and the words Kestrel uses for its own concepts: issue, event, release, crash,
signal, pageview and so on. Kestrel's strings are written from English with those words; core strings shared
with Vigilant Vole come from vole-kit and follow Vigilant Vole's glossaries. DSN, dSYM, SDK, `sentry-cli` and
"symbolication" stay in English everywhere; the [README](../locales/glossary/README.md) explains why.

## 7. Security

- DSN public keys are public by design and can only ingest. Legacy secret keys are accepted and ignored.
- Ingest routes are exempt from CSRF and answer CORS with `*` or the key's allowed origins.
- Rate limits: a token bucket per key and an in-memory bucket per IP (the IP is never written to disk),
  body size caps and decompression limits.
- Ingest can listen on a separate port, so the admin interface can stay on a LAN or VPN.
- Admin and API authentication, 2FA, token scopes, CSP and security headers come from vole-kit.

## 8. Performance targets (2 vCPU / 2 GB)

500 events/s and 3000 signals/s sustained; ingest p99 under 30 ms; issue list and dashboards under
200 ms with 1 million events and 50 million signals; under 150 MB RSS when idle; container image under
40 MB. Measured with a load harness modelled on Vigilant Vole's `tools/loadtest`.

## 9. Testing

- **SDK compatibility:** a capture mode (`KESTREL_CAPTURE_DIR`) records real requests; fixtures per SDK and
  version in `tests/fixtures/sentry/` (sentry-cocoa from an iOS test app, sentry-javascript via a
  browser, sentry-rust, sentry-python). Contract and grouping snapshot tests; a nightly job runs current
  SDKs and `sentry-cli` against the built image.
- **Symbolication:** a small Swift test app with known crash sites and its dSYM; assertions on function,
  file, line, inlining and demangling.
- **Privacy:** after ingest, scan every text and blob column of both databases for the test IP, user agent
  and raw install id; check that salts rotate, that no response sets a cookie, and that PII is scrubbed.
- **Upgrades:** fixture databases from every release, as in Vigilant Vole.

## 10. Delivery

Same pipeline as Vigilant Vole: release-please, static binaries for x86_64 and aarch64, multi-arch images
on GHCR, a Linux installer, Proxmox and Unraid templates, a GitHub Pages site in the family style.

## 11. Phases

| # | Phase | Done when |
|---|---|---|
| 1 | Spike: real sentry-cocoa crash, dSYM upload with sentry-cli, symbolication with `symbolic`, static musl build | A readable Swift stack from a real crash; image size known |
| 2 | Vigilant Vole: golden tests, carve shared code into `src/kit/`, `KitConfig` | VV released with no behaviour change |
| 3 | Cargo workspace with `kit/`, template sync, core migration set, generic `Alert` | `vole-kit` tagged; VV fully green |
| 4 | Kestrel skeleton: auth, users, tokens, settings, i18n, layout, projects and keys, Docker, CI | Sign-in and projects in the family design, 26 languages |
| 5 | Ingest, normalisation, grouping, issues and events UI, search, retention, writer | All SDK fixtures grouped correctly; load targets met |
| 6 | Debug file upload, SymCache, symbolication | Xcode build phase uploads; crashes symbolicated |
| 7 | Alerts, release health, releases, API and MCP for errors | **v0.1 "errors"** (released) |
| 8 | Analytics ingest, rollups, dashboards, `k.js`, privacy tests | Web analytics on the owner's sites (built) |
| 9 | KestrelKit, adoption in the owner's apps, linked release view, analytics API and MCP | **v0.2 "analytics"** (server side built: ingest, App tab, explorer, adoption, API, MCP; waits for KestrelKit's first release) |
| 10 | Hardening: load tests, site, installers, templates, translations | **v1.0** |

## 12. Risks

1. Swift demangling and `symbolic` in a static musl build – answered by the spike before anything else.
2. Regressions in Vigilant Vole while extracting vole-kit – golden tests first, a pure move step, and kit
   changes landing in the same repository as Vigilant Vole.
3. Sentry protocol drift – recorded fixtures and the nightly SDK matrix.
4. SQLite write contention and growth – a single batch writer, a separate disposable store, per-issue
   sampling, load tests in phase 5.
5. Grouping quality – versioned configuration and snapshot tests from the first day.

## 13. Later (v1.1 and beyond)

JavaScript source maps (artifact bundles), Apple `.ips` and MetricKit, minidumps, re-symbolication when a
dSYM arrives late, funnels and retention cohorts, countries from a user-provided GeoIP database, a Rust
signals crate, TelemetryDeck wire compatibility, performance and tracing.
