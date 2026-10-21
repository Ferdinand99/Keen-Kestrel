# Keen Kestrel – glossary (Norwegian Bokmål, `nb`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/nb.md).

## Stil
- Tiltale: uformell «du», korte imperativer («Skriv inn…», «Velg…»). Aldri «De».
- Anførselstegn: «…» (inni: ‘…’).
- Sentence case: bare første ord med stor bokstav; måneder og ukedager med liten.
- Sammensetninger skrives i ett ord (feilsøkingsfil, varselregel); lånord med bindestrek (app-ramme, DSN-nøkkel).
- Flertall: `.one` (nøyaktig 1) og `.other` (alt annet, også 0).
- Fagord beholdes: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Norsk | Note |
|---|---|---|
| issue | problem (et; pl. problemer) | as in Crashlytics; "sak" (Jira) sounds like a ticket |
| error | feil (en; pl. feil) | VV: «Feil» |
| event | hendelse (en; pl. hendelser) | VV uses «hendelse» for both incidents and monitor events; Kestrel has no incidents, so no clash here |
| release | versjon (en) | same word as app version: in Kestrel a release *is* the app version; «utgivelse» only when the act of publishing is meant |
| app version | versjon | VV: «Versjon» |
| environment | miljø (et; pl. miljøer) | |
| resolve / resolved | merk som løst / løst | VV: «Løst»; button short form «Løst» |
| resolve in next release | løs i neste versjon | |
| unresolved | uløst | filter tab «Uløste» |
| ignore / ignored | ignorer / ignorert | |
| merge (issues) | slå sammen | VV: «Slå sammen» |
| regression / regressed | regresjon / gjenåpnet | pill «Gjenåpnet»; «regresjon» in alert names and help text |
| new / ongoing | ny / pågående | issue substatus |
| crash | krasj (et; pl. krasj) | "krasje" as verb; avoid «kræsj» |
| crash-free sessions / users | krasjfrie økter / krasjfrie brukere | short label «Krasjfri» |
| release health | stabilitet | «helse» reads oddly; tab «Stabilitet», heading «Versjonsstabilitet» |
| stack trace | stakksporing | |
| frame / in-app frame | ramme / app-ramme | toggle «Bare app-kode» |
| breadcrumbs | brødsmuler | help text: «hendelsene før feilen» |
| grouping | gruppering | «Hvorfor gruppert» for "why grouped" |
| fingerprint | fingeravtrykk (et) | the SDK field `fingerprint` stays as code |
| debug files | feilsøkingsfiler | |
| symbolicated / unsymbolicated | symbolisert / ikke symbolisert | status labels; the noun stays «symbolication» |
| project | prosjekt (et) | |
| key (project key) | nøkkel (en) | «prosjektnøkkel» where it must be specific |
| alert / alert rule | varsel / varselregel | VV: «Varsler» |
| spike | brå økning | alert type; not «topp» (peak in charts) |
| first seen / last seen | først sett / sist sett | |
| user (of the app) | bruker | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktive brukere (daglig / ukentlig / månedlig) | avoid DAU/WAU/MAU in the UI |
| signal | signal (et; pl. signaler) | analytics event from an app |
| pageview | sidevisning | VV: «Sidevisninger»; short «Visninger» |
| visitor / unique visitors | besøkende / unike besøkende | VV |
| referrer | henvisende side | VV: «Mest henvisende sider» |
| session | økt (en; pl. økter) | VV uses «økt» for sign-in sessions too |
| retention (data) | lagringstid | settings label «Behold … i» as in VV; never for cohort retention (not in v1) |
| sample / sampling | utvalg / sampling | sample rate: «utvalgsrate» |
| tag | etikett | VV: «Etiketter» |
| analytics | analyse | VV: «Analyse» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer «symbolisert» phrasing in sentences |
