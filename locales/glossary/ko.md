# Keen Kestrel – glossary (Korean, `ko`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/ko.md).

## Style
- 해요체 for sentences (Apple style), noun-ending labels for buttons, headings and tabs. No exclamation marks.
- Quotes: “ ” (and ‘ ’). Particles are never attached to `{placeholders}`; sentences are rephrased
  (e.g. `“{name}” 해결됨`, `{name}: …`).
- Counts: no plural forms; `이슈 {n}개`. Standard loanword spellings.
- Kept in Latin script: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, and other product and protocol names.

## Terms

| English | 한국어 | Note |
|---|---|---|
| issue | 이슈 | the usual word among Korean developers (GitHub, Jira); Crashlytics says 문제 |
| error | 오류 | VV |
| event | 이벤트 | VV |
| release | 릴리스 | the version string; 버전 is the app version in analytics |
| app version | 버전 | VV |
| environment | 환경 | |
| resolve / resolved | 해결됨으로 표시 / 해결됨 | VV: 해결됨 |
| resolve in next release | 다음 릴리스에서 해결 | |
| unresolved | 미해결 | |
| ignore / ignored | 무시 / 무시됨 | VV: 무시 |
| merge (issues) | 병합 | VV |
| regression / regressed | 회귀 / 재발 | pill 재발 |
| new / ongoing | 신규 / 진행 중 | issue substatus |
| crash | 크래시 | loanword, in line with VV's 인시던트 and 플래핑; Apple says 충돌, Google 비정상 종료 |
| crash-free sessions / users | 크래시 없는 세션 / 크래시 없는 사용자 | short label 크래시 없음 |
| release health | 안정성 | tab 안정성, heading 릴리스 안정성 |
| stack trace | 스택 트레이스 | |
| frame / in-app frame | 프레임 / 앱 내 프레임 | toggle 앱 코드만 |
| breadcrumbs | 브레드크럼 | help text: 오류 전 단계 |
| grouping | 그룹화 | 그룹화된 이유 |
| fingerprint | 핑거프린트 | the SDK field `fingerprint` stays as code |
| debug files | 디버그 파일 | |
| symbolicated / unsymbolicated | 기호화됨 / 기호화 안 됨 | status labels; the noun stays “symbolication” |
| project | 프로젝트 | |
| key (project key) | 키 | 프로젝트 키 where it must be specific |
| alert / alert rule | 알림 / 알림 규칙 | VV uses 알림 for both alerts and notifications |
| spike | 급증 | alert type |
| first seen / last seen | 최초 발생 / 최근 발생 | |
| user (of the app) | 사용자 | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | 활성 사용자(일간 / 주간 / 월간) | avoid DAU/WAU/MAU in the UI |
| signal | 시그널 | analytics event from an app |
| pageview | 페이지 조회 | VV: 페이지 조회수; short 조회수 |
| visitor / unique visitors | 방문자 / 순 방문자 | VV |
| referrer | 리퍼러 | VV; 유입 사이트 in descriptions |
| session | 세션 | VV |
| retention (data) | 보관 기간 | VV; never for cohort retention (not in v1) |
| sample / sampling | 샘플 / 샘플링 | sample rate: 샘플링 비율 |
| tag | 태그 | VV |
| analytics | 분석 | VV: 분석 |
| notification channel / channel | 알림 채널 / 채널 | vole-kit: 채널 |
| digest | 요약 | alerts gathered into one message; status pill 요약에 포함 |
| cooldown (“at most every … per issue”) | 이슈당 최소 알림 간격 | summary: 이슈당 최소 간격 {time} (no particle on the placeholder) |
| threshold | 기준 | spike form reads 시간당 이벤트 [100] 개 초과; message field 알림 기준 |
| multiplier (× the usual) | 배 | 평소의 {k}배 초과 |
| adoption | 채택률 | share of the sessions in the last 7 days |
| crash-free rate | 크래시 없는 비율 | |
| errored (session) | 오류 있음, 크래시 없음 | |
| abnormal (session ended abnormally) | 비정상 종료 | |
| finalized (release) | 확정됨 | lowercase status after a date in English |
| header (webhook) | 헤더 | vole-kit: 사용자 지정 헤더 |
| web analytics / Web tab | 웹 분석 / 웹 | tab 웹 |
| entry page | 진입 페이지 | count column 진입 수 |
| screen size | 화면 크기 | |
| device type (desktop / mobile / tablet) | 기기 유형(데스크톱 / 모바일 / 태블릿) | |
| operating system | 운영체제 | |
| browser | 브라우저 | |
| custom event | 맞춤 이벤트 | sent with `kestrel('event', …)` |
| current visitors | 현재 방문자 | the last 5 minutes |
| direct / none (no referrer) | 직접 / 없음 | |
| raw events | 원시 이벤트 | 원시 웹 이벤트 in retention |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README |
| symbolication | symbolication | unchanged noun; prefer 기호화됨 phrasing in sentences |
