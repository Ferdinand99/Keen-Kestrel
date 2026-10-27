# App analytics with Keen Kestrel

Keen Kestrel counts how your iOS and macOS apps are used without telling you who uses them: the
[KestrelKit](https://github.com/Ferdinand99/KestrelKit) Swift package sends **signals** (`App.launched`,
`Paywall.shown`, `Export.finished` …), and the project's **App** tab shows daily, weekly and monthly active users,
sessions and signals over time, the signal types, version adoption, OS versions, device models and locales. Errors and
analytics share the same projects and keys, so one project holds an app's crashes (from the Sentry SDK) and its usage,
and each release shows both.

## 1. Add KestrelKit

Open the project's **App** tab: until the first signal it shows these steps with your Kestrel's address and one of the
project's enabled keys.

1. In Xcode, choose **File › Add Package Dependencies…** and enter `https://github.com/Ferdinand99/KestrelKit`.
2. Start it when your app launches:

   ```swift
   import KestrelKit

   Kestrel.configure(url: URL(string: "https://kestrel.example.com")!, key: "<public key>")
   ```

   `https://kestrel.example.com` is your Kestrel's address (`KK_PUBLIC_URL`); the key is the same public key as in
   the project's DSN (Settings → Keys). If Kestrel's admin interface isn't reachable from the internet, give ingest
   a port of its own with `KK_INGEST_PORT`: it serves `/ingest/v1/signals` too.
3. Send signals where something happens:

   ```swift
   Kestrel.signal("Paywall.shown", payload: ["plan": "pro"])
   ```

KestrelKit sends `App.launched` and `Session.started` by itself, so users and sessions are counted without any code.
It queues signals on the device, sends them in batches every 30 seconds and when the app goes to the background, and
works offline. Its [README](https://github.com/Ferdinand99/KestrelKit#readme) has the options (debug builds, test
mode, a default payload, the opt-out switch). Signals show up in Kestrel within a minute.

To use it from another SDK, send what KestrelKit sends: its
[`docs/protocol.md`](https://github.com/Ferdinand99/KestrelKit/blob/main/docs/protocol.md) describes
`POST /ingest/v1/signals`, and the API reference (`/api-docs`) has the endpoint too.

## 2. The App tab

- **Period**: today (by hour), the last 7 or 30 days, or a custom range of up to 400 days (by day). Days are UTC.
- **Filters**: a version, a platform, a signal type; and **Include test signals**. Clicking a version or platform in a
  list filters by it too, and chips remove filters. Filtered numbers come from the raw signals, which are kept 30
  days.
- **Numbers**: daily active users (on the period's last day), weekly and monthly active users (the distinct users in
  the 7 and 30 days up to that day), sessions and signals in the period, and the current users (seen in the last five
  minutes, updated live).
- **Charts**: active users per day (daily, weekly and monthly lines), sessions and signals; by hour for one day.
- **Version adoption**: each of the top five versions' share of the day's active users, over the period.
- **Lists**: signal types, versions and builds, OS versions, device models, platforms and locales, each with users
  (counted per day and added up over the period) and signals.

The **Signal explorer** (next to the dashboard) lists the signal types; pick one to see its signals and users over
time and its payload: each key with its most frequent values. It takes the same period and filters.

Test signals, those from debug builds, TestFlight and the simulator (KestrelKit marks them), are kept but left out of
the numbers until you include them, so testing doesn't inflate your users. The tab says how many there were.

The project overview shows the daily, weekly and monthly users and the current users once there are signals.

## 3. Releases: crashes next to adoption

Sentry's Apple SDK names a release `bundle@version+build` (`com.example.app@1.4.0+57`). Kestrel matches that to the
signals' bundle id, version and build, so the **Releases** tab and each release's page show the release's
**adoption**, its share of the app's daily active users in the last 7 days, next to its crash-free sessions and users
and the issues it brought. A release named without a build (`com.example.app@1.4.0`) counts every build of the
version. Releases without app signals show their share of the sessions, as before.

## 4. Privacy: what is kept

Kestrel sets no cookies and never stores IP addresses, user agents or the install id. For each signal it keeps:

| Kept | How |
|---|---|
| The type and payload | What your app passed: at most 20 keys of 40 characters, values of 200. Email addresses, phone numbers, card numbers, `Bearer`/`Basic` credentials and IP addresses in values become `[Filtered]` (KestrelKit does this on the device already; Kestrel does it again). Keys that name an IP address or a user agent are dropped. |
| The app | Bundle id, version, build. |
| The device | Platform, OS version, hardware model (such as `iPhone16,2`), locale (such as `nb_NO`), and the debug/TestFlight/simulator flags. Never the device's name. |
| A user | `HMAC-SHA256(project secret, install id)`, cut to 64 bits. KestrelKit's install id is a random UUID it makes once (never the IDFA or IDFV); the project's secret is random, made with the project and never shown, so the hash can't be turned back into the install id or matched across projects. |
| A session | The same keyed hash of KestrelKit's random session id. |
| The time | When the signal happened, corrected for the device's clock with the batch's `sent_at`, and never in the future. |

The IP address only feeds the per-IP rate limit in memory (an IPv6 address counts as its /64); the user agent isn't
read at all.

A signal's time is the device's word, so Kestrel keeps it only if it is at most 48 hours old once corrected with
`sent_at` (a device that was offline longer loses the signals older than that; later times count as now). Each
signal counts against the key's rate limit (1000 a minute unless the key has its own limit under **Settings →
Keys**), and when the server's disk is nearly full the endpoint answers 429 with `Retry-After: 60`.

**Strict mode** (Settings → App analytics) keys the hash with a key that changes every UTC day. A user then gets an
unrelated id every day, so nobody can be followed from one day to the next, and nor can anyone who gets hold of the
databases. Daily users stay exact; weekly and monthly users become the sum of the days (someone who used the app on
three days counts three times). Turning it on doesn't change what was already counted.

KestrelKit's own switch, `Kestrel.isEnabled`, lets your users opt out on the device: nothing is sent, and the queue
and the install id are deleted.

## 5. How long things are kept

Per project, in **Settings → Retention**: raw signals 30 days (1–400; the same setting as raw web events), the daily
and hourly counts the App tab shows 400 days. **Settings → App analytics** turns signals off for a project (KestrelKit
then stops sending until the next launch); what was counted stays.

## 6. API and MCP

The same numbers are in the REST API (`/api-docs`): `/api/v1/projects/{id}/analytics/app/timeseries`, `…/breakdown`,
`…/signal-types`, `…/payload` and `…/adoption`, with the period, filters and `test=1`, and each release's
`adoption_users_7d`. The MCP tools `analytics_summary` and `analytics_breakdown` answer for apps with `source: app`
(or on their own when the project only has app analytics). They give counts only, never a user hash.
