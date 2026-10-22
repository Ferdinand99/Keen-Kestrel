# Keen Kestrel – glossary (Hebrew, `he`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/he.md).

## Style
- Right-to-left. Modern Hebrew, gender-neutral: infinitive (להזין, לשמור) for buttons and actions; impersonal or noun
  phrases for sentences; masculine singular only when unavoidable.
- Button labels as nouns where VV does so: Save = שמירה, Cancel = ביטול, Delete = מחיקה.
- Quotation marks “ ” as in English; no niqqud; Western digits; abbreviations with geresh ׳.
- No capitals. Mixed-script strings keep Latin terms as written; wrap them in `<bdi>` in templates.
- Plurals: one (exactly 1), two (exactly 2), other.
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, Webhook, and other technical terms
  and product names.

## Terms

| English | עברית | Note |
|---|---|---|
| issue | בעיה (pl. בעיות) | as in Crashlytics |
| error | שגיאה | VV: “שגיאה” |
| event | אירוע (pl. אירועים) | VV: “אירועים” (incidents are תקרית, so no clash) |
| release | גרסה (pl. גרסאות) | same word as app version: in Kestrel a release *is* the app version |
| app version | גרסה | VV |
| environment | סביבה | |
| resolve / resolved | סימון כנפתרה / נפתרה | VV: “נפתר”; feminine here because it agrees with בעיה |
| resolve in next release | נפתרה בגרסה הבאה | |
| unresolved | לא נפתרו | filter tab “לא נפתרו” |
| ignore / ignored | התעלמות / בהתעלמות | VV: “מתעלמים” |
| merge (issues) | מיזוג | VV: “מיזוג” |
| regression / regressed | רגרסיה / נפתחה מחדש | pill “נפתחה מחדש” |
| new / ongoing | חדשה / נמשכת | issue substatus (feminine: בעיה) |
| crash | קריסה (pl. קריסות) | |
| crash-free sessions / users | סשנים ללא קריסות / משתמשים ללא קריסות | short label “ללא קריסות” |
| release health | יציבות | tab “יציבות”, heading “יציבות גרסאות” |
| stack trace | מחסנית קריאות | “stack trace” is common among developers; use one form only |
| frame / in-app frame | מסגרת / מסגרת של האפליקציה | toggle “רק קוד האפליקציה” |
| breadcrumbs | פירורי לחם | help text: “השלבים שלפני השגיאה” |
| grouping | קיבוץ | “למה קובצו” |
| fingerprint | טביעת אצבע | the SDK field `fingerprint` stays as code |
| debug files | קובצי ניפוי באגים | |
| symbolicated / unsymbolicated | עם סמלים / ללא סמלים | status labels; the noun stays “symbolication” |
| project | פרויקט | |
| key (project key) | מפתח | “מפתח הפרויקט” where it must be specific |
| alert / alert rule | התראה / כלל התראה | VV: “התראות” (same word as notification) |
| spike | זינוק | alert type |
| first seen / last seen | נראה לראשונה / נראה לאחרונה | |
| user (of the app) | משתמש | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | משתמשים פעילים (יומי / שבועי / חודשי) | avoid DAU/WAU/MAU in the UI |
| signal | אות (pl. אותות) | analytics event from an app |
| pageview | צפייה בדף | VV: “צפיות בדף”; short “צפיות” |
| visitor / unique visitors | מבקר / מבקרים ייחודיים | VV |
| referrer | מפנה (pl. מפנים) | VV: “אתרים מפנים” |
| session | סשן (pl. סשנים) | VV: “סשנים” |
| retention (data) | שמירה | VV: “שמירת …”; never for cohort retention (not in v1) |
| sample / sampling | דגימה / דגימה | sample rate: “קצב דגימה” |
| tag | תגית | VV: “תגיות” |
| analytics | אנליטיקה | VV: “אנליטיקה” |
| notification channel / channel | ערוץ התראות / ערוץ | vole-kit: “ערוץ”; plural “ערוצי התראות” |
| digest | סיכום | alerts gathered into one message; “שליחה כסיכום” |
| cooldown (“at most every … per issue”) | לכל היותר פעם ב-… לכל בעיה | form label “לכל בעיה, לכל היותר פעם ב-” |
| threshold | סף | message field “סף התראה” |
| multiplier (× the usual) | פי | “פי {k} מהרגיל” |
| adoption | אימוץ | share of the sessions in the last 7 days |
| crash-free rate | שיעור ללא קריסות | |
| errored (session) | עם שגיאות, בלי קריסה | |
| abnormal (session ended abnormally) | הסתיימו באופן חריג | plural, agrees with סשנים |
| finalized (release) | סופית | lowercase status after a date in English (feminine: גרסה) |
| header (webhook) | כותרת | vole-kit: “כותרות מותאמות” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged, in Latin script; prefer “עם סמלים” phrasing in sentences |
