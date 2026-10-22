# Keen Kestrel – glossary (Hungarian, `hu`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/hu.md).

## Style
- Register: impersonal / infinitive, no "te", no stiff formal address. Buttons: noun forms as in common Hungarian
  software ("Mentés", "Törlés"); "Adja meg…" for prompts.
- Quotation marks: „ ” (low-high). Keep `…`; dashes as in English (— –).
- Sentence case, lowercase month and weekday names. Hungarian nouns stay singular after numerals ("3 probléma"), so
  plural keys have only `one` and `other`, both singular.
- Avoid suffixes on `{placeholders}`; rephrase.
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, and other product, protocol and
  provider names.

## Terms

| English | Magyar | Note |
|---|---|---|
| issue | probléma | as in Crashlytics; not "hiba", which is the error itself |
| error | hiba | VV: „Hiba” |
| event | esemény | VV: „Események” |
| release | verzió | same word as app version: in Kestrel a release *is* the app version |
| app version | verzió | VV |
| environment | környezet | |
| resolve / resolved | megoldottnak jelöl / megoldva | VV: „Megoldva”; button „Megoldva” |
| resolve in next release | megoldva a következő verzióban | |
| unresolved | megoldatlan | filter tab „Megoldatlan” |
| ignore / ignored | figyelmen kívül hagyás / figyelmen kívül hagyva | VV; short pill „Mellőzve” if space is tight |
| merge (issues) | egyesítés | VV: „Egyesítés” |
| regression / regressed | regresszió / újranyitva | pill „Újranyitva” |
| new / ongoing | új / folyamatban | issue substatus |
| crash | összeomlás | verb: összeomlik |
| crash-free sessions / users | összeomlásmentes munkamenetek / felhasználók | short label „Összeomlásmentes” |
| release health | stabilitás | tab „Stabilitás”, heading „Verziók stabilitása” |
| stack trace | veremkiírat | "hívási verem" (call stack) is also understood; use one form only |
| frame / in-app frame | keret / alkalmazáskeret | toggle „Csak az alkalmazás kódja” |
| breadcrumbs | morzsák | from „morzsamenü”; help text: „a hibát megelőző lépések” |
| grouping | csoportosítás | „Miért lett csoportosítva” |
| fingerprint | ujjlenyomat | the SDK field `fingerprint` stays as code |
| debug files | hibakeresési fájlok | |
| symbolicated / unsymbolicated | szimbolizált / nem szimbolizált | status labels; the noun stays „symbolication” |
| project | projekt | |
| key (project key) | kulcs | „projektkulcs” where it must be specific |
| alert / alert rule | riasztás / riasztási szabály | VV: „Riasztások” |
| spike | hirtelen növekedés | alert type; short „Kiugrás” |
| first seen / last seen | első előfordulás / utolsó előfordulás | short „Első” / „Utolsó” in table headers |
| user (of the app) | felhasználó | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktív felhasználók (napi / heti / havi) | avoid DAU/WAU/MAU in the UI |
| signal | jelzés | analytics event from an app |
| pageview | oldalmegtekintés | VV: „Oldalmegtekintések”; short „Megtekintések” |
| visitor / unique visitors | látogató / egyedi látogatók | VV |
| referrer | hivatkozó oldal | VV |
| session | munkamenet | VV: „Munkamenetek” |
| retention (data) | megőrzés | VV; never for cohort retention (not in v1) |
| sample / sampling | minta / mintavételezés | sample rate: „mintavételi arány” |
| tag | címke | VV: „Címkék” |
| analytics | analitika | VV: „Analitika” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; suffixes with a hyphen only if unavoidable („DSN-t”), better rephrase |
| symbolication | symbolication | unchanged noun; prefer „szimbolizált” phrasing in sentences |
| notification channel / channel | értesítési csatorna / csatorna | VV: „értesítési csatorna”; short „Csatorna” in tables and buttons |
| digest | összesítő | „Küldés összesítőként”; status pill „Összesítőben” |
| cooldown (at most every … per issue) | „problémánként legfeljebb … időnként” | label „Problémánként legfeljebb ennyi időnként”, then the duration |
| threshold / alert above | küszöb / riasztási küszöb | form label „Több mint … esemény óránként” |
| multiplier (× the usual) | a szokásosnál …× több | „…-szoros” would need a suffix on the number; avoid |
| adoption | elterjedtség | the release's share of sessions in the last 7 days |
| errored (session) | hibával, összeomlás nélkül | |
| abnormal (session) | rendellenesen ért véget | |
| finalized (release) | véglegesítve | lowercase status after a date |
| culprit (alert message field) | hibaforrás | as in the issue search help |
| attempt (delivery) | próbálkozás | „{n} próbálkozás” (delivery attempts) |
| header (webhook) | fejléc | VV kit: „Egyéni fejlécek” |
