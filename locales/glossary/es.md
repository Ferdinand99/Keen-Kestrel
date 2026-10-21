# Keen Kestrel – glossary (Spanish, `es`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/es.md).

## Estilo
- Español neutro internacional.
- Tratamiento: tuteo informal mediante imperativos («Introduce…», «Elige…») o frases impersonales; nunca «usted»,
  «vosotros» ni «vos».
- Comillas: « » (y “ ” dentro); signos ¿? y ¡! de apertura donde corresponda.
- Mayúscula solo inicial (frase); los meses y días en minúscula.
- Plurales: `.one`, `.many` (múltiplos exactos de 1.000.000, «de eventos») y `.other`.
- Sin traducir: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.
- Textos de botones y pestañas cortos; sin signos de exclamación salvo en el original.

## Terms

| English | Español | Note |
|---|---|---|
| issue | problema (m.) | as in Crashlytics; masculine: «el problema», «resuelto» |
| error | error (m.) | VV |
| event | evento (m.) | VV: «Eventos» |
| release | versión (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | versión | VV |
| environment | entorno (m.) | |
| resolve / resolved | marcar como resuelto / resuelto | VV: «Resuelto»; button short form «Resuelto» |
| resolve in next release | resolver en la próxima versión | |
| unresolved | sin resolver | filter tab «Sin resolver» |
| ignore / ignored | ignorar / ignorado | VV: «Ignorados» |
| merge (issues) | combinar | VV: «Combinar» |
| regression / regressed | regresión / reabierto | pill «Reabierto» |
| new / ongoing | nuevo / en curso | issue substatus |
| crash | fallo (m.) | «cierre inesperado» in longer help text; verb «fallar» |
| crash-free sessions / users | sesiones sin fallos / usuarios sin fallos | short label «Sin fallos» |
| release health | estabilidad | tab «Estabilidad», heading «Estabilidad de versiones» |
| stack trace | seguimiento de pila (m.) | |
| frame / in-app frame | marco / marco de la app | toggle «Solo código de la app» |
| breadcrumbs | breadcrumbs | Sentry term kept; help text: «los pasos antes del error» |
| grouping | agrupación | «Por qué se agruparon» |
| fingerprint | huella (f.) | the SDK field `fingerprint` stays as code |
| debug files | archivos de depuración | |
| symbolicated / unsymbolicated | simbolizado / sin simbolizar | status labels; the noun stays «symbolication» |
| project | proyecto (m.) | |
| key (project key) | clave (f.) | «clave del proyecto» where it must be specific |
| alert / alert rule | alerta (f.) / regla de alerta | VV: «Alertas» |
| spike | pico (m.) | alert type «Pico repentino» in long form |
| first seen / last seen | primera vez / última vez | table headers; «visto por primera vez» in sentences |
| user (of the app) | usuario | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | usuarios activos (diarios / semanales / mensuales) | avoid DAU/WAU/MAU in the UI |
| signal | señal (f.) | analytics event from an app |
| pageview | visualización de página | VV; short «Visualizaciones» |
| visitor / unique visitors | visitante / visitantes únicos | VV |
| referrer | referente (m.) | VV: «Principales referentes» |
| session | sesión (f.) | VV |
| retention (data) | conservación (f.) | VV; never for cohort retention (not in v1) |
| sample / sampling | muestra / muestreo | sample rate: «tasa de muestreo» |
| tag | etiqueta (f.) | VV: «Etiquetas» |
| analytics | analítica | VV: «Analítica» |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; «el DSN», «el dSYM», «el SDK» |
| symbolication | symbolication (f.) | unchanged noun; prefer «simbolizado» phrasing in sentences |
