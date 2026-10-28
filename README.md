<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/kestrel-logo-dark.svg">
    <img src="assets/brand/kestrel-logo-light.svg" alt="Keen Kestrel" width="420">
  </picture>
</p>

<p align="center"><strong>Self-hosted error tracking and privacy-friendly analytics in one small binary.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg" alt="License: AGPL-3.0"></a>
  <a href="https://hedgerow.tools/Keen-Kestrel/"><img src="https://img.shields.io/badge/website-hedgerow.tools%2FKeen--Kestrel-black.svg" alt="Website"></a>
</p>

> **Status: v0.2 "analytics", an early release (Docker image only).** Error tracking works end to end
> in 26 languages: the official Sentry SDKs send events, which are scrubbed of personal data, grouped into issues
> and stored; issue and event pages with search, live updates, stack traces and triage; iOS and macOS crashes
> symbolicated from dSYMs uploaded with `sentry-cli`; alerts for new issues, regressions, spikes and a release's
> first crash to ~40 notification channels (Slack, Discord, Teams, Telegram, ntfy, email, webhooks …); release
> health with crash-free sessions and users; and a REST API, OpenAPI document and MCP tools for all of it.
> **Web analytics:** a script under 2 KB, no cookies and no stored IP addresses, a Web
> tab with visitors, pageviews, pages, referrers, UTM, browsers and devices, filters and live current visitors, and it
> respects Global Privacy Control. **App analytics** with the
> [KestrelKit](https://github.com/Ferdinand99/KestrelKit) Swift package: an App tab with daily,
> weekly and monthly active users, sessions, signal types, version adoption, OS versions, devices and locales, a signal
> explorer for payloads, and each release's adoption next to its crash-free rate. Installers and Unraid/Proxmox
> templates come by v1.0. The translations are unreviewed drafts. **[Sending errors to Kestrel](docs/sending-errors.md)**
> shows how to point each SDK at it, upload dSYMs and set up alerts; **[Web analytics](docs/web-analytics.md)** shows
> how to add the snippet and what is kept; **[App analytics](docs/app-analytics.md)** does the same for KestrelKit.
> The design is in [`docs/design.md`](docs/design.md); to build and run it, see
> [`docs/development.md`](docs/development.md).

Keen Kestrel answers two questions about the apps and sites you run:

- **What broke?** Errors and crashes from the official Sentry SDKs (iOS/Swift, JavaScript, Rust, Python,
  Go and more) work unchanged. Issues are grouped, alerts go to the same ~40 channels (and the Hedgerow app) as
  [Vigilant Vole](https://github.com/Ferdinand99/Vigilant-Vole), and iOS crashes are symbolicated from the
  dSYMs that `sentry-cli` uploads.
- **How is it used?** App signals and website pageviews without cookies or stored IP addresses, with
  dashboards for users, versions, devices, pages and referrers.

One Rust binary and SQLite: no Kafka, ClickHouse, Redis or Postgres. It shares its design language and
foundations with Vigilant Vole and runs on Docker, Unraid, Proxmox or plain Linux.

## Install

Kestrel runs as one container with its data in a single folder:

```bash
mkdir keen-kestrel && cd keen-kestrel
curl -fsSLO https://raw.githubusercontent.com/Ferdinand99/Keen-Kestrel/main/docker-compose.yml
docker compose up -d
```

Then open `http://<host>:3002/setup` to create the first admin, create a project and copy its DSN. Until the
first admin exists, `/setup` asks for a setup token: set `KK_SETUP_TOKEN`, or read the one Kestrel makes up and
logs at the first start (`docker compose logs keen-kestrel`, a line with `/setup?token=…`). Scripted installs
that don't want the token set `KK_SETUP_OPEN=1`.
Set the address your apps use under **Settings → General** (public URL), or as `KK_PUBLIC_URL` in
`docker-compose.yml`, which is stored there at every start: it goes into the DSNs and alert links. Put Kestrel
behind HTTPS with `KK_TRUST_PROXY=1` when it's reachable from the internet. Images
are on GHCR for amd64 and arm64: `ghcr.io/ferdinand99/keen-kestrel:<version>` or `:latest`.

Back up the data folder as a whole. Notification channel secrets are sealed with the instance key in it
(`session.key`, since 0.6); a copy of the database without that file loses them, and they must be entered again.
Upgrading to 0.6 seals existing channels at the first start, after a plaintext copy in `backups/`.
The release after 0.7.0 (vole-kit 0.6) seals the two-factor secrets of your users the same way. The first start takes
a copy first (`backups/`, with a `.plaintext-secrets` marker next to it; it is deleted automatically once the database holds
no plaintext secret, or after 7 days). 0.7.0 can't open a database the new release has started on: restore that copy to
go back. Creating write or never-expiring API tokens, adding or promoting admins, an admin's password or two-factor
reset and deleting a passkey now ask for your password.
The release that adds the Hedgerow app channel renames Kestrel's internal slug from `kestrel` to `keen-kestrel` (the push
relay knows the products by their full names). It shows up as the MCP server name, the API reference download names
and the source or tag Keep, ClickSend and Opsgenie receive: update a rule that matched `kestrel`.

## Notifications in the Hedgerow app

The Hedgerow app on your iPhone can show Kestrel's alerts as push notifications, with no account and no other
service to run.

1. Under **Settings → Notifications**, add a channel of the kind **Hedgerow app**. It has no settings. Choose it in
   the alert rules of your projects like any other channel. Only one Hedgerow app channel can be active at a time.
2. In the Hedgerow app, open the connection to your server and turn on **Varsler** (notifications). The app
   registers itself with your server, and nothing is pushed to it before that.

Push is off until an admin adds the channel: with no active Hedgerow app channel the server refuses to register
devices. A read-only API token is enough for the app. **Send test** on the channel shows an error until at least one
device has registered. Alerts wait in the alert queue like for every channel, so a failed push is retried after 1, 5
and 15 minutes; a push that reached at least one device is never repeated.

What leaves your server is only a random reference. Kestrel keeps the text of each alert (“New issue in Shop: …”) for
24 hours and sends the Hedgerow push relay (`pigeon.hedgerow.tools`, or the one in `KIT_PUSH_RELAY_URL` / the
`push_relay_url` setting if you run your own) a push that carries that reference and the app's device handles,
nothing else. The relay and Apple therefore never see project names, errors or messages. When the push arrives, the
app fetches the text from your own server (`GET /api/v1/push/events/{ref}`) with its token and shows it. The server
registers itself with the relay the first time the channel fires, so it needs outbound HTTPS to the relay. The upgrade
to the release that adds the channel creates three empty tables (`push_devices`, `push_events`,
`push_registrations`) after the usual copy in `backups/`; the release before it refuses a database this one has
started on, so restore that copy to go back.

## Planned for v1

| Errors | Analytics |
|---|---|
| Sentry-compatible ingest (envelopes, sessions, feedback, attachments) | App signals via the `KestrelKit` Swift package |
| Issue grouping, search, resolve/ignore/merge, regressions | Web pageviews via a script under 2 KB |
| iOS/macOS symbolication from dSYMs (`sentry-cli` compatible) | Daily, weekly and monthly users, versions, devices, locales |
| Release health: crash-free sessions and users | Pages, referrers, UTM, browsers |
| Alerts: new issue, regression, spike, first crash in a release | No cookies, no stored IPs, daily-rotating salts |
| REST API, OpenAPI, MCP tools for coding agents | Same projects, keys and releases as errors |

Not in v1: performance tracing, session replay, JavaScript source maps (planned for v1.1), funnels and
cohorts. See [`docs/design.md`](docs/design.md) for the full scope and the phases.

## Security

The October 2026 security review (what was found, fixed and left open) is in [`docs/security-review-2026-10.md`](docs/security-review-2026-10.md).

## License

[GNU AGPL-3.0-only](LICENSE), except the web analytics snippet
[`src/analytics/k.js`](src/analytics/k.js), which is MIT so it can go on any website. The Swift SDK,
[KestrelKit](https://github.com/Ferdinand99/KestrelKit), is MIT too.
