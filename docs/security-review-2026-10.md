# Security review, October 2026

Date: 27 October 2026. Covers Keen Kestrel up to the 0.8.1 release. The Swift SDK is reviewed separately in the
KestrelKit repository. Shared-library findings (vole-kit) are in the Vigilant Vole repository's review document.

## Method and limits

This was a read-only code review done by an AI reviewer. It was not an external audit and it was not done by a
professional security firm. The reviewer read the code, traced each candidate finding to the lines that cause it,
tried to refute it, and rated it. Each fix was written with tests and merged after review. Nothing was run against a
live instance: there was no dynamic testing, fuzzing or scanner run. The memory and CPU amplification figures in the
ingest findings are estimates from reading the code, not measurements.

Treat this as one careful review, not as a guarantee.

## Scope

`main` at `399280a` (the 0.7.0 line, vole-kit 0.5.0): the public ingest paths (envelopes, store, web events, app
signals), body decoding, grouping and scrubbing, the writer and retention, alerts and notification channels, the
debug-file (sentry-cli compatible) API, MCP, roles and authorization, and the privacy claims. How Keen Kestrel uses
vole-kit was read; the kit itself is covered elsewhere.

The main attacker considered is an anonymous internet client who knows a project's public DSN key, which ships in
every app and page by design. Secondary attackers are a signed-in viewer or editor and a holder of a leaked CI upload
token.

Stored XSS, SQL injection, zip-slip, chunk path handling, decompression bombs (size caps), CSRF on `/api/v1` and
role checks held up in review.

## Findings

Fixes shipped in 0.8.0 (#42, #44) and 0.8.1 (#45).

| ID | Severity | Title | Status |
|---|---|---|---|
| KK-01 | High | One envelope could carry unlimited items; parse before authentication | Fixed in #42 |
| KK-02 | High | No byte, item or storage budget on public ingest | Fixed in #42 |
| KK-03 | Medium | Unlimited issues and so unlimited alerts; forged events change issue state | Alert flood fixed in #42. Accepted: see below |
| KK-04 | Medium | Back-dated app signals force repeated day recomputes on the writer | Fixed in #42 |
| KK-05 | Medium | Unbounded permanent growth in `kestrel.sqlite` from anonymous input | Partly fixed in #42 (releases, environments, session strings). Open: distinct web-analytics breakdown values |
| KK-06 | Low-Medium | Pre-authentication buffering and decoder allocations | Fixed in #42 |
| KK-07 | Low-Medium | `difs:write` upload token not bound to a project | Partly fixed in #44. Open: see below |
| KK-08 | Low | Untrusted event text returned to MCP assistants | Fixed in #42 |
| KK-09 | Low | Event titles reach notification channels unneutralised | Partly fixed: channels are admin-only (#42), Discord mentions neutralised by vole-kit 0.6.0 (#44). Open: Markdown and links in other providers |
| KK-10 | Low | Privacy claims hold for keyed fields only | Open |
| KK-11 | Low | SSE streams have no cap and no lifetime | Fixed in #42 |
| KK-12 | Low | IP rate limit depends on `KK_TRUST_PROXY`; exact IPv6 keys | Partly fixed (IPv6 per /64 in #42; the kit warns about proxy headers without trust). Open: the proxy dependence itself |
| KK-13 | Low | Debug-file processing memory and time DoS by editors | Open |
| KK-14 | Info | No timeouts, connection cap or concurrency limit on the public listener | Open |
| KS-03 (server side) | Low | The server did not scrub shortened IPv6 addresses or card numbers grouped by dots or non-breaking spaces (from the KestrelKit review) | Fixed in #53 |
| Advisories | n/a | RUSTSEC-2026-0391, -0398, -0403 (zstd 0.13) | Accepted in #41 |

### What was changed

- **KK-01 and KK-02.** An envelope may hold at most 100 items and 20 events, and the first violation answers 413.
  The key is checked before the body is read. A key with no limit of its own gets 1000 items per minute; items are
  charged to the key and the client address (IPv6 per /64). Attachments are kept up to 1 MiB each and 5 per event;
  sessions, user reports and feedback are cut at 4 KiB. The writer queue is bounded in bytes (256 MiB) as well as by
  count. With under 1 GiB free on the data volume, or `store.sqlite` over `KK_MAX_STORE_MB` (default: unlimited),
  event details stop being stored and web events and signals answer 429.
- **KK-03.** A new-issue or regression rule sends at most 10 alerts per 10 minutes, with the rest collapsed into one
  "and N more" alert.
- **KK-04.** Signals older than 48 hours are dropped.
- **KK-05.** Releases that nothing refers to are removed after 90 days unseen; a project takes at most 1000 different
  releases and environments per day from sessions; session release and environment strings are length-capped.
- **KK-06.** zstd decoder window limited to 8 MiB, at most two content encodings.
- **KK-07.** `difs:write` tokens are bound to one project (or, for admins and discouraged, all projects) through a
  Keen Kestrel table. Existing tokens became "all projects" in the migration.
- **KK-08.** All MCP write tools are marked destructive; instructions and tool descriptions say event text is
  untrusted.
- **KK-11.** Issue, Web and App streams are capped (64 each) and end after one hour.

## Accepted and open items

- **Forged events and issue creation (KK-03), accepted.** Anyone with the DSN can send events, and a copied
  fingerprint can join a real issue or reopen a resolved one. This is inherent in Sentry-compatible anonymous ingest.
  What is limited is the cost: rate budgets, the alert cap and the digest. "Allowed origins" is a deterrent for
  browsers only. Per-project caps on new issues were suggested and not built.
- **Tokens on the generic API tokens page (KK-07), open.** The kit's token model has no product field, so a
  `difs:write` token made under Settings, API tokens is bound to no project and is refused (403, with a message
  pointing to the Debug files page). Binding by the kit is not built.
- **Chunk upload is not project-scoped (KK-07), open.** The chunk endpoint is organisation-level and
  content-addressed by SHA-1, so a bound token can still store chunks. The assemble, upload and release routes do check the
  token's project. There is no per-project quota on debug-file bytes or chunks.
- **KK-10, privacy claims, open.** The statement "IP addresses are never stored" is true for IP-named keys and headers.
  The error path scrubs credentials and card numbers in free text but not IP literals or email addresses in exception
  messages, breadcrumbs, tags or URLs. Attachments, feedback names and some session fields are stored unscrubbed (and
  are not displayed anywhere). `KESTREL_CAPTURE_DIR` writes full headers and bodies and is meant for tests only. The
  keyed-hash key for user ids lives in the same database as the hashes. Fix options are rewording the docs or adding a
  scrub option.
- **KK-12, proxy configuration, open.** Behind a proxy without `KK_TRUST_PROXY` all clients share one address, so one
  noisy client can use up the shared budget for everyone; with it on and the port also reachable directly,
  `X-Forwarded-For` is attacker-chosen.
- **KK-13, debug-file processing, open.** An editor or a token holder can upload crafted large objects (up to 1 GB)
  that are converted in memory, and processing is serialised behind one lock. Not reachable by anonymous clients.
- **KK-14, listener limits, open.** No request body timeout, connection cap or concurrency limit in the application.
  The usual reverse proxy provides these; they matter when the port is published directly. Whether hyper's defaults
  are enough was not verified.
- **KK-05 remainder.** Web-analytics breakdown rows (paths, referrers, UTM values, event names) have no cap on
  distinct values per day, unlike app payload values.
- **Per-project storage budget.** The disk watermark is instance-wide; there is no per-project quota.
- **Dependencies.** The three zstd advisories only reach `symbolic-debuginfo`, whose single zstd call is bulk
  decompression of ELF sections; none of the affected code paths is used. The ignores are documented in `deny.toml`
  and should be removed when symbolic moves to zstd 0.14.

## Not covered

- vole-kit internals (see the Vigilant Vole review), including the kit's MCP authentication, CSRF globs for
  `/api/*/envelope` and security headers as they apply to ingest and `k.js`.
- Dependency analysis beyond the `deny.toml` notes: no check of advisories for the `symbolic-*`, `zip`, `brotli`,
  `zstd`, `flate2`, `multer` and `askama` crates.
- CI/CD and release pipeline, the Pages site, the Dockerfile beyond the entrypoint, and any particular deployment
  (firewall, proxy configuration, volume layout).
- Alert delivery details beyond the trigger path, bot detection in `ua.rs`, frame rendering, path reduction and
  grouping correctness, translations.
- The KestrelKit SDK (separate review) and `k.js` behaviour in real browsers.
- Dynamic behaviour: no request was sent, nothing was built or fuzzed. The recommended load tests (a small gzip of
  600,000 tiny events; a loop of 10 MB attachments) were not run.

## Notes for operators

Release notes: [0.8.0](https://github.com/Ferdinand99/Keen-Kestrel/releases/tag/v0.8.0) and
[0.8.1](https://github.com/Ferdinand99/Keen-Kestrel/releases/tag/v0.8.1).

- **Rate limit default.** A key without its own limit is limited to 1000 items per minute. A busy site or app above
  that must set a limit on its key (up to 1,000,000). The default is shown in the project settings.
- **Envelope and item limits.** Envelopes with more than 100 items or 20 events get 413. Attachments over 1 MiB or
  beyond 5 per event, and sessions and feedback over 4 KiB, are dropped. Signals older than 48 hours are dropped.
- **Disk pressure.** With under 1 GiB free (or over `KK_MAX_STORE_MB`) event details stop being stored.
- **Notification channels are admin-only.** Editors can no longer add, change, test or delete them.
- **Debug-file tokens** are bound to a project. Existing tokens became "all projects". A `difs:write` token made on
  the generic API tokens page is refused until recreated on the Debug files page.
- **App payload keys** containing `pass`, `secret`, `token`, `auth`, `key`, `card`, `cookie` or `cvv` (or `ssn` as a
  word) are dropped, for example `api_key` and `author`. Control characters are removed before scrubbing, so values
  split by a tab or newline are caught. Since #53 the server also scrubs shortened IPv6 addresses and card numbers
  grouped by dots or non-breaking spaces, as KestrelKit does on the device (KS-03).
- **vole-kit 0.6.0 (0.8.0).** A fresh instance asks for a setup token at `/setup` (`KK_SETUP_TOKEN`, or the random one
  logged at first start; `KK_SETUP_OPEN=1` opts out). Two-factor secrets are sealed at rest; the first start takes a
  pre-migration copy, and going back to 0.7.0 needs it. Creating write or never-expiring tokens on the kit's token
  page, adding or promoting admins, resetting another user's password or two-factor sign-in, and deleting a passkey
  ask for the password. Notification redirects to other hosts and link-local or metadata destinations stop working.
  Cookies are `Secure` when the public URL is https. Other devices are signed out after 2FA or passkey changes.
