# Keen Kestrel – glossary (Polish, `pl`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/pl.md).

## Style
- Address: neutral imperatives and impersonal phrasing ("Wpisz nazwę", "Nie można zapisać"); no "Pan/Pani", "ty"
  avoided. Avoid gendered forms for the viewer; use impersonal phrasing.
- Quotes: „…” (and »…« inside). Ellipsis `…` kept as in English.
- Names inserted into sentences are kept in the nominative inside quotes where the case would change:
  `Usuń: „{name}”?`.
- Sentence case; months and weekdays lower-case.
- Plurals: one / few / many / other. `.few` (2–4, not 12–14) nominative plural ("3 problemy"); `.many` (0, 5–21, …)
  genitive plural ("5 problemów"); `.other` (fractions) genitive singular.
- Prefer short forms in tabs, buttons and pills.
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Polski | Note |
|---|---|---|
| issue | problem (m.; pl. problemy / problemów) | as in Crashlytics |
| error | błąd (m.; pl. błędy / błędów) | VV: "Błąd" |
| event | zdarzenie (n.; pl. zdarzenia / zdarzeń) | VV: "Zdarzenia" |
| release | wersja (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | wersja | VV |
| environment | środowisko (n.) | |
| resolve / resolved | oznacz jako rozwiązany / rozwiązany | VV: "Rozwiązany"; button short form "Rozwiązany" |
| resolve in next release | rozwiąż w następnej wersji | |
| unresolved | nierozwiązany | filter tab "Nierozwiązane" |
| ignore / ignored | ignoruj / ignorowany | VV: "Ignorowane" |
| merge (issues) | scal | VV: "Scal" |
| regression / regressed | regresja / ponownie otwarty | pill "Ponownie otwarty" |
| new / ongoing | nowy / trwający | issue substatus |
| crash | awaria (f.; pl. awarie / awarii) | as in Google Play; VV uses "awaria" for the *outage* status, which Kestrel doesn't have |
| crash-free sessions / users | sesje bez awarii / użytkownicy bez awarii | short label "Bez awarii" |
| release health | stabilność | tab "Stabilność", heading "Stabilność wersji" |
| stack trace | ślad stosu (m.) | |
| frame / in-app frame | ramka / ramka aplikacji | toggle "Tylko kod aplikacji" |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: "kroki przed błędem" |
| grouping | grupowanie | "Dlaczego zgrupowano" |
| fingerprint | odcisk (m.) | short for "odcisk palca"; the SDK field `fingerprint` stays as code |
| debug files | pliki debugowania | |
| symbolicated / unsymbolicated | z symbolami / bez symboli | status labels; the noun stays "symbolication" |
| project | projekt (m.) | |
| key (project key) | klucz (m.) | "klucz projektu" where it must be specific |
| alert / alert rule | alert (m.) / reguła alertu | VV: "Alerty" |
| spike | skok (m.) | alert type "Nagły skok" in long form |
| first seen / last seen | pierwsze wystąpienie / ostatnie wystąpienie | short "Pierwsze" / "Ostatnie" in table headers |
| user (of the app) | użytkownik | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktywni użytkownicy (dziennie / tygodniowo / miesięcznie) | avoid DAU/WAU/MAU in the UI |
| signal | sygnał (m.) | analytics event from an app |
| pageview | wyświetlenie strony | VV; short "Wyświetlenia" |
| visitor / unique visitors | odwiedzający / unikalni odwiedzający | VV |
| referrer | źródło odwiedzin | VV: "Główne źródła odwiedzin" |
| session | sesja (f.) | VV |
| retention (data) | przechowywanie | VV: "Przechowuj … przez"; never for cohort retention (not in v1) |
| sample / sampling | próbka / próbkowanie | sample rate: "częstotliwość próbkowania" |
| tag | tag (m.) | VV: "Tagi" |
| analytics | analityka | VV: "Analityka" |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; inflect with a hyphen only if unavoidable, better rephrase |
| symbolication | symbolication | unchanged noun; prefer "z symbolami" phrasing in sentences |
| notification channel / channel | kanał powiadomień / kanał (m.; pl. kanały / kanałów) | kit: "Dodaj kanał", settings page "Powiadomienia" |
| digest | zestawienie (n.) | "Wysyłaj jako zestawienie", "zestawienie co {time}"; status pill "W zestawieniu" |
| cooldown ("at most every … per issue") | najwyżej co … na problem | select label "Na problem najwyżej co [1 godz.]" |
| threshold / alert above | ponad … / alert powyżej | "Ponad [100] zdarzeń na godzinę"; message field "Alert powyżej" |
| multiplier | … × więcej niż zwykle | "I ponad [5] ×" |
| adoption | udział (m.) | share of the sessions in the last 7 days |
| crashed / errored / abnormal (session) | z awarią / z błędami, bez awarii / zakończone nieprawidłowo | session outcomes |
| finalized (release) | sfinalizowana | lowercase status, agrees with "wersja" |
| attempt (delivery) | próba (f.; pl. próby / prób) | |
| culprit | miejsce | message field label; "źródło błędu" in search help |
| instance | instancja (f.) | kit: "nazwij instancję" |
| header (webhook) | nagłówek (m.) | |
| secrets | sekrety | channel form footer |
| web analytics | analityka internetowa | tab „Web”; settings and overview card „Analityka internetowa” |
| entry page | strona wejścia | list „Strony wejścia”; column „Wejścia” |
| screen size | rozmiar ekranu | list „Rozmiary ekranu” |
| device type (desktop / mobile / tablet) | typ urządzenia (komputer / telefon / tablet) | card „Urządzenia” |
| operating system | system operacyjny |  |
| browser | przeglądarka (f.) |  |
| custom event | własne zdarzenie | „Własne zdarzenia”; keeps „zdarzenie” for event |
| current visitors | odwiedzający teraz | stat card, last 5 minutes |
| direct / none (no referrer) | bezpośrednio / brak |  |
| raw events | surowe zdarzenia | „Liczby po filtrowaniu” for filtered numbers |
