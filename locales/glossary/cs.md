# Keen Kestrel – glossary (Czech, `cs`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/cs.md).

## Style
- Address: neutral polite plural imperative ("Zadejte", "Vyberte"); no "Vy" sentences, no informal "ty".
- Quotation marks: „ “. Ellipsis … kept as in English.
- Sentence case; small initial letters except proper names and product names.
- Plurals: one (1), few (2–4), many (decimals: genitive singular), other (0 and 5+: genitive plural).
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, and other product, protocol and
  code names.

## Terms

| English | Čeština | Note |
|---|---|---|
| issue | problém (m.; pl. problémy / problémů) | as in Crashlytics |
| error | chyba (f.) | VV: "Chyba" |
| event | událost (f.; pl. události / událostí) | VV: "Události" |
| release | verze (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | verze | VV |
| environment | prostředí (n.) | |
| resolve / resolved | označit jako vyřešené / vyřešeno | VV: "Vyřešeno"; button short form "Vyřešeno" |
| resolve in next release | vyřešit v příští verzi | |
| unresolved | nevyřešeno | filter tab "Nevyřešené" |
| ignore / ignored | ignorovat / ignorováno | VV: "Ignorováno" |
| merge (issues) | sloučit | VV: "Sloučit" |
| regression / regressed | regrese / znovu otevřeno | pill "Znovu otevřeno" |
| new / ongoing | nové / trvající | issue substatus (neuter, agrees with "problém" only loosely; used as a status word) |
| crash | pád (m.; pl. pády / pádů) | "pád aplikace" in sentences; Google uses "selhání", which is longer and vaguer. VV uses "výpadek" for outages, so no clash |
| crash-free sessions / users | relace bez pádů / uživatelé bez pádů | short label "Bez pádů" |
| release health | stabilita | tab "Stabilita", heading "Stabilita verzí" |
| stack trace | trasování zásobníku | |
| frame / in-app frame | rámec / rámec aplikace | toggle "Jen kód aplikace" |
| breadcrumbs | drobečky (pl.) | from "drobečková navigace"; help text: "kroky před chybou" |
| grouping | seskupení | "Proč seskupeno" |
| fingerprint | otisk (m.) | the SDK field `fingerprint` stays as code |
| debug files | ladicí soubory | |
| symbolicated / unsymbolicated | se symboly / bez symbolů | status labels; the noun stays "symbolication" |
| project | projekt (m.) | |
| key (project key) | klíč (m.) | "klíč projektu" where it must be specific |
| alert / alert rule | upozornění (n.) / pravidlo upozornění | VV: "Upozornění" |
| spike | prudký nárůst | alert type; short "Nárůst" |
| first seen / last seen | poprvé / naposledy | table headers; "poprvé zaznamenáno" in sentences |
| user (of the app) | uživatel | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktivní uživatelé (denně / týdně / měsíčně) | avoid DAU/WAU/MAU in the UI |
| signal | signál (m.) | analytics event from an app |
| pageview | zobrazení stránky | VV; short "Zobrazení" |
| visitor / unique visitors | návštěvník / unikátní návštěvníci | VV |
| referrer | odkazující web | VV: "Nejčastější odkazující weby" |
| session | relace (f.) | VV: "Relace" |
| retention (data) | uchování | VV: "Uchovávat …"; never for cohort retention (not in v1) |
| sample / sampling | vzorek / vzorkování | sample rate: "vzorkovací frekvence" |
| tag | štítek (m.) | VV: "Štítky" |
| analytics | analytika | VV: "Analytika" |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; avoid declining them, rephrase instead |
| symbolication | symbolication | unchanged noun; prefer "se symboly" phrasing in sentences |
| notification channel / channel | kanál oznámení / kanál (m.; pl. kanály / kanálů) | kit: "Přidat kanál", settings page "Oznámení" |
| digest | souhrn (m.) | "Posílat jako souhrn", "souhrn jednou za {time}"; status pill "V souhrnu" |
| cooldown ("at most every … per issue") | nejvýše jednou za … na problém | select label "Pro problém nejvýše jednou za [1 h]" |
| threshold / alert above | víc než … / upozornit nad | "Víc než [100] událostí za hodinu"; message field "Upozornit nad" |
| multiplier | … × víc než obvykle | "A víc než [5] ×" |
| adoption | podíl (m.) | share of the sessions in the last 7 days |
| crashed / errored / abnormal (session) | s pádem / s chybami, bez pádu / abnormálně ukončené | session outcomes |
| finalized (release) | dokončena | lowercase status, agrees with "verze" |
| attempt (delivery) | pokus (m.; pl. pokusy / pokusů) | |
| culprit | místo | message field label; "místo chyby" in search help |
| instance | instance (f.) | kit: "pojmenujte svou instanci" |
| header (webhook) | hlavička (f.) | |
| secrets | tajné údaje | channel form footer |
| web analytics | webová analytika | tab „Web“ |
| entry page | vstupní stránka | column „Vstupy“ |
| screen size | velikost obrazovky |  |
| device type (desktop / mobile / tablet) | typ zařízení (počítač / mobil / tablet) | card „Zařízení“ |
| operating system | operační systém |  |
| browser | prohlížeč (m.) |  |
| custom event | vlastní událost |  |
| current visitors | aktuální návštěvníci | stat card, last 5 minutes |
| direct / none (no referrer) | přímo / žádný |  |
| raw events | surové události | filtered numbers: „filtrované počty“ |
