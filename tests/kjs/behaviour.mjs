// What k.js does in a browser, checked in Node with a small fake DOM (no
// dependencies). Runs every case against the readable source and against
// the served form (comment lines and indentation removed, the same rule as
// src/analytics/kjs.rs), so both behave alike.
//
//   node tests/kjs/behaviour.mjs [path or URL of k.js]   (default: src/analytics/k.js)
//
// docs/development.md has the docker command for the test server.

import { readFileSync } from 'node:fs';

const arg = process.argv[2] || new URL('../../src/analytics/k.js', import.meta.url).pathname;
const source = arg.startsWith('http') ? await (await fetch(arg)).text() : readFileSync(arg, 'utf8');

/** The served form: the rule in src/analytics/kjs.rs. */
function minify(src) {
  return src
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('//'))
    .join('\n') + '\n';
}

/** Runs k.js in a fake page and returns what it sent and the page's hooks. */
function page(code, opts = {}) {
  const sent = [];
  const loc = new URL(opts.href || 'https://shop.example/');
  const winL = {};
  const docL = {};
  const attrs = { 'data-key': '0123456789abcdef0123456789abcdef', ...(opts.attrs || {}) };
  if (opts.noKey) delete attrs['data-key'];
  const script = {
    src: 'https://kestrel.example.com/js/k.js',
    getAttribute: (n) => (n in attrs ? attrs[n] : null),
    hasAttribute: (n) => n in attrs,
  };
  const document = {
    currentScript: script,
    referrer: opts.referrer || '',
    visibilityState: opts.prerender ? 'prerender' : 'visible',
    addEventListener: (t, f) => (docL[t] ||= []).push(f),
    removeEventListener: (t, f) => (docL[t] = (docL[t] || []).filter((g) => g !== f)),
  };
  Object.defineProperty(document, 'cookie', {
    get: () => '',
    set: () => {
      throw new Error('k.js set a cookie');
    },
  });
  const record = (url, body, how) => sent.push({ url, how, body: JSON.parse(body) });
  const window = {
    addEventListener: (t, f) => (winL[t] ||= []).push(f),
    kestrel: opts.queue,
  };
  const fetchFn = (url, init) => {
    if (init.credentials !== 'omit' || init.keepalive !== true || init.method !== 'POST') {
      throw new Error('fetch options: ' + JSON.stringify(init));
    }
    if (init.headers) throw new Error('k.js must not set headers (that would need a preflight)');
    record(url, init.body, 'fetch');
    return Promise.resolve({});
  };
  if (!opts.noFetch) window.fetch = fetchFn;
  const navigator = {
    webdriver: !!opts.webdriver,
    globalPrivacyControl: opts.gpc,
    sendBeacon: (url, body) => {
      record(url, body, 'beacon');
      return true;
    },
  };
  const history = {
    pushState(_s, _t, u) {
      loc.href = new URL(u, loc.href).href;
    },
    replaceState(_s, _t, u) {
      loc.href = new URL(u, loc.href).href;
    },
  };
  const localStorage = {
    getItem: (k) => (opts.ignore && k === 'kestrel_ignore' ? 'true' : null),
    setItem: () => {
      throw new Error('k.js wrote to localStorage');
    },
  };
  const screen = { width: 1440 };
  const location = {
    get href() { return loc.href; },
    get pathname() { return loc.pathname; },
    get hash() { return loc.hash; },
    get hostname() { return loc.hostname; },
    get protocol() { return loc.protocol; },
  };
  window.history = history;
  const run = new Function(
    'window', 'document', 'location', 'history', 'navigator', 'screen', 'localStorage', 'fetch', 'URL',
    code,
  );
  run(window, document, location, history, navigator, screen, localStorage, opts.noFetch ? undefined : fetchFn, URL);
  return {
    sent,
    window,
    history,
    go(u) { loc.href = new URL(u, loc.href).href; },
    fire: (t) => (winL[t] || []).forEach((f) => f({})),
    show() {
      document.visibilityState = 'visible';
      (docL.visibilitychange || []).slice().forEach((f) => f({}));
    },
  };
}

let failures = 0;
let passed = 0;
function check(name, cond, detail) {
  if (cond) passed++;
  else {
    failures++;
    console.log('FAIL ' + name + (detail ? ': ' + JSON.stringify(detail) : ''));
  }
}

for (const [form, code] of [['source', source], ['served', minify(source)]]) {
  const t = (n) => `${form}: ${n}`;

  // A pageview on load, to the server k.js came from, without cookies.
  let p = page(code, { href: 'https://shop.example/pricing?utm_source=x', referrer: 'https://news.example/a' });
  check(t('one pageview on load'), p.sent.length === 1, p.sent);
  const first = p.sent[0] || { body: {} };
  check(t('endpoint'), first.url === 'https://kestrel.example.com/ingest/v1/event', first.url);
  check(t('payload'), first.body.k === '0123456789abcdef0123456789abcdef' && first.body.n === 'pageview'
    && first.body.u === 'https://shop.example/pricing?utm_source=x' && first.body.r === 'https://news.example/a'
    && first.body.w === 1440 && first.body.h === 0, first.body);
  check(t('fetch with keepalive, no credentials'), first.how === 'fetch');

  // Single-page apps: pushState, replaceState, popstate; the same path once.
  p.history.pushState({}, '', '/docs');
  check(t('pushState counts'), p.sent.length === 2 && p.sent[1].body.u === 'https://shop.example/docs', p.sent);
  p.history.pushState({}, '', '/docs?tab=2');
  check(t('same path, another query: no pageview'), p.sent.length === 2);
  p.history.replaceState({}, '', '/blog');
  check(t('replaceState counts'), p.sent.length === 3);
  p.go('/pricing');
  p.fire('popstate');
  check(t('popstate counts'), p.sent.length === 4 && p.sent[3].body.u.endsWith('/pricing'));
  p.go('/pricing#faq');
  p.fire('hashchange');
  check(t('hash changes are ignored without data-hash'), p.sent.length === 4);

  // Hash routing.
  p = page(code, { href: 'https://app.example/#/home', attrs: { 'data-hash': '' } });
  p.go('/#/settings');
  p.fire('hashchange');
  check(t('data-hash: hash changes count'), p.sent.length === 2 && p.sent[1].body.h === 1, p.sent);

  // Local development, automation, opt-out, exclusions, no key.
  check(t('localhost is ignored'), page(code, { href: 'http://localhost:3000/' }).sent.length === 0);
  check(t('127.0.0.1 is ignored'), page(code, { href: 'http://127.0.0.1:8080/' }).sent.length === 0);
  check(t('data-localhost sends'), page(code, { href: 'http://localhost:3000/', attrs: { 'data-localhost': '' } }).sent.length === 1);
  check(t('webdriver is ignored'), page(code, { webdriver: true }).sent.length === 0);
  // Global Privacy Control: nothing is sent, not even custom events.
  p = page(code, { gpc: true });
  p.window.kestrel('event', 'Signup', { plan: 'pro' });
  p.history.pushState({}, '', '/docs');
  check(t('Global Privacy Control sends nothing'), p.sent.length === 0, p.sent);
  check(t('globalPrivacyControl false sends'), page(code, { gpc: false }).sent.length === 1);
  check(t('globalPrivacyControl unknown sends'), page(code, {}).sent.length === 1);
  check(t('kestrel_ignore opts out'), page(code, { ignore: true }).sent.length === 0);
  check(t('no key, nothing sent'), page(code, { noKey: true }).sent.length === 0);
  const ex = { 'data-exclude': '/admin/*, /preview' };
  check(t('excluded wildcard'), page(code, { href: 'https://shop.example/admin/users', attrs: ex }).sent.length === 0);
  check(t('excluded exact'), page(code, { href: 'https://shop.example/preview', attrs: ex }).sent.length === 0);
  check(t('not excluded'), page(code, { href: 'https://shop.example/previews', attrs: ex }).sent.length === 1);

  // Custom events, also queued before k.js loaded.
  const queue = function () {};
  queue.q = [['event', 'Early', { a: 1 }]];
  p = page(code, { queue });
  check(t('queued events are sent'), p.sent.some((s) => s.body.n === 'Early' && s.body.p.a === 1), p.sent);
  p.window.kestrel('event', 'Signup', { plan: 'pro' });
  const ev = p.sent[p.sent.length - 1].body;
  check(t('custom event'), ev.n === 'Signup' && ev.p.plan === 'pro' && ev.u === 'https://shop.example/', ev);
  const before = p.sent.length;
  p.window.kestrel('event');
  p.window.kestrel('other', 'x');
  check(t('bad calls are ignored'), p.sent.length === before);

  // data-api, sendBeacon without fetch, prerendering.
  p = page(code, { attrs: { 'data-api': 'https://stats.example/ingest/v1/event' } });
  check(t('data-api'), p.sent[0].url === 'https://stats.example/ingest/v1/event');
  p = page(code, { noFetch: true });
  check(t('sendBeacon without fetch'), p.sent.length === 1 && p.sent[0].how === 'beacon');
  p = page(code, { prerender: true });
  check(t('nothing while prerendering'), p.sent.length === 0);
  p.show();
  check(t('a pageview once shown'), p.sent.length === 1);
}

console.log(`k.js behaviour: ${passed} passed, ${failures} failed (${source.length} bytes source)`);
process.exit(failures ? 1 : 0);
