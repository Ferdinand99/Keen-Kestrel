# Keen Kestrel – glossary (Chinese (Simplified), `zh-CN`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/zh-CN.md).

## Style

- 简体中文，使用中国大陆术语；中性、简洁，标签中不用“你/您”，句子中尽量省略主语，必要时用“你”。
- 全角标点：，。：；？！；引号用 “ ” 和 ‘ ’；省略号 `…` 保留；破折号 —。
- 中文与拉丁字母、数字之间不加空格（Apple 风格）；占位符与中文相邻也不加空格，除非为分隔点“ · ”。
- 保持原文：DSN、dSYM、SDK、`sentry-cli`、symbolication、JSON、UTM，以及其他产品名、协议和技术术语。
- 无复数形式，`{n}` 后接量词：个（问题、项目、密钥、信号）、次（事件、崩溃、会话）、条（规则）、位（访客、用户）。

## Terms

| English | 简体中文 | Note |
|---|---|---|
| issue | 问题 | as in Crashlytics; counter 个 |
| error | 错误 | VV |
| event | 事件 | VV uses 事件 for both incidents and monitor events; Kestrel has no incidents, so no clash here. Counter 次 |
| release | 版本 | same word as app version: in Kestrel a release *is* the app version |
| app version | 版本 | VV |
| environment | 环境 | |
| resolve / resolved | 标记为已解决 / 已解决 | VV: 已解决 |
| resolve in next release | 在下一版本中解决 | |
| unresolved | 未解决 | |
| ignore / ignored | 忽略 / 已忽略 | VV: 忽略 |
| merge (issues) | 合并 | VV |
| regression / regressed | 回归 / 已复发 | pill 已复发 |
| new / ongoing | 新 / 持续中 | issue substatus |
| crash | 崩溃 | |
| crash-free sessions / users | 无崩溃会话 / 无崩溃用户 | short label 无崩溃 |
| release health | 稳定性 | tab 稳定性, heading 版本稳定性 |
| stack trace | 堆栈跟踪 | |
| frame / in-app frame | 帧 / 应用内帧 | toggle 仅应用代码 |
| breadcrumbs | 面包屑 | help text: 出错前的操作 |
| grouping | 分组 | 分组原因 |
| fingerprint | 指纹 | the SDK field `fingerprint` stays as code |
| debug files | 调试文件 | |
| symbolicated / unsymbolicated | 已符号化 / 未符号化 | status labels, as in Apple's Chinese docs; the noun stays “symbolication” |
| project | 项目 | |
| key (project key) | 密钥 | 项目密钥 where it must be specific; VV also uses 密钥 for secrets, but a project key is public by design |
| alert / alert rule | 告警 / 告警规则 | VV: 告警 |
| spike | 激增 | alert type |
| first seen / last seen | 首次出现 / 最近出现 | |
| user (of the app) | 用户 | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | 活跃用户（日 / 周 / 月） | 日活 / 周活 / 月活 are common shorthand but avoid them in the UI |
| signal | 信号 | analytics event from an app |
| pageview | 页面浏览 | VV: 页面浏览量; short 浏览量 |
| visitor / unique visitors | 访客 / 独立访客 | VV |
| referrer | 来源网站 | VV |
| session | 会话 | VV |
| retention (data) | 保留时长 | VV: 保留; never for cohort retention (留存, not in v1) |
| sample / sampling | 样本 / 采样 | sample rate: 采样率 |
| tag | 标签 | VV |
| analytics | 统计 | VV: 统计 |
| notification channel / channel | 通知渠道 / 渠道 | vole-kit: 渠道; counter 个 |
| digest | 摘要 | alerts gathered into one message; status pill 已入摘要 |
| cooldown (“at most every … per issue”) | 每个问题最短间隔 | summary: 每个问题最多每{time}一次 |
| threshold | 阈值 | spike form reads 每小时超过 [100] 次事件; message field 告警阈值 |
| multiplier (× the usual) | 倍 | 超过平常的{k}倍 |
| adoption | 采用率 | share of the sessions in the last 7 days |
| crash-free rate | 无崩溃率 | |
| errored (session) | 有错误，未崩溃 | |
| abnormal (session ended abnormally) | 异常结束 | |
| finalized (release) | 已定版 | lowercase status after a date in English |
| header (webhook) | 请求头 | vole-kit: 自定义请求头 |
| alert (counter) | 条 | {n}条告警; rules also take 条 |
| web analytics / Web tab | 网站统计 / 网站 | tab 网站 |
| entry page | 入口页面 | count column 入口次数 |
| screen size | 屏幕尺寸 | |
| device type (desktop / mobile / tablet) | 设备类型（桌面端 / 移动端 / 平板） | |
| operating system | 操作系统 | |
| browser | 浏览器 | |
| custom event | 自定义事件 | sent with `kestrel('event', …)` |
| current visitors | 当前访客 | the last 5 minutes |
| direct / none (no referrer) | 直接访问 / 无 | |
| raw events | 原始事件 | 原始网站事件 in retention |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer 已符号化 phrasing in sentences |
