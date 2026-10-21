# Keen Kestrel – glossary (French, `fr`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/fr.md).

## Style
- Adresse : neutre, sans « tu » ni « vous » quand c’est naturel (infinitifs et groupes nominaux pour boutons et
  consignes : « Enregistrer », « Saisir un nom »). Si un pronom est inévitable, tournure impersonnelle. Ton de
  logiciel moderne (style Apple France).
- Guillemets « texte » avec espace fine insécable (U+202F) à l’intérieur ; espace fine insécable (U+202F) avant ; ! ?
  et espace insécable (U+00A0) avant :. Apostrophe typographique ’.
- Casse de phrase : majuscule initiale seulement (pas de majuscules de titre).
- Pluriels : `.one` (0 et 1), `.many` (multiples exacts de 1 000 000 : « {n} d’événements »), `.other`.
- Termes conservés en anglais : DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook.

## Terms

| English | Français | Note |
|---|---|---|
| issue | problème (m.) | as in Crashlytics; « ticket » would suggest a support request |
| error | erreur (f.) | VV : « Erreur » |
| event | événement (m.) | VV : « Événements » |
| release | version (f.) | same word as app version: in Kestrel a release *is* the app version |
| app version | version | VV |
| environment | environnement (m.) | |
| resolve / resolved | marquer comme résolu / résolu | VV : « Résolu »; button short form « Résolu » |
| resolve in next release | résolu dans la prochaine version | |
| unresolved | non résolu | filter tab « Non résolus » |
| ignore / ignored | ignorer / ignoré | VV : « Ignorées » (f. there, m. here: le problème) |
| merge (issues) | fusionner | VV |
| regression / regressed | régression / rouvert | pill « Rouvert » |
| new / ongoing | nouveau / en cours | issue substatus |
| crash | plantage (m.) | as in Apple’s French tools; verb « planter » |
| crash-free sessions / users | sessions sans plantage / utilisateurs sans plantage | short label « Sans plantage » |
| release health | stabilité | tab « Stabilité », heading « Stabilité des versions » |
| stack trace | trace de pile (f.) | |
| frame / in-app frame | frame (m.) / frame de l’app | toggle « Code de l’app seulement » |
| breadcrumbs | fil d’Ariane (m.) | the usual French word for breadcrumbs; help text: « les étapes avant l’erreur » |
| grouping | regroupement (m.) | « Pourquoi regroupés » |
| fingerprint | empreinte (f.) | the SDK field `fingerprint` stays as code |
| debug files | fichiers de débogage | |
| symbolicated / unsymbolicated | symbolisé / non symbolisé | status labels; the noun stays « symbolication » |
| project | projet (m.) | |
| key (project key) | clé (f.) | « clé du projet » where it must be specific |
| alert / alert rule | alerte (f.) / règle d’alerte | VV : « Alertes » |
| spike | pic (m.) | alert type « Pic soudain » in long form |
| first seen / last seen | première occurrence / dernière occurrence | short « Premier » / « Dernier » in table headers |
| user (of the app) | utilisateur | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | utilisateurs actifs (par jour / semaine / mois) | avoid DAU/WAU/MAU in the UI |
| signal | signal (m.; pl. signaux) | analytics event from an app |
| pageview | page vue (pl. pages vues) | VV : « pages vues / vues »; short « Vues » |
| visitor / unique visitors | visiteur / visiteurs uniques | VV |
| referrer | référent (m.) | VV : « Principaux référents » |
| session | session (f.) | VV |
| retention (data) | conservation (f.) | VV; never for cohort retention (not in v1) |
| sample / sampling | échantillon / échantillonnage | sample rate: « taux d’échantillonnage » |
| tag | étiquette (f.) | VV : « Étiquettes » |
| analytics | statistiques | VV : « Statistiques » |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; « le DSN », « le dSYM », « le SDK » |
| symbolication | symbolication (f.) | unchanged noun; prefer « symbolisé » phrasing in sentences |
