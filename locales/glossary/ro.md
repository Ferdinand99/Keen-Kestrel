# Keen Kestrel – glossary (Romanian, `ro`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/ro.md).

## Style
- Address: informal-polite singular imperative as in Apple's Romanian UI ("Introdu…", "Alege…"); never
  "dumneavoastră".
- Quotation marks „ ” (nested: « »). Typographic apostrophes and … kept as in English.
- Sentence case; no capitals for nouns in the middle of a sentence.
- Comma-below letters only: ș ț (U+0219, U+021B), never the cedilla forms ş ţ.
- Plurals: `one` (exactly 1), `few` (0 and numbers ending 01–19: "{n} probleme"), `other` (20+: "{n} de probleme").
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, and other brand and technical
  terms.

## Terms

| English | Română | Note |
|---|---|---|
| issue | problemă (f.; pl. probleme) | as in Crashlytics |
| error | eroare (f.; pl. erori) | VV: „Eroare” |
| event | eveniment (n.; pl. evenimente) | VV: „Evenimente” |
| release | versiune (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | versiune | VV |
| environment | mediu (n.; pl. medii) | |
| resolve / resolved | marchează ca rezolvată / rezolvată | VV: „Rezolvat” (m./n.); feminine here because it agrees with „problemă”; button short form „Rezolvată” |
| resolve in next release | rezolvă în versiunea următoare | |
| unresolved | nerezolvată | filter tab „Nerezolvate” |
| ignore / ignored | ignoră / ignorată | VV: „Ignorate” |
| merge (issues) | îmbină | VV: „Îmbinare” |
| regression / regressed | regresie / redeschisă | pill „Redeschisă” |
| new / ongoing | nouă / în desfășurare | issue substatus (feminine: problemă) |
| crash | blocare (f.; pl. blocări) | as in Google Play; „închidere neașteptată” in longer help text |
| crash-free sessions / users | sesiuni fără blocări / utilizatori fără blocări | short label „Fără blocări” |
| release health | stabilitate | tab „Stabilitate”, heading „Stabilitatea versiunilor” |
| stack trace | urmărirea stivei | |
| frame / in-app frame | cadru / cadru din aplicație | toggle „Doar codul aplicației” |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: „pașii dinaintea erorii” |
| grouping | grupare | „De ce au fost grupate” |
| fingerprint | amprentă (f.) | the SDK field `fingerprint` stays as code |
| debug files | fișiere de depanare | |
| symbolicated / unsymbolicated | cu simboluri / fără simboluri | status labels; the noun stays „symbolication” |
| project | proiect (n.) | |
| key (project key) | cheie (f.) | „cheia proiectului” where it must be specific |
| alert / alert rule | alertă (f.) / regulă de alertă | VV: „Alerte” |
| spike | creștere bruscă | alert type; short „Creștere” |
| first seen / last seen | prima apariție / ultima apariție | short „Prima” / „Ultima” in table headers |
| user (of the app) | utilizator | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | utilizatori activi (zilnic / săptămânal / lunar) | avoid DAU/WAU/MAU in the UI |
| signal | semnal (n.; pl. semnale) | analytics event from an app |
| pageview | vizualizare de pagină | VV; short „Vizualizări” |
| visitor / unique visitors | vizitator / vizitatori unici | VV |
| referrer | sursă de trafic | VV: „Principalele surse de trafic” |
| session | sesiune (f.) | VV |
| retention (data) | păstrare | VV: „Păstrează …”; never for cohort retention (not in v1) |
| sample / sampling | eșantion / eșantionare | sample rate: „rată de eșantionare” |
| tag | etichetă (f.) | VV: „Etichete” |
| analytics | statistici | VV: „Statistici” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; articles with a hyphen: „DSN-ul”, „SDK-ul” |
| symbolication | symbolication | unchanged noun; prefer „cu simboluri” phrasing in sentences |
| notification channel / channel | canal de notificări / canal (n.; pl. canale) | VV: „Canale de notificări” |
| digest | rezumat (n.) | „Trimite ca rezumat”; status pill „În rezumat” |
| cooldown (at most every … per issue) | cel mult o dată la … | „Pentru fiecare problemă, cel mult o dată la [1 h]” |
| threshold / alert above | prag (n.) / prag de alertă | form label „Peste [100] evenimente pe oră” |
| adoption | adoptare (f.) | share of the last 7 days’ sessions |
| errored (session) | cu erori, fără blocare | session row label |
| abnormal (session ended abnormally) | încheiate anormal | session row label (f. pl.: sesiuni) |
| crashed (session) | blocate | session row label (f. pl.: sesiuni) |
| finalized (release) | finalizată | lowercase status after a date |
| culprit (“Where” in alert messages) | origine (f.) | as in the issue search help |
| skipped (alert delivery) | omisă | not „ignorată”, which is the issue status |
| attempt (delivery) | încercare (f.; pl. încercări) | |
| header (HTTP) | antet (n.; pl. anteturi) | VV |
| AI assistant | asistent AI | |
| numbers before nouns | „{n} de …” | where a count can be 20+ and isn’t a plural key, put the number after a colon („evenimente în ultima oră: {n}”) |
| entry page | pagină de intrare (f.) | tab „Pagini de intrare”; count column „Intrări” |
| screen size | dimensiune de ecran (f.) | tab „Dimensiuni de ecran” |
| device type (desktop / mobile / tablet) | tip de dispozitiv (desktop / mobil / tabletă) | card „Dispozitive” |
| operating system | sistem de operare (n.) | tab short form „Sisteme” |
| browser | browser (n.; pl. browsere) |  |
| custom event | eveniment personalizat (n.) |  |
| current visitors | vizitatori acum | stat label; meta „În ultimele 5 minute” |
| direct / none (no referrer) | Direct / fără sursă | list row for visits without a traffic source |
| raw events | evenimente brute | kept for filters |
| snippet (k.js) | fragment de cod / script | „scriptul k.js” |
| UTM source / medium / campaign | sursă / mediu / campanie UTM |  |
