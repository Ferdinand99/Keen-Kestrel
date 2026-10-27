# Sending errors to Keen Kestrel

Keen Kestrel speaks the Sentry protocol, so the official Sentry SDKs send to it unchanged: you only give them
Kestrel's DSN instead of Sentry's. This page shows how for each SDK, how to get readable iOS and macOS crashes by
uploading dSYMs, how releases and crash-free rates work, and how to get alerts.

## 1. Create a project and copy its DSN

Sign in, open **Projects → New project**, give it a name and pick a platform. The project's **Overview** shows its
DSN and a setup snippet for Swift, JavaScript, Rust and Python; **Settings → Keys** has every key of the project.
A DSN looks like this:

```
https://0123456789abcdef0123456789abcdef@kestrel.example.com/1
```

The part before the `@` is the project's public key, the number at the end its id. A DSN can only send events, so
it is fine in an app. Set the public URL under **Settings → General** (or `KK_PUBLIC_URL` on the server, for example
`https://kestrel.example.com`) so DSNs and the links in alerts use the address your apps and people reach; without
it, Kestrel uses the address of each request.
If ingest should be reachable from the internet while the web interface stays on your LAN or VPN, give it a port
of its own with `KK_INGEST_PORT`.

## 2. Point the SDK at Kestrel

Set the **release** (the app's version) and the **environment** (`production`, `staging` …) in every SDK: issues,
regressions, release health and the "first crash in a release" alert all go by them.

### Swift: iOS and macOS (sentry-cocoa)

Add `https://github.com/getsentry/sentry-cocoa` with Xcode's **File → Add Package Dependencies…**, then:

```swift
import Sentry

SentrySDK.start { options in
    options.dsn = "https://<public key>@kestrel.example.com/1"
    options.environment = "production"
    // The release defaults to "<bundle id>@<version>+<build>", which Kestrel uses as is.
}
```

Sessions (for crash-free rates) are on by default. Crashes are sent the next time the app starts.

### JavaScript in the browser (@sentry/browser)

```js
import * as Sentry from "@sentry/browser";

Sentry.init({
  dsn: "https://<public key>@kestrel.example.com/1",
  release: "shop-web@1.4.0",
  environment: "production",
});
```

If the page is on another origin than Kestrel, the key's **Allowed origins** (Settings → Keys) may list the sites
that may use it; an empty list allows any.

### Node.js (@sentry/node)

```js
const Sentry = require("@sentry/node");

Sentry.init({
  dsn: "https://<public key>@kestrel.example.com/1",
  release: "shop-api@1.4.0",
  environment: "production",
});
```

### Python (sentry-sdk)

```python
import sentry_sdk

sentry_sdk.init(
    dsn="https://<public key>@kestrel.example.com/1",
    release="shop@1.4.0",
    environment="production",
)
```

### Rust (sentry)

```rust
let mut options = sentry::ClientOptions::default();
options.release = sentry::release_name!();
options.environment = Some("production".into());
options.attach_stacktrace = true;
let _guard = sentry::init(("https://<public key>@kestrel.example.com/1", options));
```

### Other SDKs

Any official Sentry SDK (Go, Java, .NET, PHP, Ruby, Dart …) works the same way: give it the DSN. Kestrel keeps
events, sessions, user feedback and the attachments of stored events; performance data (transactions, spans,
profiles), replays, check-ins, logs and metrics are accepted and dropped.

### Check that it arrives

Send a test event from the app (for example `SentrySDK.capture(message: "Hello from the app")`), or with `curl`:

```bash
printf '{}\n{"type":"event"}\n{"message":"Hello from curl","level":"warning"}\n' |
  curl -sS "https://kestrel.example.com/api/1/envelope/" \
    -H "X-Sentry-Auth: Sentry sentry_version=7, sentry_key=<public key>" --data-binary @-
```

It shows up in the project's **Issues** within a second.

### What Kestrel keeps

IP addresses are never stored, `{{auto}}` included. `Authorization` and `Cookie` headers, values of keys that look
like passwords, tokens or keys, and card numbers become `[Filtered]`. User ids, email addresses and user names
are replaced by keyed hashes. Paths from build machines (`/Users/<name>/…`, CI workspaces) lose the part with the
user name. Event details are kept 30 days and issues 90 days after they were last seen, which each project can
change under **Settings → Retention**.

### Limits

A project key is public (it ships in your app), so Kestrel limits what one key can send. A key accepts 1000 items
a minute (events, sessions, pageviews or signals) unless you set its own limit under **Settings → Keys**; a client
address (an IPv6 prefix counts as one) 6000. An envelope counts for each of its items. Over a limit Kestrel
answers 429, which the SDKs honour. An envelope may hold 100 items and 20 events; attachments are kept up to 1 MiB
each and 5 per event, sessions and user feedback up to 4 KiB each; the rest is accepted and dropped. An envelope
over the limits gets 413. When the server's disk is nearly full (under 1 GiB) or `KK_MAX_STORE_MB` is reached,
Kestrel still counts events but keeps no new details, attachments or feedback until retention has made room.

## 3. Readable iOS and macOS crashes: upload dSYMs

Release builds of Swift apps are stripped, so a crash only has addresses until Kestrel has the build's dSYM. Upload
it with the official `sentry-cli`; the project's **Debug files** page has the commands with this instance's address
and the project's slug filled in.

1. On the **Debug files** page, **Create an upload token** (or make one under **Settings → API tokens** with
   **Read and upload debug files**). It may only upload debug files and manage releases.
2. Upload the dSYMs after each build:

   ```bash
   export SENTRY_AUTH_TOKEN=kk_…
   sentry-cli --url https://kestrel.example.com debug-files upload --org any --project <project slug> --wait path/to/App.app.dSYM
   ```

   Any `--org` works. In Xcode, add a **Run Script** build phase with the same command (pointing at
   `"$DWARF_DSYM_FOLDER_PATH"`); fastlane's Sentry plugin works too with `url:` set to Kestrel. A zip of dSYMs can
   also be posted to `/api/v1/projects/<id>/dsyms`.
3. Crashes that arrive after the upload show functions, files and lines, inlined frames included, and are grouped
   by them. The Debug files page lists what was uploaded and which release last used each file.

## 4. Releases and crash-free rates

A release appears the first time an event or a session names it. You can also make it from a build pipeline with
sentry-cli, using the same token as for the dSYMs:

```bash
sentry-cli --url https://kestrel.example.com releases new --org any --project <project slug> shop@1.4.0
sentry-cli --url https://kestrel.example.com releases finalize --org any shop@1.4.0
```

`set-commits` and `deploys new` are accepted too, but Kestrel doesn't keep commits or deploys.

The project's **Releases** tab lists every release with its crash-free sessions and users, its share of the last
week's sessions (adoption), its new issues and events. A release's page has its health per day over 30 days, the
issues that started in it, the resolved issues it brought back, and the debug files its crashes used. Crash-free
rates come from the SDK's sessions: sentry-cocoa and the browser SDK send them by default; for servers, the Node
and Python SDKs count request sessions when release health is on.

## 5. Alerts

Alerts go to **notification channels**, which belong to the whole instance: Slack, Discord, Microsoft Teams,
Google Chat, Mattermost, Telegram, Signal, ntfy, Pushover, Gotify, email, PagerDuty, Opsgenie, webhooks and about
25 more.

1. **Settings → Notifications → Add channel** (admins): pick the service, fill in its form, choose the
   language its messages are written in, and press **Send test** to check it arrives. Saved secrets (tokens,
   webhook URLs, passwords) are never shown again.
2. In a project, open the **Alerts** tab and add a rule:
   - **New issue**: an issue is seen for the first time;
   - **Regression**: a resolved issue gets a new event;
   - **Spike**: an issue gets more events in an hour than a number you choose and than a multiple of its usual
     hourly rate over the past week;
   - **First crash in a release**: a release crashes for the first time.

   Choose its channels, and optionally an environment (only `production`, say). An issue sets a rule off at most
   once per cooldown (an hour by default); turn on **Send as a digest** to get one message every few minutes
   instead of one per alert.
3. The **Sent alerts** list on the same tab shows every alert, the channel it went to and whether it arrived.
   Deliveries that fail are tried again after 1, 5 and 15 minutes.

Messages say what happened and where ("New issue in Shop — ValueError: invalid literal for int()"), with the
project, level, release, environment and counts, and link to the issue when the public URL is set.

## 6. Automate it

Everything above has a REST API (`/api-docs` on your instance, also as OpenAPI and as Markdown for AI assistants)
and an MCP server at `/mcp` with tools to search issues, read an issue's stack trace, check release health and,
with a read-and-write token, resolve or ignore issues. Create tokens under **Settings → API tokens**.
