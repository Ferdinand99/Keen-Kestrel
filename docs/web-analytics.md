# Web analytics with Keen Kestrel

Keen Kestrel counts your website's visitors and pageviews without cookies and without keeping IP addresses: a small
script, `k.js`, sends a pageview from every page, and the project's **Web** tab shows visitors and pageviews over
time, top pages, entry pages, referrers, UTM campaigns, browsers, operating systems, device types, screen sizes and
custom events. Errors and analytics share the same projects and keys, so a website can be one project for both.

## 1. Add the snippet

Open the project's **Web** tab: until the first pageview it shows the snippet with one of the project's enabled keys.
Paste it into the `<head>` of every page:

```html
<script defer src="https://kestrel.example.com/js/k.js" data-key="<public key>"></script>
```

`https://kestrel.example.com` is your Kestrel's address (`KK_PUBLIC_URL`); the key is the same public key as in the
project's DSN. Pageviews show up within a minute. If Kestrel's admin interface isn't reachable from the internet, give
ingest a port of its own with `KK_INGEST_PORT`: it serves `k.js` and `/ingest/v1/event` too.

Then, in **Settings → Keys**, set the key's **allowed origins** to your website (for example
`https://www.example.com`), so only your pages can send with it. Kestrel checks the page's `Origin` (or, failing that,
its `Referer`) against them.

`k.js` is under 2 KB gzip-compressed, has no dependencies, and is cached for a day.

### Options

| Attribute | What it does |
|---|---|
| `data-key` | The project's public key (required). |
| `data-api` | Where to send, if not to the server `k.js` came from (e.g. a proxy on your own domain): `https://stats.example.com/ingest/v1/event`. |
| `data-hash` | Count hash routes as pages: `/#/settings` and `/#/cart` are two pages. |
| `data-localhost` | Count visits from `localhost`, `127.0.0.1` and `file:` pages too (they are ignored otherwise). |
| `data-exclude` | Paths not to count, comma-separated; `*` matches anything: `data-exclude="/admin/*, /preview"`. |

To leave your own visits out, open your site, then the browser's console, and run
`localStorage.kestrel_ignore = 'true'` (and `localStorage.removeItem('kestrel_ignore')` to count them again). Browsers
driven by automation (`navigator.webdriver`) never send.

## 2. Single-page apps

Nothing to do: `k.js` sends a pageview whenever the path changes through `history.pushState`, `replaceState` or the
back and forward buttons. A change of only the query string doesn't count as a new page. Apps that route with
`#/…` add `data-hash`.

## 3. Custom events

Count things that aren't pages, like a sign-up or a download:

```js
kestrel('event', 'Signup', { plan: 'pro' })
```

The name (at most 100 characters) shows up under **Custom events** with how often it happened and how many visitors
did it. Properties (at most 20, as text of at most 200 characters) are kept with the raw event for later. To call
`kestrel()` before `k.js` has loaded, add this line before the snippet:

```html
<script>window.kestrel = window.kestrel || function () { (kestrel.q = kestrel.q || []).push(arguments) }</script>
```

## 4. The Web tab

- **Period**: today (by hour), the last 7 or 30 days, or a custom range of up to 400 days (by day). Days are UTC.
- **Numbers**: unique visitors, pageviews, views per visitor, custom events, and the current visitors (seen in the last
  five minutes, updated live).
- **Lists**: top pages and entry pages (the first page a visitor saw that day), referrers (the referring site's domain;
  "Direct / none" when there was none) and UTM sources, mediums and campaigns, browsers, operating systems, device
  types (desktop, mobile, tablet) and screen sizes, and custom events.
- **Filters**: click a value to see only the visits with it (a page, a referrer, a browser …); click more to combine
  them, and remove one with its chip. Filtered numbers come from the raw events, which are kept 30 days.

The overview shows visitors and pageviews of the last 7 days once there is data. The same numbers are in the REST API
(`/api/v1/projects/{id}/analytics/timeseries` and `…/breakdown`, see `/api-docs`) and the MCP tools
`analytics_summary` and `analytics_breakdown`.

## 5. Privacy: what is kept

Kestrel sets no cookies, uses no local storage for identifiers, and doesn't keep IP addresses or user agents. For each
pageview it keeps:

| Kept | How |
|---|---|
| The page | Host and path. The query string is dropped except `utm_source`, `utm_medium`, `utm_campaign`, `utm_term` and `utm_content`; the fragment is dropped unless `data-hash` is on. |
| The referrer | Only the referring site's domain (`news.example.com`), and nothing when it is your own site. |
| Browser, OS, device type | Read from the user agent when the pageview arrives; the user agent itself isn't kept. Names only, no versions. |
| Screen size | One of five width ranges, never the width. |
| A visitor id | `sha256(daily salt ‖ project ‖ IP address ‖ user agent ‖ host)`, cut to 64 bits. The salt is random, lives only in memory and is replaced at midnight UTC (and at every restart), so the id can't be turned back into an address, and the same browser gets an unrelated id the next day. |
| The time | When it arrived. |

Email addresses, phone numbers, card numbers, IP addresses and credentials in paths, UTM values and custom event names
and properties are replaced by `[Filtered]` before anything is written; properties named like secrets, IP addresses or
user agents are dropped.

Because visitor ids change every day, unique visitors are counted per day: over a week, someone who came on three days
counts three times. Kestrel can't follow anyone from one day to the next, and nor can anyone who gets hold of its
databases.

Kestrel doesn't act on the browser's "Do Not Track" or Global Privacy Control signals yet (nothing personal is kept
either way); this is an open question in the design.

Bots, crawlers, link previews, monitors, headless and automated browsers and prefetches are recognised by their user
agent and headers and not counted. They get the same answer as everyone else.

## 6. How long things are kept

Per project, in **Settings → Retention**: raw web events 30 days (1–400), the hourly and daily counts the Web tab shows
400 days. **Settings → Web analytics** turns pageviews off for a project; what was counted stays.
