# Keen Kestrel – glossary (Italian, `it`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/it.md).

## Stile
- Registro: informale/neutro, mai «Lei». Imperativo informale («Inserisci», «Salva») o infinito impersonale; tono
  moderno, calmo, stile Apple.
- Virgolette: « … » per le citazioni, “ ” al loro interno. Apostrofo tipografico ’.
- Maiuscole: solo la prima lettera della frase; nomi dei mesi e dei giorni in minuscolo.
- Plurali: categorie `one` (esattamente 1), `many` (multipli esatti di 1.000.000, con «di»: «{n} di eventi») e
  `other` (tutti gli altri, anche 0).
- Termini tecnici invariati: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook. I prestiti comuni
  (crash, stack trace, frame) sono preferiti alle traduzioni lunghe, come «uptime» e «dashboard» in Vigilant Vole.

## Terms

| English | Italiano | Note |
|---|---|---|
| issue | problema (m.; pl. problemi) | as in Crashlytics |
| error | errore (m.) | VV |
| event | evento (m.) | VV: «Eventi» |
| release | versione (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | versione | VV |
| environment | ambiente (m.) | |
| resolve / resolved | segna come risolto / risolto | VV: «Risolto»; button short form «Risolto» |
| resolve in next release | risolvi nella prossima versione | |
| unresolved | non risolto | filter tab «Non risolti» |
| ignore / ignored | ignora / ignorato | VV: «Ignorati» |
| merge (issues) | unisci | VV: «Unisci» |
| regression / regressed | regressione / riaperto | pill «Riaperto» |
| new / ongoing | nuovo / in corso | issue substatus |
| crash | crash (m., invariable) | Apple and Google say «arresto anomalo», too long for labels; use it in help text if needed |
| crash-free sessions / users | sessioni senza crash / utenti senza crash | short label «Senza crash» |
| release health | stabilità | tab «Stabilità», heading «Stabilità delle versioni» |
| stack trace | stack trace (f.) | loanword, as Italian developers say it |
| frame / in-app frame | frame (m.) / frame dell’app | toggle «Solo codice dell’app» |
| breadcrumbs | breadcrumb (m. pl.) | Sentry term kept; help text: «i passaggi prima dell’errore» |
| grouping | raggruppamento | «Perché raggruppati» |
| fingerprint | impronta (f.) | the SDK field `fingerprint` stays as code |
| debug files | file di debug | |
| symbolicated / unsymbolicated | simbolizzato / non simbolizzato | status labels; the noun stays «symbolication» |
| project | progetto (m.) | |
| key (project key) | chiave (f.) | «chiave del progetto» where it must be specific |
| alert / alert rule | avviso (m.) / regola di avviso | VV: «Avvisi» |
| spike | picco (m.) | alert type «Picco improvviso» in long form |
| first seen / last seen | prima occorrenza / ultima occorrenza | short «Prima» / «Ultima» in table headers |
| user (of the app) | utente | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | utenti attivi (giornalieri / settimanali / mensili) | avoid DAU/WAU/MAU in the UI |
| signal | segnale (m.) | analytics event from an app |
| pageview | visualizzazione di pagina | VV; short «Visualizzazioni» |
| visitor / unique visitors | visitatore / visitatori unici | VV |
| referrer | referrer (m.) | VV: «Referrer principali» |
| session | sessione (f.) | VV |
| retention (data) | conservazione (f.) | VV; never for cohort retention (not in v1) |
| sample / sampling | campione / campionamento | sample rate: «frequenza di campionamento» |
| tag | tag (m.) | VV |
| analytics | analisi | VV: «Analisi» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; «il DSN», «il dSYM», «l’SDK» |
| symbolication | symbolication (f.) | unchanged noun; prefer «simbolizzato» phrasing in sentences |
| notification channel / channel | canale di notifica / canale (m.) | VV: «Canali di notifica» |
| digest | riepilogo (m.) | «Invia come riepilogo»; status pill «Nel riepilogo» |
| cooldown (at most every … per issue) | al massimo ogni … | «Per problema, al massimo ogni [1 h]» |
| threshold / alert above | soglia (f.) / soglia di avviso | form label «Più di [100] eventi l’ora» |
| adoption | adozione (f.) | share of the last 7 days’ sessions; also share of an app’s daily active users |
| errored (session) | con errori, senza crash | session row label |
| abnormal (session ended abnormally) | chiusura anomala | session row label |
| crashed (session) | con crash | session row label |
| finalized (release) | finalizzata | lowercase status after a date |
| culprit (“Where” in alert messages) | origine (f.) | as in the issue search help |
| skipped (alert delivery) | saltato | not «ignorato», which is the issue status |
| attempt (delivery) | tentativo (m.) | failed deliveries «vengono ritentati» |
| header (HTTP) | intestazione (f.) | VV |
| AI assistant | assistente IA | |
| entry page | pagina di ingresso (f.) | tab «Pagine di ingresso»; count column «Ingressi» |
| screen size | dimensione schermo (f.) | tab «Dimensioni schermo» |
| device type (desktop / mobile / tablet) | tipo di dispositivo (desktop / mobile / tablet) | card «Dispositivi» |
| operating system | sistema operativo (m.) | tab short form «Sistemi» |
| browser | browser (m., invariable) |  |
| custom event | evento personalizzato (m.) |  |
| current visitors | visitatori ora | stat label; meta «Ultimi 5 minuti» |
| direct / none (no referrer) | Diretto / nessuno | list row for visits without a referrer |
| raw events | eventi grezzi | kept for filters |
| snippet (k.js) | snippet (m.) | loanword, as with «stack trace» |
| UTM source / medium / campaign | sorgente / mezzo / campagna UTM | as in Google Analytics |
| app analytics | analisi app | tab «App»; page title «Analisi app di …» |
| install / install id | installazione (f.) / ID di installazione (m.) | a random id per app installation |
| signal type | tipo di segnale (m.) | `App.launched` and the like stay as code |
| payload | payload (m., invariable) | loanword, as with «snippet»; the key/value data an app sends with a signal |
| signal explorer | Esplora segnali | nav item and page title, Italian UI style for an explorer tool |
| test signal | segnale di test (m.) | from debug builds, TestFlight and the simulator |
| strict mode | modalità rigorosa (f.) | |
| build (app build) | build (f., invariable) | «build di debug» |
| device model | modello di dispositivo (m.) | |
| locale | lingua e regione | as Apple’s «Lingua e regione»; card «Lingue» |
| platform | piattaforma (f.) | |
| OS version | versione del sistema (f.) | |
| release (as an analytics dimension) | versione completa | dimension label, to tell `bundle@version+build` apart from «Versione» (app version) |
