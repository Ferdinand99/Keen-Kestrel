# Keen Kestrel – glossary (Finnish, `fi`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/fi.md).

## Style
- Address: neutral. Imperatives and infinitives ("Valitse projekti", "Lisää avain"), never "te"; "sinä" forms only
  when unavoidable ("sinun").
- Quotation marks: ”…” (same mark on both sides).
- Capitalisation: sentence case; no capitalised nouns in the middle of a sentence.
- Compounds are written as one word (virheenkorjaustiedosto, hälytyssääntö); long labels are shortened.
- Numerals take the partitive singular after n > 1 in UI counts, which equals the `other` form; `one` = exactly 1.
  Avoid case endings on `{placeholders}`; rephrase instead.
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Suomi | Note |
|---|---|---|
| issue | ongelma | as in Crashlytics |
| error | virhe | VV: ”Virhe” |
| event | tapahtuma | VV: ”Tapahtumat” |
| release | versio | same word as app version: in Kestrel a release *is* the app version |
| app version | versio | VV: ”Versio” |
| environment | ympäristö | |
| resolve / resolved | merkitse ratkaistuksi / ratkaistu | VV: ”Ratkaistu”; button short form ”Ratkaistu” |
| resolve in next release | ratkaistu seuraavassa versiossa | |
| unresolved | ratkaisematon | filter tab ”Avoimet” is shorter if needed |
| ignore / ignored | ohita / ohitettu | VV uses ”Ohitetaan” for ignored JS errors |
| merge (issues) | yhdistä | VV: ”Yhdistä” |
| regression / regressed | regressio / avattu uudelleen | pill ”Uudelleen” |
| new / ongoing | uusi / jatkuva | issue substatus |
| crash | kaatuminen (pl. kaatumiset) | verb: kaatua |
| crash-free sessions / users | istunnot ilman kaatumisia / käyttäjät ilman kaatumisia | short label ”Ei kaatumisia” |
| release health | vakaus | tab ”Vakaus”, heading ”Version vakaus” |
| stack trace | pinojäljitys | |
| frame / in-app frame | kehys / sovelluksen kehys | toggle ”Vain sovelluksen koodi” |
| breadcrumbs | murupolku | help text: ”vaiheet ennen virhettä” |
| grouping | ryhmittely | ”Miksi ryhmitelty” |
| fingerprint | sormenjälki | the SDK field `fingerprint` stays as code |
| debug files | virheenkorjaustiedostot | |
| symbolicated / unsymbolicated | symboloitu / symboloimaton | status labels; the noun stays ”symbolication” |
| project | projekti | |
| key (project key) | avain | ”projektiavain” where it must be specific |
| alert / alert rule | hälytys / hälytyssääntö | VV: ”Hälytykset” |
| spike | äkillinen kasvu | alert type; short ”Piikki” |
| first seen / last seen | ensimmäinen havainto / viimeisin havainto | short ”Ensin” / ”Viimeksi” in table headers |
| user (of the app) | käyttäjä | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktiiviset käyttäjät (päivittäin / viikoittain / kuukausittain) | avoid DAU/WAU/MAU in the UI |
| signal | signaali | analytics event from an app |
| pageview | sivunäyttö | VV: ”Sivunäytöt”; short ”Näytöt” |
| visitor / unique visitors | kävijä / yksittäiset kävijät | VV |
| referrer | viittaaja | VV |
| session | istunto | VV: ”Istunnot” |
| retention (data) | säilytysaika | VV; never for cohort retention (not in v1) |
| sample / sampling | otos / otanta | sample rate: ”otantataajuus” |
| tag | tunniste | VV: ”Tunnisteet” |
| analytics | analytiikka | VV: ”Analytiikka” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer ”symboloitu” phrasing in sentences |
