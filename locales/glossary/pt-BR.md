# Keen Kestrel – glossary (Portuguese (Brazil), `pt-BR`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/pt-BR.md).

## Estilo
- Português do Brasil. Tratamento: "você", com imperativo natural ("Digite um nome") ou frases impessoais; nunca
  "tu" nem "o senhor".
- Aspas: “…” (e ‘…’ dentro). Maiúscula só no início da frase (sentence case). "e-mail" com hífen, "arquivo", "tela",
  "usuário".
- Plurais: `.one` para 0 e 1, `.many` para múltiplos exatos de 1.000.000 ("{n} de eventos"), `.other` nos demais.
- Termos mantidos em inglês: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.
- Textos curtos e calmos, sem exclamações; botões e abas curtos.

## Terms

| English | Português (Brasil) | Note |
|---|---|---|
| issue | problema (m.) | as in Crashlytics; masculine: “o problema”, “resolvido” |
| error | erro (m.) | VV |
| event | evento (m.) | VV: “Eventos” |
| release | versão (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | versão | VV |
| environment | ambiente (m.) | |
| resolve / resolved | marcar como resolvido / resolvido | VV: “Resolvido”; button short form “Resolvido” |
| resolve in next release | resolver na próxima versão | |
| unresolved | não resolvido | filter tab “Não resolvidos” |
| ignore / ignored | ignorar / ignorado | VV: “Ignorados” |
| merge (issues) | mesclar | VV: “Mesclar” |
| regression / regressed | regressão / reaberto | pill “Reaberto” |
| new / ongoing | novo / em andamento | issue substatus |
| crash | falha (f.) | as in Firebase; verb “falhar”; “o app fechou inesperadamente” in help text |
| crash-free sessions / users | sessões sem falhas / usuários sem falhas | short label “Sem falhas” |
| release health | estabilidade | tab “Estabilidade”, heading “Estabilidade das versões” |
| stack trace | rastreamento de pilha (m.) | “stack trace” is also understood; use one form only |
| frame / in-app frame | quadro / quadro do app | toggle “Só código do app” |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: “os passos antes do erro” |
| grouping | agrupamento | “Por que foram agrupados” |
| fingerprint | impressão digital (f.) | the SDK field `fingerprint` stays as code |
| debug files | arquivos de depuração | |
| symbolicated / unsymbolicated | simbolizado / não simbolizado | status labels; the noun stays “symbolication” |
| project | projeto (m.) | |
| key (project key) | chave (f.) | “chave do projeto” where it must be specific |
| alert / alert rule | alerta (m.) / regra de alerta | VV: “Alertas” |
| spike | pico (m.) | alert type “Pico repentino” in long form |
| first seen / last seen | primeira ocorrência / última ocorrência | short “Primeira” / “Última” in table headers |
| user (of the app) | usuário | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | usuários ativos (diários / semanais / mensais) | avoid DAU/WAU/MAU in the UI |
| signal | sinal (m.; pl. sinais) | analytics event from an app |
| pageview | visualização de página | VV; short “Visualizações” |
| visitor / unique visitors | visitante / visitantes únicos | VV |
| referrer | referenciador (m.) | VV: “Principais referenciadores” |
| session | sessão (f.) | VV |
| retention (data) | retenção (f.) | VV: “Manter … por”; never for cohort retention (not in v1) |
| sample / sampling | amostra / amostragem | sample rate: “taxa de amostragem” |
| tag | tag (f.) | VV |
| analytics | análises | VV: “Análises” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; “o DSN”, “o dSYM”, “o SDK” |
| symbolication | symbolication (f.) | unchanged noun; prefer “simbolizado” phrasing in sentences |
