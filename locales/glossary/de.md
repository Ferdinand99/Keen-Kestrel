# Keen Kestrel – glossary (German, `de`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/de.md).

## Stil
- Anrede: informelles „du“ (Imperative wie „Gib einen Namen ein“), nie „Sie“. Ton: knapp, ruhig, Apple-Stil.
- Anführungszeichen: „…“ und ‚…‘; Apostroph ’ nur wo im Deutschen vorhanden.
- Großschreibung nach deutscher Norm (Substantive groß, Labels im Satzstil).
- Lange Komposita auf Buttons, Tabs und Pills werden gekürzt.
- Plural: `.one` (genau 1) und `.other` (alles andere, auch 0).
- Fachbegriffe bleiben englisch: DSN, dSYM, SDK, `sentry-cli`, Symbolication, JSON, UTM, Webhook.

## Terms

| English | Deutsch | Note |
|---|---|---|
| issue | Problem (das; Pl. Probleme) | as in Crashlytics; "Issue" is common among developers but clumsy in sentences |
| error | Fehler (der) | VV: „Fehler“ |
| event | Ereignis (das; Pl. Ereignisse) | VV: „Ereignisse“ |
| release | Release (das; Pl. Releases) | the version string `bundle@version+build`; "Version" is the app version in analytics |
| app version | Version | VV: „Version“ |
| environment | Umgebung (die) | |
| resolve / resolved | als behoben markieren / behoben | VV: „Behoben“; button short form „Behoben“ |
| resolve in next release | im nächsten Release beheben | |
| unresolved | offen | shorter than „ungelöst“; filter tab „Offen“ |
| ignore / ignored | ignorieren / ignoriert | VV: „Ignoriert“ |
| merge (issues) | zusammenführen | VV: „Zusammenführen“ |
| regression / regressed | Regression / wieder aufgetreten | pill „Wieder aufgetreten“ (short: „Wieder offen“) |
| new / ongoing | neu / andauernd | issue substatus |
| crash | Absturz (der; Pl. Abstürze) | verb: abstürzen |
| crash-free sessions / users | absturzfreie Sitzungen / absturzfreie Nutzer | short label „Absturzfrei“ |
| release health | Release-Stabilität | literal „Gesundheit“ reads oddly; tab „Stabilität“ |
| stack trace | Stacktrace (der) | |
| frame / in-app frame | Frame (der) / App-Frame | toggle „Nur App-Code“ |
| breadcrumbs | Breadcrumbs (Pl.) | Sentry term kept; help text: „die Schritte vor dem Fehler“ |
| grouping | Gruppierung | „Warum gruppiert“ for "why grouped" |
| fingerprint | Fingerprint (der) | as in „SSH-Fingerprint“; the SDK field `fingerprint` stays as code |
| debug files | Debug-Dateien | |
| symbolicated / unsymbolicated | symbolisiert / nicht symbolisiert | status labels; the noun stays „Symbolication“ |
| project | Projekt (das) | |
| key (project key) | Schlüssel (der) | „Projektschlüssel“ where it must be specific |
| alert / alert rule | Alarm / Alarmregel | VV: „Alarme“ |
| spike | sprunghafter Anstieg | alert type; short „Anstieg“; not „Spitze“ (peak in charts) |
| first seen / last seen | zuerst gesehen / zuletzt gesehen | |
| user (of the app) | Nutzer (der) | deliberately not „Benutzer“, which VV uses for accounts on the instance |
| active users (daily / weekly / monthly) | aktive Nutzer (täglich / wöchentlich / monatlich) | avoid DAU/WAU/MAU in the UI |
| signal | Signal (das; Pl. Signale) | analytics event from an app |
| pageview | Seitenaufruf (der) | VV: „Seitenaufrufe“; short „Aufrufe“ |
| visitor / unique visitors | Besucher / eindeutige Besucher | VV |
| referrer | Referrer (der) | VV: „Top-Referrer“ |
| session | Sitzung (die) | VV uses „Sitzung“ for sign-in sessions too |
| retention (data) | Aufbewahrung | VV; settings label „Aufbewahren für“; never for cohort retention (not in v1) |
| sample / sampling | Stichprobe / Sampling | sample rate: „Sample-Rate“ |
| tag | Tag (das) | VV: neuter, „das Tag“ |
| analytics | Analyse | VV: „Analyse“ |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; „der DSN“, „das dSYM“, „das SDK“ |
| symbolication | Symbolication (die) | unchanged noun; prefer „symbolisiert“ phrasing in sentences |
