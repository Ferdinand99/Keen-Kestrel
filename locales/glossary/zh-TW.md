# Keen Kestrel – glossary (Chinese (Traditional, Taiwan), `zh-TW`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/zh-TW.md).

## Style
- Neutral and concise: labels omit the subject, "你" only where a sentence needs it (e.g. "你的").
- Taiwan MOE punctuation: full-width ，。：；？！, quotation marks 「 」 (nested 『 』), ellipsis ….
- A space separates digits/Latin from Chinese (`{n} 個問題`, `{n} 秒`), except around full-width punctuation.
- Taiwan vocabulary (伺服器, 軟體, 網路, 預設, 使用者, 設定, 資料庫, 應用程式), not a conversion of Simplified.
- Where English uses “ ” around a name, use 「 」.
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, Webhook, and other product and
  protocol names.

## Terms

| English | 繁體中文 | Note |
|---|---|---|
| issue | 問題 | as in Crashlytics; counter 個 |
| error | 錯誤 | VV |
| event | 事件 | VV uses 事件 for status-page incidents and 活動紀錄 for monitor events; Kestrel has no incidents, and 事件 is the usual word for an error event, so it is used here deliberately |
| release | 版本 | same word as app version: in Kestrel a release *is* the app version |
| app version | 版本 | VV |
| environment | 環境 | |
| resolve / resolved | 標為已解決 / 已解決 | VV: 標為已解決 / 已解決 |
| resolve in next release | 在下一版中解決 | |
| unresolved | 未解決 | |
| ignore / ignored | 忽略 / 已忽略 | VV: 已忽略 |
| merge (issues) | 合併 | VV |
| regression / regressed | 迴歸 / 再次發生 | pill 再次發生 |
| new / ongoing | 新 / 持續中 | issue substatus |
| crash | 當機 | Taiwan usage, as in Apple's and Google's zh-TW tools; not 崩潰 (Mainland) |
| crash-free sessions / users | 無當機工作階段 / 無當機使用者 | short label 無當機 |
| release health | 穩定性 | tab 穩定性, heading 版本穩定性 |
| stack trace | 堆疊追蹤 | |
| frame / in-app frame | 框架 / 應用程式內框架 | toggle 僅限應用程式碼 |
| breadcrumbs | 麵包屑 | help text: 發生錯誤前的操作 |
| grouping | 分組 | 分組原因 |
| fingerprint | 指紋 | the SDK field `fingerprint` stays as code |
| debug files | 偵錯檔案 | |
| symbolicated / unsymbolicated | 已符號化 / 未符號化 | status labels; the noun stays “symbolication” |
| project | 專案 | |
| key (project key) | 金鑰 | 專案金鑰 where it must be specific |
| alert / alert rule | 警報 / 警報規則 | VV: 警報 |
| spike | 激增 | alert type |
| first seen / last seen | 首次出現 / 最近出現 | |
| user (of the app) | 使用者 | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | 活躍使用者（每日 / 每週 / 每月） | avoid DAU/WAU/MAU in the UI |
| signal | 訊號 | analytics event from an app |
| pageview | 頁面瀏覽 | VV: 頁面瀏覽次數; short 瀏覽次數 |
| visitor / unique visitors | 訪客 / 不重複訪客 | VV |
| referrer | 參照來源 | VV |
| session | 工作階段 | VV: 工作階段 (also the Google Analytics word) |
| retention (data) | 保留時間 | VV: 保留; never for cohort retention (not in v1) |
| sample / sampling | 樣本 / 取樣 | sample rate: 取樣率 |
| tag | 標籤 | VV |
| analytics | 分析 | VV: 分析 |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer 已符號化 phrasing in sentences |
