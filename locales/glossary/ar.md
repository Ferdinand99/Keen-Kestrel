# Keen Kestrel – glossary (Arabic, `ar`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/ar.md).

## Style
- Right-to-left. Modern Standard Arabic, neutral. Nouns and the masculine singular imperative ("أدخل", "اختر"), or
  impersonal phrasing. Never formal or stiff.
- Quotation marks: « » for names and quoted text. Punctuation: ، ؛ ؟ . Digits: Western digits (0-9) everywhere.
  No tatweel.
- Plurals: all six CLDR categories (zero, one, two, few 3–10, many 11–99, other).
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, product names, protocols,
  URLs, file names and code. Wrap them in `<bdi>` in templates when they sit next to Arabic text.

## Terms

| English | العربية | Note |
|---|---|---|
| issue | مشكلة (pl. مشكلات) | as in Crashlytics |
| error | خطأ (pl. أخطاء) | VV: «خطأ» |
| event | حدث (pl. أحداث) | VV: «الأحداث» |
| release | إصدار (pl. إصدارات) | same word as app version: in Kestrel a release *is* the app version |
| app version | الإصدار | VV |
| environment | بيئة (pl. بيئات) | |
| resolve / resolved | وضع علامة «تم الحل» / تم الحل | VV: «تم الحل»; button short form «تم الحل» |
| resolve in next release | حل في الإصدار التالي | |
| unresolved | غير محلولة | filter tab «غير المحلولة» (agrees with مشكلة, feminine) |
| ignore / ignored | تجاهل / متجاهَل | VV: «متجاهَل» |
| merge (issues) | دمج | VV: «دمج» |
| regression / regressed | تراجع / أُعيد فتحها | pill «أُعيد فتحها» |
| new / ongoing | جديدة / مستمرة | issue substatus (feminine: مشكلة) |
| crash | عطل (pl. أعطال) | as in Google Play; VV uses «متعطل» (same root) for *down*, which Kestrel doesn't show |
| crash-free sessions / users | جلسات بلا أعطال / مستخدمون بلا أعطال | short label «بلا أعطال» |
| release health | الاستقرار | tab «الاستقرار», heading «استقرار الإصدارات» |
| stack trace | تتبع المكدس | |
| frame / in-app frame | إطار / إطار من التطبيق | toggle «كود التطبيق فقط» |
| breadcrumbs | مسار الأحداث | help text: «الخطوات التي سبقت الخطأ» |
| grouping | التجميع | «سبب التجميع» |
| fingerprint | بصمة | the SDK field `fingerprint` stays as code |
| debug files | ملفات تصحيح الأخطاء | |
| symbolicated / unsymbolicated | مع الرموز / بلا رموز | status labels; the noun stays «symbolication» |
| project | مشروع (pl. مشاريع) | |
| key (project key) | مفتاح (pl. مفاتيح) | «مفتاح المشروع» where it must be specific |
| alert / alert rule | تنبيه / قاعدة تنبيه | VV: «التنبيهات» |
| spike | ارتفاع مفاجئ | alert type; not «الذروة» (peak in VV statistics) |
| first seen / last seen | أول ظهور / آخر ظهور | |
| user (of the app) | مستخدم | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | المستخدمون النشطون (يوميًا / أسبوعيًا / شهريًا) | avoid DAU/WAU/MAU in the UI |
| signal | إشارة (pl. إشارات) | analytics event from an app |
| pageview | مشاهدة صفحة | VV: «مشاهدات الصفحة»; short «المشاهدات» |
| visitor / unique visitors | زائر / الزوار الفريدون | VV |
| referrer | موقع مُحيل (pl. المواقع المُحيلة) | VV |
| session | جلسة (pl. جلسات) | VV: «الجلسات» |
| retention (data) | الاحتفاظ | VV: «الاحتفاظ بـ… لمدة»; never for cohort retention (not in v1) |
| sample / sampling | عيّنة / أخذ العينات | sample rate: «معدل أخذ العينات» |
| tag | وسم (pl. وسوم) | VV: «الوسوم» |
| analytics | التحليلات | VV: «التحليلات» |
| notification channel / channel | قناة إشعارات / قناة | vole-kit: «القنوات»; feminine; plural «قنوات الإشعارات» |
| digest | ملخص | alerts gathered into one message; «إرسال كملخص» |
| cooldown (“at most every … per issue”) | مرة على الأكثر كل … لكل مشكلة | form label «لكل مشكلة، مرة على الأكثر كل» |
| threshold | حد | message field «حد التنبيه» |
| multiplier (× the usual) | أضعاف / × | summary keeps «{k}× المعتاد» |
| adoption | الاعتماد | share of the sessions in the last 7 days; also share of an app's daily active users |
| crash-free rate | نسبة الجلسات بلا أعطال | |
| errored (session) | بأخطاء دون عطل | |
| abnormal (session ended abnormally) | انتهت بشكل غير طبيعي | |
| finalized (release) | نهائي | lowercase status after a date in English (masculine: إصدار) |
| header (webhook) | ترويسة (pl. ترويسات) | vole-kit: «ترويسات مخصصة» |
| instance | التثبيت | as in existing Kestrel strings |
| web analytics / Web tab | تحليلات الويب / الويب | tab «الويب» |
| entry page | صفحة الدخول (pl. صفحات الدخول) | count column «مرات الدخول» |
| screen size | حجم الشاشة (pl. أحجام الشاشات) | |
| device type (desktop / mobile / tablet) | نوع الجهاز (سطح المكتب / الجوال / جهاز لوحي) | |
| operating system | نظام التشغيل (pl. أنظمة التشغيل) | |
| browser | متصفح (pl. متصفحات) | |
| custom event | حدث مخصّص (pl. أحداث مخصّصة) | sent with `kestrel('event', …)` |
| current visitors | الزوار الحاليون | the last 5 minutes |
| direct / none (no referrer) | مباشر / بلا مُحيل | |
| raw events | الأحداث الأولية | «أحداث الويب الأولية» in retention |
| traffic sources (card) | مصادر الزيارات | not «المصادر» alone: that is the keys' allowed origins |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged, in Latin script; prefer «مع الرموز» phrasing in sentences |
| app analytics / App tab | تحليلات التطبيق / التطبيق | tab «التطبيق» |
| install / install id | التثبيت / معرّف التثبيت | a random id per app installation |
| signal type | نوع الإشارة (pl. أنواع الإشارات) | |
| payload | الحمولة (pl. الحمولات) | key/value data sent with a signal |
| signal explorer | مستكشف الإشارات | |
| test signal | إشارة اختبار (pl. إشارات الاختبار) | debug builds, TestFlight, simulator |
| strict mode | الوضع الصارم | the user id changes every day |
| build (app build) | البنية (pl. البنيات) | «بنيات التصحيح» for debug builds |
| version (dimension next to release) | رقم الإصدار (pl. أرقام الإصدار) | where «الإصدار» (release) and the bare version appear side by side |
| device model | طراز الجهاز (pl. طرازات الأجهزة) | |
| locale | اللغة والمنطقة | language and region setting |
| platform | المنصة (pl. المنصات) | |
