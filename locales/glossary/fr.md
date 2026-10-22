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
| notification channel / channel | canal de notification / canal (m.; pl. canaux) | VV: « Canaux de notification » |
| digest | récapitulatif (m.) | « Envoyer un récapitulatif »; status pill « Dans le récapitulatif » |
| cooldown (at most every … per issue) | intervalle minimal | « Par problème, intervalle minimal de [1 h] »; avoids « toutes les 1 h » |
| digest interval (one message every …) | un message par période de … | same reason as above |
| threshold / alert above | seuil (m.) / seuil d’alerte | form label « Plus de [100] événements par heure » |
| adoption | adoption (f.) | share of the last 7 days’ sessions; also share of an app’s daily active users |
| errored (session) | avec erreurs, sans plantage | session row label |
| abnormal (session ended abnormally) | fin anormale | session row label |
| crashed (session) | plantée | « Plantées » as row label (f.: session) |
| finalized (release) | finalisée | lowercase status after a date |
| culprit (“Where” in alert messages) | origine (f.) | as in the issue search help |
| skipped (alert delivery) | non envoyée | not « ignorée », which is the issue status |
| attempt (delivery) | tentative (f.) | « Les envois échoués sont retentés » |
| header (HTTP) | en-tête (m.) | VV |
| AI assistant | assistant IA | |
| entry page | page d’entrée (f.) | tab « Pages d’entrée » |
| screen size | taille d’écran (f.) | tab « Tailles d’écran » |
| device type (desktop / mobile / tablet) | type d’appareil (ordinateur / mobile / tablette) | card « Appareils » |
| operating system | système d’exploitation (m.) | tab short form « Systèmes » |
| browser | navigateur (m.) |  |
| custom event | événement personnalisé (m.) |  |
| current visitors | visiteurs en ce moment | stat label; meta « 5 dernières minutes » |
| direct / none (no referrer) | Direct / aucun | list row for visits without a referrer |
| raw events | événements bruts | kept for filters; retention « Conserver les événements web bruts » |
| snippet (k.js) | script (m.) | « le script k.js »; avoids « extrait » |
| UTM source / medium / campaign | source / support / campagne UTM |  |
| app analytics | statistiques d’app | tab « App »; page title « Statistiques d’app de … » |
| install / install id | installation (f.) / identifiant d’installation | a random id per app installation |
| signal type | type de signal (m.) | `App.launched` and the like stay as code |
| payload | charge utile (f.) | the key/value data an app sends with a signal |
| signal explorer | explorateur de signaux (m.) | nav item short « Explorateur » |
| test signal | signal de test (m.) | from debug builds, TestFlight and the simulator |
| strict mode | mode strict (m.) | |
| build (app build) | build (m.) | « build de débogage » |
| device model | modèle d’appareil (m.) | |
| locale | langue et région | as Apple’s « Langue et région »; card « Langues » |
| platform | plateforme (f.) | |
| OS version | version du système (f.) | |
| release (as an analytics dimension) | version complète | dimension label, to tell `bundle@version+build` apart from « Version » (app version) |
