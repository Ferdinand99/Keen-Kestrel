# Keen Kestrel – glossary (Danish, `da`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/da.md).

## Stil
- Tiltale: uformelt "du", aldrig "De". Naturlig imperativ: "Angiv et navn", "Vælg et projekt". Kortfattet, roligt tonefald.
- Citationstegn: »…« (dansk stil), indeni ›…‹.
- Store/små bogstaver: kun første ord med stort (sætningsstil); månedsnavne, ugedage og sprognavne med småt.
- Tal: `.one` for præcis 1, `.other` for alt andet (også 0).
- Fagord bevares: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Dansk | Note |
|---|---|---|
| issue | problem (et; pl. problemer) | as in Crashlytics |
| error | fejl (en; pl. fejl) | VV: »Fejl« |
| event | begivenhed (en) | VV: »Begivenheder« for monitor events; »hændelse« stays reserved for incidents, as in VV |
| release | version (en) | same word as app version: in Kestrel a release *is* the app version |
| app version | version | VV: »Version« |
| environment | miljø (et) | |
| resolve / resolved | markér som løst / løst | VV: »Løst«; button short form »Løst« |
| resolve in next release | løs i næste version | |
| unresolved | uløst | filter tab »Uløste« |
| ignore / ignored | ignorér / ignoreret | VV: »Ignoreret« |
| merge (issues) | flet | VV: »Flet« |
| regression / regressed | regression / genåbnet | pill »Genåbnet« |
| new / ongoing | ny / igangværende | issue substatus |
| crash | nedbrud (et; pl. nedbrud) | as in Google Play; VV uses »nedbrud« for the *outage* status, which Kestrel doesn't have |
| crash-free sessions / users | sessioner uden nedbrud / brugere uden nedbrud | short label »Uden nedbrud« |
| release health | stabilitet | tab »Stabilitet«, heading »Versionsstabilitet« |
| stack trace | staksporing | |
| frame / in-app frame | ramme / app-ramme | toggle »Kun app-kode« |
| breadcrumbs | brødkrummer | help text: »trinnene før fejlen« |
| grouping | gruppering | »Hvorfor grupperet« |
| fingerprint | fingeraftryk (et) | the SDK field `fingerprint` stays as code |
| debug files | fejlfindingsfiler | |
| symbolicated / unsymbolicated | symboliseret / ikke symboliseret | status labels; the noun stays »symbolication« |
| project | projekt (et) | |
| key (project key) | nøgle (en) | »projektnøgle« where it must be specific |
| alert / alert rule | alarm / alarmregel | VV: »Alarmer« |
| spike | pludselig stigning | alert type; short »Stigning« |
| first seen / last seen | først set / sidst set | |
| user (of the app) | bruger | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktive brugere (dagligt / ugentligt / månedligt) | avoid DAU/WAU/MAU in the UI |
| signal | signal (et; pl. signaler) | analytics event from an app |
| pageview | sidevisning | VV: »Sidevisninger«; short »Visninger« |
| visitor / unique visitors | besøgende / unikke besøgende | VV |
| referrer | henviser (pl. henvisere) | VV |
| session | session (en; pl. sessioner) | VV: »Sessioner« |
| retention (data) | opbevaring | VV: »Opbevar … i«; never for cohort retention (not in v1) |
| sample / sampling | stikprøve / sampling | sample rate: »samplingrate« |
| tag | tag (et; pl. tags) | VV |
| analytics | statistik | VV: »Statistik« |
| notification channel / channel | notifikationskanal / kanal | kit/VV: »Indstillinger → Notifikationer«; short »kanal« in rules and tables |
| digest | sammendrag (et) | »Send som sammendrag«; status pill »I sammendrag« |
| cooldown (at most every … per issue) | højst én gang pr. … pr. problem | label »Pr. problem, højst én gang pr.« |
| threshold | grænse (en) | in the form: »Mere end [100] begivenheder i timen« |
| adoption | udbredelse | the release's share of sessions |
| abnormal (session ended abnormally) | afsluttet unormalt | |
| errored (session) | med fejl, uden nedbrud | |
| finalized (release) | færdiggjort | lowercase status after a date |
| attempt (delivery) | forsøg (et; pl. forsøg) | |
| header (HTTP) | header (en; pl. headers) | as the kit: »Headernavn«, »Tilføj header« |
| AI assistant | AI-assistent | |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer »symboliseret« phrasing in sentences |
