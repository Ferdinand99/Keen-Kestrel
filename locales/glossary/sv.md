# Keen Kestrel – glossary (Swedish, `sv`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/sv.md).

## Stil
- Tilltal: informellt "du", naturliga imperativ ("Ange ett namn"). Aldrig "ni".
- Citattecken: ”…” (högerställt på båda sidor), ’…’ inuti.
- Gemener i meningar (sentence case), månader och veckodagar med liten bokstav.
- Kompositer skrivs ihop (felsökningsfil, larmregel); lånord med bindestreck (app-ram, DSN-nyckel).
- Plural: `.one` (exakt 1) och `.other` (allt annat, även 0).
- Facktermer behålls: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Svenska | Note |
|---|---|---|
| issue | problem (ett; pl. problem) | as in Crashlytics |
| error | fel (ett; pl. fel) | VV: ”Fel” |
| event | händelse (en) | VV: ”Händelser” |
| release | version (en) | same word as app version: in Kestrel a release *is* the app version |
| app version | version | VV: ”Version” |
| environment | miljö (en) | |
| resolve / resolved | markera som åtgärdad / åtgärdad | VV: ”Åtgärdad”; button short form ”Åtgärdad” |
| resolve in next release | åtgärda i nästa version | |
| unresolved | öppen | shorter than ”ej åtgärdad”; filter tab ”Öppna” |
| ignore / ignored | ignorera / ignorerad | |
| merge (issues) | slå samman | VV: ”Slå samman” |
| regression / regressed | regression / återöppnad | pill ”Återöppnad” |
| new / ongoing | ny / pågående | issue substatus |
| crash | krasch (en; pl. krascher) | verb: krascha |
| crash-free sessions / users | kraschfria sessioner / kraschfria användare | short label ”Kraschfri” |
| release health | stabilitet | tab ”Stabilitet”, heading ”Versionsstabilitet” |
| stack trace | stackspårning | |
| frame / in-app frame | ram / app-ram | toggle ”Bara appens kod” |
| breadcrumbs | brödsmulor | help text: ”stegen före felet” |
| grouping | gruppering | ”Varför grupperade” |
| fingerprint | fingeravtryck (ett) | the SDK field `fingerprint` stays as code |
| debug files | felsökningsfiler | |
| symbolicated / unsymbolicated | symboliserad / inte symboliserad | status labels; the noun stays ”symbolication” |
| project | projekt (ett) | |
| key (project key) | nyckel (en) | ”projektnyckel” where it must be specific |
| alert / alert rule | larm (ett) / larmregel | VV: ”Larm” |
| spike | plötslig ökning | alert type; short ”Ökning”; not ”topp” (peak in charts) |
| first seen / last seen | först sedd / senast sedd | |
| user (of the app) | användare | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktiva användare (dagligen / veckovis / månadsvis) | avoid DAU/WAU/MAU in the UI |
| signal | signal (en; pl. signaler) | analytics event from an app |
| pageview | sidvisning | VV: ”Sidvisningar”; short ”Visningar” |
| visitor / unique visitors | besökare / unika besökare | VV |
| referrer | hänvisare | VV |
| session | session (en) | VV: ”Sessioner” |
| retention (data) | lagringstid | VV: ”Spara … i”; never for cohort retention (not in v1) |
| sample / sampling | urval / sampling | sample rate: ”urvalsfrekvens” |
| tag | tagg | VV: ”Taggar” |
| analytics | statistik | VV: ”Statistik” |
| notification channel / channel | aviseringskanal / kanal | kit/VV: ”Inställningar → Aviseringar”; short ”kanal” in rules and tables |
| digest | sammandrag (ett) | ”Skicka som sammandrag”; status pill ”I sammandrag” |
| cooldown (at most every … per issue) | högst en gång per … och problem | label ”Per problem, högst en gång per” |
| threshold | gräns (en) | in the form: ”Fler än [100] händelser i timmen” |
| adoption | spridning | the release's share of sessions |
| abnormal (session ended abnormally) | avslutades onormalt | |
| errored (session) | med fel, ingen krasch | |
| finalized (release) | slutförd | lowercase status after a date |
| attempt (delivery) | försök (ett; pl. försök) | |
| header (HTTP) | rubrik | as the kit: ”Egna rubriker”; ”Rubriknamn”, ”Lägg till rubrik” |
| AI assistant | AI-assistent | |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer ”symboliserad” phrasing in sentences |
| web analytics | webbstatistik | tab ”Webb”; page title ”Webbstatistik för …” |
| entry page | landningssida (en) | count column ”Ingångar” |
| screen size | skärmstorlek (en) | |
| device type (desktop / mobile / tablet) | enhetstyp (dator / mobil / surfplatta) | card ”Enheter” |
| operating system | operativsystem (ett; pl. operativsystem) | |
| browser | webbläsare (en) | |
| custom event | egen händelse (pl. egna händelser) | shorter than ”anpassad händelse” |
| current visitors | besökare just nu | live counter, last 5 minutes |
| direct / none (no referrer) | direkt / ingen | |
| raw events | råa händelser | settings label ”Spara råa webbhändelser” |
