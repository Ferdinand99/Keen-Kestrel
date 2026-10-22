# Keen Kestrel – glossary (Japanese, `ja`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/ja.md).

## Style
- Polite but concise: noun-ending labels for buttons and headings; です/ます for sentences. Addressing the reader is
  avoided (no お客様, no あなた).
- Punctuation: 。、 and 「」 for quotes (English “ ” become 「」); full-width ！？ only where English has them. No spaces
  between Japanese and Latin/digits.
- Katakana loanwords with long vowel ー (ユーザー, フィンガープリント).
- Plural categories: other only.
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, Webhook, and other product and protocol
  names.

## Terms

| English | 日本語 | Note |
|---|---|---|
| issue | 問題 | as in Crashlytics; 「イシュー」 is common among developers but less clear |
| error | エラー | VV |
| event | イベント | VV: 「イベント」 |
| release | リリース | the version string; 「バージョン」 is the app version in analytics |
| app version | バージョン | VV |
| environment | 環境 | |
| resolve / resolved | 解決済みにする / 解決済み | VV: 「解決済み」 |
| resolve in next release | 次のリリースで解決 | |
| unresolved | 未解決 | |
| ignore / ignored | 無視 / 無視中 | VV: 「無視」 |
| merge (issues) | マージ | VV: 「マージ」 |
| regression / regressed | リグレッション / 再発 | pill 「再発」 |
| new / ongoing | 新規 / 継続中 | issue substatus |
| crash | クラッシュ | |
| crash-free sessions / users | クラッシュフリーのセッション / ユーザー | short label 「クラッシュフリー」 |
| release health | 安定性 | tab 「安定性」, heading 「リリースの安定性」 |
| stack trace | スタックトレース | |
| frame / in-app frame | フレーム / アプリ内フレーム | toggle 「アプリのコードのみ」 |
| breadcrumbs | ブレッドクラム | help text: 「エラー前の操作」 |
| grouping | グループ化 | 「グループ化の理由」 |
| fingerprint | フィンガープリント | the SDK field `fingerprint` stays as code |
| debug files | デバッグファイル | |
| symbolicated / unsymbolicated | シンボル化済み / 未シンボル化 | status labels, as in Apple's Japanese docs; the noun stays 「symbolication」 |
| project | プロジェクト | |
| key (project key) | キー | 「プロジェクトキー」 where it must be specific |
| alert / alert rule | アラート / アラートルール | VV: 「アラート」 |
| spike | 急増 | alert type |
| first seen / last seen | 初回発生 / 最終発生 | |
| user (of the app) | ユーザー | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | アクティブユーザー（日次 / 週次 / 月次） | avoid DAU/WAU/MAU in the UI |
| signal | シグナル | analytics event from an app |
| pageview | ページビュー | VV; short 「閲覧数」 |
| visitor / unique visitors | 訪問者 / ユニーク訪問者 | VV |
| referrer | リファラー | VV: 「上位のリファラー」 |
| session | セッション | VV |
| retention (data) | 保持期間 | VV; never for cohort retention (not in v1) |
| sample / sampling | サンプル / サンプリング | sample rate: 「サンプルレート」 |
| tag | タグ | VV |
| analytics | アナリティクス | VV: 「アナリティクス」 |
| notification channel / channel | 通知チャンネル / チャンネル | vole-kit: 「チャンネル」 |
| digest | ダイジェスト | alerts gathered into one message; status pill 「ダイジェスト内」 |
| cooldown (“at most every … per issue”) | 問題ごとの最短間隔 | summary: 「問題ごとに{time}に1回まで」 |
| threshold | しきい値 | spike form reads 「1時間に [100] 件を超えるイベント」; message field 「アラートのしきい値」 |
| multiplier (× the usual) | 倍 | 「通常の{k}倍超」; the form shows 「かつ平均の [5] ×」 |
| adoption | 普及率 | share of the sessions in the last 7 days |
| crash-free rate | クラッシュフリー率 | |
| errored (session) | エラーあり、クラッシュなし | |
| abnormal (session ended abnormally) | 異常終了 | |
| finalized (release) | 確定済み | lowercase status after a date in English |
| header (webhook) | ヘッダー | vole-kit: 「カスタムヘッダー」 |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer 「シンボル化済み」 phrasing in sentences |
