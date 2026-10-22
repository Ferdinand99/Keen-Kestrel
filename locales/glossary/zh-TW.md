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
| notification channel / channel | 通知管道 / 管道 | vole-kit: 管道; counter 個 |
| digest | 摘要 | alerts gathered into one message; status pill 已併入摘要 |
| cooldown (“at most every … per issue”) | 每個問題最短間隔 | summary: 每個問題最多每 {time} 一次 |
| threshold | 門檻 | spike form reads 每小時超過 [100] 次事件; message field 警報門檻 |
| multiplier (× the usual) | 倍 | 超過平常的 {k} 倍 |
| adoption | 採用率 | share of the sessions in the last 7 days; also share of an app's daily active users |
| crash-free rate | 無當機率 | |
| errored (session) | 有錯誤，未當機 | |
| abnormal (session ended abnormally) | 異常結束 | |
| finalized (release) | 已定版 | lowercase status after a date in English |
| header (webhook) | 標頭 | vole-kit: 自訂標頭 |
| alert (counter) | 則 | {n} 則警報; rules take 條 |
| web analytics / Web tab | 網站分析 / 網站 | tab 網站 |
| entry page | 入口頁面 | count column 進入次數 |
| screen size | 螢幕大小 | |
| device type (desktop / mobile / tablet) | 裝置類型（桌上型電腦 / 行動裝置 / 平板） | |
| operating system | 作業系統 | |
| browser | 瀏覽器 | |
| custom event | 自訂事件 | sent with `kestrel('event', …)` |
| current visitors | 目前訪客 | the last 5 minutes |
| direct / none (no referrer) | 直接 / 無 | |
| raw events | 原始事件 | 原始網站事件 in retention |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer 已符號化 phrasing in sentences |
| app analytics / App tab | App 分析 / App | as in Apple's zh-TW wording; 應用程式 stays in running text elsewhere, but the tab and the product area use the short “App” |
| install / install id | 安裝 / 安裝 ID | a random id per app installation |
| signal type | 訊號類型 | |
| payload | 酬載 | key/value data sent with a signal |
| signal explorer | 訊號探索 | |
| test signal | 測試訊號 | debug builds, TestFlight, simulator |
| strict mode | 嚴格模式 | the user id changes every day |
| build (app build) | 建置版本 | as in App Store Connect; 偵錯建置版本 |
| version (dimension next to release) | 版本號 | where 版本 (release) and the bare version appear side by side |
| device model | 裝置型號 | |
| locale | 語言與地區 | as in Apple's settings |
| platform | 平台 | |
