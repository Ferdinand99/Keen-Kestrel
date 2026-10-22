// k.js: Keen Kestrel's web analytics snippet (docs/web-analytics.md).
//
//   <script defer src="https://kestrel.example.com/js/k.js" data-key="<public key>"></script>
//
// Sends a pageview when the page loads and when a single-page app moves to
// another path (history.pushState, replaceState, popstate; hash changes with
// data-hash). No cookies, no storage, no identifiers: Kestrel tells visitors
// apart for one day with a salted hash it makes on the server.
//
// Attributes: data-key (required), data-api (where to send; default: the
// server k.js came from), data-hash (hash routes count as pages),
// data-localhost (send from localhost and 127.0.0.1 too), data-exclude
// (comma-separated paths not to count; * matches anything).
// Custom events: kestrel('event', 'Signup', {plan: 'pro'}). Calls made
// before k.js loads are queued by: window.kestrel = window.kestrel ||
// function () { (kestrel.q = kestrel.q || []).push(arguments) }.
// localStorage.kestrel_ignore = 'true' stops this browser from counting, and
// so does Global Privacy Control (navigator.globalPrivacyControl).
//
// Served with comment lines and indentation removed (src/analytics/kjs.rs):
// no comments after code on a line, no multi-line strings.
(function () {
  'use strict';
  var w = window, d = document, l = location, s = d.currentScript;
  if (!s) return;
  var key = s.getAttribute('data-key');
  var api = s.getAttribute('data-api') || new URL(s.src).origin + '/ingest/v1/event';
  var hash = s.hasAttribute('data-hash');
  var local = s.hasAttribute('data-localhost');
  var ex = (s.getAttribute('data-exclude') || '').split(',').map(function (p) {
    p = p.trim();
    return p && new RegExp('^' + p.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*') + '$');
  }).filter(Boolean);
  var last;

  function off() {
    var n = navigator;
    if (!key || n.globalPrivacyControl === true || n.webdriver || w._phantom || w.__nightmare || w.Cypress) return 1;
    if (!local && (/^(localhost|127(\.\d+){3}|\[::1\]|0\.0\.0\.0)$/.test(l.hostname) || l.protocol == 'file:')) return 1;
    try {
      if (localStorage.getItem('kestrel_ignore') == 'true') return 1;
    } catch (e) {}
    return ex.some(function (re) {
      return re.test(l.pathname);
    });
  }

  function send(n, p) {
    if (off()) return;
    var b = JSON.stringify({k: key, n: n, u: l.href, r: d.referrer || null, w: screen.width, h: hash ? 1 : 0, p: p});
    if (w.fetch) {
      fetch(api, {method: 'POST', body: b, keepalive: true, credentials: 'omit'}).catch(function () {});
    } else if (navigator.sendBeacon) {
      navigator.sendBeacon(api, b);
    }
  }

  function page() {
    var p = l.pathname + (hash ? l.hash : '');
    if (p === last) return;
    last = p;
    send('pageview');
  }

  ['pushState', 'replaceState'].forEach(function (m) {
    var f = history[m];
    if (f) {
      history[m] = function () {
        var r = f.apply(this, arguments);
        page();
        return r;
      };
    }
  });
  w.addEventListener('popstate', page);
  if (hash) w.addEventListener('hashchange', page);

  var q = (w.kestrel && w.kestrel.q) || [];
  w.kestrel = function (c, n, p) {
    if (c == 'event' && n) send(String(n), p && typeof p == 'object' ? p : undefined);
  };
  q.forEach(function (a) {
    w.kestrel.apply(null, a);
  });

  if (d.visibilityState == 'prerender') {
    d.addEventListener('visibilitychange', function v() {
      if (d.visibilityState == 'prerender') return;
      d.removeEventListener('visibilitychange', v);
      page();
    });
  } else {
    page();
  }
})();
