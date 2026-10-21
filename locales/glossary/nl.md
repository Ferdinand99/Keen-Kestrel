# Keen Kestrel – glossary (Dutch, `nl`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/nl.md).

## Stijl
- Aanspreekvorm: “je/jouw”, nooit “u”; korte gebiedende wijs (“Voer een naam in”).
- Aanhalingstekens: “…” met ‘…’ binnenin; apostrof ’ (’s, zo’n).
- Zinsbegin met hoofdletter, verder kleine letters (zinsstijl); geen uitroeptekens.
- Meervoud: `.one` (precies 1) en `.other` (al het andere, ook 0).
- Productnamen en technische termen blijven Engels: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.
  Lange samenstellingen vermijden in knoppen en tabs.

## Terms

| English | Nederlands | Note |
|---|---|---|
| issue | probleem (het; mv. problemen) | as in Crashlytics; “issue” is common among developers but less clear to others |
| error | fout (de) | VV: “Fout” |
| event | gebeurtenis (de; mv. gebeurtenissen) | VV: “Gebeurtenissen” |
| release | release (de; mv. releases) | the version string; “versie” is the app version in analytics |
| app version | versie | VV: “Versie” |
| environment | omgeving (de) | |
| resolve / resolved | markeren als opgelost / opgelost | VV: “Opgelost”; button short form “Opgelost” |
| resolve in next release | oplossen in volgende release | |
| unresolved | open | shorter than “onopgelost”; filter tab “Open” |
| ignore / ignored | negeren / genegeerd | VV: “Genegeerd” |
| merge (issues) | samenvoegen | VV |
| regression / regressed | regressie / heropend | pill “Heropend” |
| new / ongoing | nieuw / doorlopend | issue substatus |
| crash | crash (de; mv. crashes) | verb: crashen |
| crash-free sessions / users | crashvrije sessies / crashvrije gebruikers | short label “Crashvrij” |
| release health | stabiliteit | tab “Stabiliteit”, heading “Releasestabiliteit” |
| stack trace | stacktrace (de) | |
| frame / in-app frame | frame (het) / app-frame | toggle “Alleen app-code” |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: “de stappen vóór de fout” |
| grouping | groepering | “Waarom gegroepeerd” |
| fingerprint | vingerafdruk (de) | the SDK field `fingerprint` stays as code |
| debug files | debugbestanden | |
| symbolicated / unsymbolicated | gesymboliseerd / niet gesymboliseerd | status labels; the noun stays “symbolication” |
| project | project (het) | |
| key (project key) | sleutel (de) | “projectsleutel” where it must be specific |
| alert / alert rule | melding / meldingsregel | VV shows alerts as “Meldingen”; “waarschuwing” only for the warning level |
| spike | piek (de) | alert type “Plotselinge piek” in long form |
| first seen / last seen | eerst gezien / laatst gezien | |
| user (of the app) | gebruiker | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | actieve gebruikers (dagelijks / wekelijks / maandelijks) | avoid DAU/WAU/MAU in the UI |
| signal | signaal (het; mv. signalen) | analytics event from an app |
| pageview | paginaweergave | VV: “Paginaweergaven”; short “Weergaven” |
| visitor / unique visitors | bezoeker / unieke bezoekers | VV |
| referrer | verwijzer | VV: “Top verwijzers” |
| session | sessie (de) | VV: “Sessies” |
| retention (data) | bewaartermijn | VV; never for cohort retention (not in v1) |
| sample / sampling | steekproef / sampling | sample rate: “samplefrequentie” |
| tag | tag (de) | VV |
| analytics | analyse | VV: “Analyse” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer “gesymboliseerd” phrasing in sentences |
