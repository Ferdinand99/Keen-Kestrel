# Keen Kestrel – glossary (Persian, `fa`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/fa.md).

## Style
- Right-to-left. Standard Iranian Persian, neutral polite imperative («وارد کنید», «انتخاب کنید»); no slang, no «شما»
  unless needed.
- Quotes « » for quoted names and values. Punctuation ، ؛ ؟ ; keep … and the em dash.
- Letters ک ی (never ك ي). Use ZWNJ (U+200C) in می‌شود, ‌ها plurals, etc.
- Literal digits in prose are Persian digits ۰-۹; code-like values (versions, URLs, release strings) stay as in English.
- No sentence capitalisation. Plurals: only `one` and `other`, both with a singular noun after the number.
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, and other product, protocol and
  technical terms. Wrap them in `<bdi>` in templates.

## Terms

| English | فارسی | Note |
|---|---|---|
| issue | مشکل | as in Crashlytics; `{n} مشکل` |
| error | خطا | VV: «خطا» |
| event | رویداد | VV uses «رویداد» for both incidents and monitor events; Kestrel has no incidents, so no clash here |
| release | نسخه | same word as app version: in Kestrel a release *is* the app version |
| app version | نسخه | VV |
| environment | محیط | |
| resolve / resolved | علامت‌گذاری به‌عنوان حل‌شده / حل‌شده | VV: «حل‌شده»; button short form «حل‌شده» |
| resolve in next release | حل در نسخهٔ بعدی | |
| unresolved | حل‌نشده | filter tab «حل‌نشده» |
| ignore / ignored | نادیده‌گرفتن / نادیده | VV: «نادیده» |
| merge (issues) | ادغام | VV: «ادغام» |
| regression / regressed | بازگشت خطا / بازگشایی‌شده | pill «بازگشایی‌شده» |
| new / ongoing | جدید / ادامه‌دار | issue substatus |
| crash | کرش | familiar loanword among developers; «خرابی» is vaguer and close to VV's outage words |
| crash-free sessions / users | نشست‌های بدون کرش / کاربران بدون کرش | short label «بدون کرش» |
| release health | پایداری | tab «پایداری», heading «پایداری نسخه‌ها» |
| stack trace | ردیابی پشته | |
| frame / in-app frame | فریم / فریم برنامه | toggle «فقط کد برنامه» |
| breadcrumbs | ردپا | help text: «مراحل پیش از خطا» |
| grouping | گروه‌بندی | «چرا گروه‌بندی شد» |
| fingerprint | اثر انگشت | the SDK field `fingerprint` stays as code |
| debug files | فایل‌های اشکال‌زدایی | |
| symbolicated / unsymbolicated | با نماد / بدون نماد | status labels; the noun stays «symbolication» |
| project | پروژه | |
| key (project key) | کلید | «کلید پروژه» where it must be specific |
| alert / alert rule | هشدار / قانون هشدار | VV: «هشدارها» |
| spike | جهش | alert type; «افزایش ناگهانی» in long form |
| first seen / last seen | اولین مشاهده / آخرین مشاهده | |
| user (of the app) | کاربر | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | کاربران فعال (روزانه / هفتگی / ماهانه) | avoid DAU/WAU/MAU in the UI |
| signal | سیگنال | analytics event from an app |
| pageview | بازدید صفحه | VV |
| visitor / unique visitors | بازدیدکننده / بازدیدکنندگان یکتا | VV |
| referrer | سایت ارجاع‌دهنده | VV |
| session | نشست | VV: «نشست‌ها» |
| retention (data) | نگهداری | VV: «نگهداری سابقه»; never for cohort retention (not in v1) |
| sample / sampling | نمونه / نمونه‌برداری | sample rate: «نرخ نمونه‌برداری» |
| tag | برچسب | VV: «برچسب‌ها» |
| analytics | تحلیل | VV: «تحلیل» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged, in Latin script; prefer «با نماد» phrasing in sentences |
