# Keen Kestrel – glossary (Russian, `ru`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/ru.md).

## Style
- Neutral register, imperative or impersonal ("Введите…", "Не удалось…"); no "ты", "вы" only when unavoidable.
- Quotation marks «…» (inner „…“). Plain «е» (no «ё»). Sentence case. Ellipsis `…` kept as in English.
- Plurals: one / few / many / other (other = genitive singular, e.g. `{n} события`).
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, and other product, protocol and
  provider names.

## Terms

| English | Русский | Note |
|---|---|---|
| issue | проблема (f.) | as in Crashlytics |
| error | ошибка (f.) | VV: «Ошибка» |
| event | событие (n.) | VV: «События» |
| release | релиз (m.) | the version string; «версия» is the app version in analytics |
| app version | версия | VV |
| environment | окружение (n.) | |
| resolve / resolved | отметить решенной / решено | VV: «Решено»; button short form «Решено» |
| resolve in next release | решить в следующем релизе | |
| unresolved | нерешенные | filter tab «Нерешенные» |
| ignore / ignored | игнорировать / игнорируется | VV: «Игнорируются» |
| merge (issues) | объединить | VV uses «Слияние» as a noun for backups; the verb reads better on a button |
| regression / regressed | регрессия / открыта снова | pill «Открыта снова» |
| new / ongoing | новая / продолжается | issue substatus (feminine: проблема) |
| crash | сбой (m.) | as in Firebase and Google Play; VV uses «сбой» for the *outage* status, which Kestrel doesn't have. Developers also say «краш»; don't use it in the UI |
| crash-free sessions / users | сеансы без сбоев / пользователи без сбоев | short label «Без сбоев» |
| release health | стабильность | tab «Стабильность», heading «Стабильность релизов» |
| stack trace | трассировка стека (f.) | |
| frame / in-app frame | фрейм / фрейм приложения | toggle «Только код приложения» |
| breadcrumbs | хлебные крошки | help text: «шаги до ошибки» |
| grouping | группировка | «Почему сгруппировано» |
| fingerprint | отпечаток (m.) | the SDK field `fingerprint` stays as code |
| debug files | отладочные файлы | |
| symbolicated / unsymbolicated | с символами / без символов | status labels; the noun stays «symbolication» |
| project | проект (m.) | |
| key (project key) | ключ (m.) | «ключ проекта» where it must be specific |
| alert / alert rule | оповещение / правило оповещения | VV: «Оповещения» |
| spike | всплеск (m.) | alert type «Резкий всплеск» in long form |
| first seen / last seen | впервые / последний раз | table headers; «впервые замечено» in sentences |
| user (of the app) | пользователь | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | активные пользователи (за день / неделю / месяц) | avoid DAU/WAU/MAU in the UI |
| signal | сигнал (m.) | analytics event from an app |
| pageview | просмотр страницы | VV: «Просмотры страниц»; short «Просмотры» |
| visitor / unique visitors | посетитель / уникальные посетители | VV |
| referrer | источник (m.) | VV: «Основные источники» |
| session | сеанс (m.) | VV: «Сеансы» |
| retention (data) | хранение | VV: «Хранить …»; never for cohort retention (not in v1) |
| sample / sampling | выборка / сэмплирование | sample rate: «частота сэмплирования» |
| tag | тег (m.) | VV: «Теги» |
| analytics | аналитика | VV: «Аналитика» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; don't decline them, rephrase instead |
| symbolication | symbolication | unchanged noun; prefer «с символами» phrasing in sentences |
| notification channel / channel | канал уведомлений / канал (m.) | VV and the kit: «Каналы», settings page «Уведомления» |
| webhook (in channel lists) | вебхук | follows the kit's channel pages («вебхуки»); keep «webhook» elsewhere |
| email (as a channel) | почта | the kit: «Почта» |
| digest | сводка (f.) | «Отправлять сводкой», status «В сводке» |
| cooldown ("at most every … per issue") | не чаще раза в … для проблемы | label «Для одной проблемы не чаще раза в [1 ч]» |
| threshold / alert above | больше / порог (m.) | form «Больше [100] событий в час»; message field «Порог» |
| multiplier ("and more than … ×") | и больше чем … × | summary «{k}× от обычного» |
| adoption | охват (m.) | share of the sessions in the last 7 days |
| crashed / errored / abnormal (sessions) | со сбоем / с ошибками, без сбоя / завершены аномально | session outcome rows |
| finalized (release) | завершен | lowercase status after a date |
| attempt (delivery) | попытка (f.) | |
| instance | экземпляр (m.) | the Kestrel server; as in the existing strings |
| header (HTTP) | заголовок (m.) | webhook headers |
| sessions_meta and similar captions | из {n} сеанса / сеансов | after «из» the number takes the genitive: one/other «сеанса», few/many «сеансов» |
