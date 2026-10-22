# Keen Kestrel – glossary (Ukrainian, `uk`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/uk.md).

## Стиль
- Звертання: нейтральне, наказовий спосіб або безособові конструкції; без «ти», «ви» лише коли неминуче.
- Лапки: «ялинки», внутрішні „лапки“. Апостроф лише ’ (U+2019): ім’я, обов’язково.
- Регістр: речення з малої літери, великою лише перша літера рядка та власні назви.
- Без російських слів і написань. Правопис 2019 року.
- Залишаються англійською: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, назви продуктів і протоколів.
- Коротке в кнопках, вкладках, мітках.
- Множина: one / few / many / other; для other (дробові) форма родового відмінка однини.

## Terms

| English | Українська | Note |
|---|---|---|
| issue | проблема (f.) | as in Crashlytics |
| error | помилка (f.) | VV: «Помилка» |
| event | подія (f.) | VV: «Події» |
| release | реліз (m.) | the version string; «версія» is the app version in analytics |
| app version | версія | VV |
| environment | середовище (n.) | |
| resolve / resolved | позначити як усунену / усунено | VV: «Усунено»; button short form «Усунено» |
| resolve in next release | усунути в наступному релізі | |
| unresolved | неусунені | filter tab «Неусунені» |
| ignore / ignored | ігнорувати / ігнорується | VV: «Ігнорується» |
| merge (issues) | об’єднати | VV: «Об’єднати» |
| regression / regressed | регресія / відкрито знову | pill «Відкрито знову» |
| new / ongoing | нова / триває | issue substatus (feminine: проблема) |
| crash | збій (m.) | as in Google Play; VV uses «збій» for the *outage* status, which Kestrel doesn't have |
| crash-free sessions / users | сеанси без збоїв / користувачі без збоїв | short label «Без збоїв» |
| release health | стабільність | tab «Стабільність», heading «Стабільність релізів» |
| stack trace | трасування стека (n.) | |
| frame / in-app frame | фрейм / фрейм застосунку | toggle «Лише код застосунку» |
| breadcrumbs | хлібні крихти | help text: «кроки перед помилкою» |
| grouping | групування | «Чому згруповано» |
| fingerprint | відбиток (m.) | the SDK field `fingerprint` stays as code |
| debug files | файли налагодження | |
| symbolicated / unsymbolicated | із символами / без символів | status labels; the noun stays «symbolication» |
| project | проєкт (m.) | 2019 spelling with «є» |
| key (project key) | ключ (m.) | «ключ проєкту» where it must be specific |
| alert / alert rule | сповіщення / правило сповіщень | VV: «Сповіщення» (same word as notification) |
| spike | сплеск (m.) | alert type «Різкий сплеск» in long form |
| first seen / last seen | уперше / востаннє | table headers; «уперше помічено» in sentences |
| user (of the app) | користувач | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | активні користувачі (за день / тиждень / місяць) | avoid DAU/WAU/MAU in the UI |
| signal | сигнал (m.) | analytics event from an app; VV uses «сигнал» loosely for heartbeat, rarely shown |
| pageview | перегляд сторінки | VV: «Перегляди сторінки»; short «Перегляди» |
| visitor / unique visitors | відвідувач / унікальні відвідувачі | VV |
| referrer | джерело переходів | VV |
| session | сеанс (m.) | VV: «Сеанси» |
| retention (data) | зберігання | VV: «Зберігати …»; never for cohort retention (not in v1) |
| sample / sampling | вибірка / семплювання | sample rate: «частота семплювання» |
| tag | тег (m.) | VV: «Теги» |
| analytics | аналітика | VV: «Аналітика» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; don't decline them, rephrase instead |
| symbolication | symbolication | unchanged noun; prefer «із символами» phrasing in sentences |
| notification channel / channel | канал сповіщень / канал (m.) | VV and the kit: «Канали», settings page «Сповіщення»; alerts and notifications share the word |
| webhook (in channel lists) | вебхук | follows the kit's channel pages («вебхуки»); keep «webhook» elsewhere |
| email (as a channel) | пошта | the kit: «Пошта» |
| digest | зведення (n.) | «Надсилати зведенням», status «У зведенні» |
| cooldown ("at most every … per issue") | не частіше ніж раз на … для проблеми | label «Для однієї проблеми не частіше ніж раз на [1 год]» |
| threshold / alert above | понад / поріг (m.) | form «Понад [100] подій за годину»; message field «Поріг» |
| multiplier ("and more than … ×") | і понад … × | summary «{k}× від звичного» |
| adoption | охоплення (n.) | share of the sessions in the last 7 days; also share of an app’s daily active users |
| crashed / errored / abnormal (sessions) | зі збоєм / з помилками, без збою / завершено аномально | session outcome rows |
| finalized (release) | завершено | lowercase status after a date |
| attempt (delivery) | спроба (f.) | |
| instance | екземпляр (m.) | the Kestrel server; as in the existing strings |
| header (HTTP) | заголовок (m.) | webhook headers |
| sessions_meta and similar captions | з {n} сеансу / сеансів | after «з» the number takes the genitive: one/other «сеансу», few/many «сеансів» |
| web analytics | вебаналітика | tab «Веб»; 2019 spelling, written together |
| entry page | сторінка входу | column «Входи» |
| screen size | розмір екрана |  |
| device type (desktop / mobile / tablet) | тип пристрою (комп’ютер / телефон / планшет) | card «Пристрої» |
| operating system | операційна система |  |
| browser | браузер (m.) |  |
| custom event | власна подія | «Власні події» |
| current visitors | зараз на сайті | stat card, last 5 minutes |
| direct / none (no referrer) | прямий перехід / немає |  |
| raw events | необроблені події | filtered numbers: «відфільтровані числа» |
| UTM medium | UTM-канал | UTM source / campaign: «UTM-джерело» / «UTM-кампанія» |
| app analytics | аналітика застосунку | tab «Застосунок» |
| install / install id | встановлення / ідентифікатор встановлення | random id per app installation |
| signal type | тип сигналу |  |
| payload | параметри | the key/value data an app sends with a signal |
| signal explorer | аналіз сигналів | nav item and page title; dashboard nav item «Огляд» |
| test signal | тестовий сигнал |  |
| strict mode | суворий режим | the user id changes every day |
| build (app build) | збірка (f.) | debug build: «налагоджувальна збірка» |
| device model | модель пристрою |  |
| locale (language and region setting) | мова й регіон |  |
| platform | платформа |  |
