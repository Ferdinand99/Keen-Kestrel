# Keen Kestrel – glossary (Portuguese (Portugal), `pt-PT`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/pt-PT.md).

## Estilo
- Português europeu, Acordo Ortográfico de 1990.
- Registo neutro: infinitivo ou terceira pessoa («Introduza o código»); nunca «tu», nunca gerúndio brasileiro
  («a carregar»).
- Aspas: « » (interiores “ ”). Reticências «…» onde o inglês as tem.
- Maiúscula inicial só em frases; meses e dias em minúscula.
- Vocabulário europeu: ecrã, ficheiro, utilizador, definições, eliminar, aceder, palavra-passe, partilhar, registo,
  separador, aplicação.
- Plurais: `.one` (exatamente 1), `.many` (múltiplos exatos de 1 000 000), `.other`.
- Termos mantidos em inglês: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Português (PT) | Note |
|---|---|---|
| issue | problema (m.) | as in Crashlytics |
| error | erro (m.) | VV |
| event | evento (m.) | VV: «Eventos» |
| release | versão (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | versão | VV |
| environment | ambiente (m.) | |
| resolve / resolved | marcar como resolvido / resolvido | VV: «Resolvido»; button short form «Resolvido» |
| resolve in next release | resolver na próxima versão | |
| unresolved | não resolvido | filter tab «Não resolvidos» |
| ignore / ignored | ignorar / ignorado | VV: «Ignorados» |
| merge (issues) | juntar | VV uses «Intercalar» for merging backups; for issues «Juntar» reads better |
| regression / regressed | regressão / reaberto | pill «Reaberto» |
| new / ongoing | novo / em curso | issue substatus |
| crash | falha (f.) | as in Google Play; VV uses «falha» for the *outage* status, which Kestrel doesn’t have |
| crash-free sessions / users | sessões sem falhas / utilizadores sem falhas | short label «Sem falhas» |
| release health | estabilidade | tab «Estabilidade», heading «Estabilidade das versões» |
| stack trace | rastreio da pilha (m.) | |
| frame / in-app frame | quadro / quadro da aplicação | toggle «Só código da aplicação» |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: «os passos antes do erro» |
| grouping | agrupamento | «Porque foram agrupados» |
| fingerprint | impressão digital (f.) | the SDK field `fingerprint` stays as code |
| debug files | ficheiros de depuração | |
| symbolicated / unsymbolicated | simbolizado / não simbolizado | status labels; the noun stays «symbolication» |
| project | projeto (m.) | |
| key (project key) | chave (f.) | «chave do projeto» where it must be specific |
| alert / alert rule | alerta (m.) / regra de alerta | VV: «Alertas» |
| spike | pico (m.) | alert type «Pico repentino» in long form |
| first seen / last seen | primeira ocorrência / última ocorrência | short «Primeira» / «Última» in table headers |
| user (of the app) | utilizador | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | utilizadores ativos (diários / semanais / mensais) | avoid DAU/WAU/MAU in the UI |
| signal | sinal (m.; pl. sinais) | analytics event from an app |
| pageview | visualização de página | VV; short «Visualizações» |
| visitor / unique visitors | visitante / visitantes únicos | VV |
| referrer | referenciador (m.) | VV |
| session | sessão (f.) | VV |
| retention (data) | retenção (f.) | VV: «Manter … durante»; never for cohort retention (not in v1) |
| sample / sampling | amostra / amostragem | sample rate: «taxa de amostragem» |
| tag | etiqueta (f.) | VV: «Etiquetas» |
| analytics | análises | VV: «Análises» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; «o DSN», «o dSYM», «o SDK» |
| symbolication | symbolication (f.) | unchanged noun; prefer «simbolizado» phrasing in sentences |
| notification channel / channel | canal de notificação / canal (m.; pl. canais) | VV: «Canais de notificação» |
| digest | resumo (m.) | «Enviar como resumo»; status pill «No resumo» |
| cooldown (at most every … per issue) | no máximo a cada … | «Por problema, no máximo a cada [1 h]» |
| threshold / alert above | limite (m.) / limite do alerta | form label «Mais de [100] eventos por hora» |
| adoption | adoção (f.) | share of the last 7 days’ sessions; also share of an app’s daily active users |
| errored (session) | com erros, sem falha | session row label |
| abnormal (session ended abnormally) | fim anómalo | session row label |
| crashed (session) | com falha | session row label |
| finalized (release) | finalizada | lowercase status after a date |
| culprit (“Where” in alert messages) | origem (f.) | as in the issue search help |
| failed (alert delivery) | não enviado | avoids «falha», which means crash here |
| skipped (alert delivery) | omitido | not «ignorado», which is the issue status |
| attempt (delivery) | tentativa (f.) | |
| link | ligação (f.) | alert message field label |
| header (HTTP) | cabeçalho (m.) | VV |
| AI assistant | assistente de IA | |
| entry page | página de entrada (f.) | tab «Páginas de entrada» |
| screen size | tamanho de ecrã (m.) | tab «Tamanhos de ecrã» |
| device type (desktop / mobile / tablet) | tipo de dispositivo (computador / telemóvel / tablet) | card «Dispositivos» |
| operating system | sistema operativo (m.) | tab short form «Sistemas» |
| browser | navegador (m.) |  |
| custom event | evento personalizado (m.) |  |
| current visitors | visitantes agora | stat label; meta «Últimos 5 minutos» |
| direct / none (no referrer) | Direto / nenhum | list row for visits without a referrer |
| raw events | eventos em bruto | kept for filters |
| snippet (k.js) | script (m.) / código | «o script k.js», «o código abaixo» |
| UTM source / medium / campaign | fonte / meio / campanha UTM |  |
| app analytics | análises de aplicação | tab «App» (short, as in App Store); «aplicação» in sentences |
| install / install id | instalação (f.) / ID de instalação (m.) | a random id per app installation |
| signal type | tipo de sinal (m.) | `App.launched` and the like stay as code |
| payload | carga útil (f.) | the key/value data an app sends with a signal |
| signal explorer | explorador de sinais (m.) | nav item short «Explorador» |
| test signal | sinal de teste (m.) | from debug builds, TestFlight and the simulator |
| strict mode | modo estrito (m.) | |
| build (app build) | compilação (f.) | «compilação de depuração» |
| device model | modelo de dispositivo (m.) | |
| locale | idioma e região | as Apple’s «Idioma e região»; card «Idiomas» |
| platform | plataforma (f.) | |
| OS version | versão do sistema (f.) | |
| release (as an analytics dimension) | versão completa | dimension label, to tell `bundle@version+build` apart from «Versão» (app version) |
