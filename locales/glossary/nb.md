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
| notification channel / channel | varslingskanal / kanal | kit/VV: «Innstillinger → Varslinger»; short «kanal» in rules and tables |
| digest | sammendrag (et) | «Send som sammendrag»; status pill «I sammendrag» |
| cooldown (at most every … per issue) | høyst én gang per … for hvert problem | label «Per problem, høyst én gang per» |
| threshold | grense (en) | in the form: «Mer enn [100] hendelser i timen» |
| adoption | utbredelse | the release's share of sessions; also share of an app's daily active users |
| abnormal (session ended abnormally) | avsluttet unormalt | |
| errored (session) | med feil, uten krasj | |
| finalized (release) | ferdigstilt | lowercase status after a date |
| attempt (delivery) | forsøk (et; pl. forsøk) | |
| header (HTTP) | header (en; pl. headere) | as the kit: «Headernavn», «Legg til header» |
| AI assistant | KI-assistent | |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer «symbolisert» phrasing in sentences |
| web analytics | webanalyse | tab «Web»; page title «Webanalyse for …» |
| entry page | landingsside (en) | count column «Innganger» |
| screen size | skjermstørrelse (en) | |
| device type (desktop / mobile / tablet) | enhetstype (datamaskin / mobil / nettbrett) | card «Enheter» |
| operating system | operativsystem (et) | |
| browser | nettleser (en) | |
| custom event | egen hendelse (pl. egne hendelser) | shorter than «egendefinert hendelse» |
| current visitors | besøkende nå | live counter, last 5 minutes |
| direct / none (no referrer) | direkte / ingen | |
| raw events | rå hendelser | settings label «Behold rå webhendelser» |
| install / install id | installasjon / installasjons-ID | a random id per app installation; only its keyed hash is kept |
| signal type | signaltype (en) |  |
| payload | nyttelast (en) | the key/value data sent with a signal |
| signal explorer | signalutforsker (en) | tab «Signalutforsker» |
| test signal | testsignal (et; pl. testsignaler) | from debug builds, TestFlight, the simulator |
| strict mode | streng modus | user id changes every day |
| build (app build) | bygg (et; pl. bygg) | as `dist` in the event view; debug build: «feilsøkingsbygg» |
| device model | enhetsmodell (en) |  |
| locale | språk og region | card «Språk» |
| platform | plattform (en) |  |
| release identifier (bundle@version+build) | versjons-ID | dimension label, to tell it apart from «Versjon» |
