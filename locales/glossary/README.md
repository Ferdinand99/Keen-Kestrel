# Translation glossaries

One file per interface language, named after its code (`de.md`, `pt-BR.md`, `zh-TW.md` …). Each file fixes the
words Keen Kestrel uses for its own concepts – issue, event, release, crash, signal, pageview and so on – and the
tone of the language, so that every string says the same thing the same way. They play the same role as
[Vigilant Vole's glossaries](https://github.com/Ferdinand99/Vigilant-Vole/tree/main/locales/glossary).

All files are drafts that native speakers have not reviewed yet. Corrections are very welcome: change the term in
the glossary and in every string that uses it, in the same pull request.

## How they are used

- **Kestrel's own strings** (issues, releases, symbolication, analytics, alert rules …) are written from English,
  one language at a time, with the words in these files. Write every string yourself from the English; don't copy
  text from other products.
- **Core strings shared with Vigilant Vole** (sign-in, setup, users, two-factor authentication, API tokens,
  settings, notification channels, the layout) come from vole-kit and are translated there. They follow Vigilant
  Vole's glossary for the language, not these files. Where a concept appears in both products – event, session,
  notification, alert, tag, retention, page views, visitors, referrers – the word here is the one Vigilant Vole
  already uses, and each file notes any deliberate difference.
- The writing rules are the family's (Vigilant Vole's `docs/translating.md` and the design system, §10): short,
  calm Apple-style copy; sentence case where the language uses it; the language's own quotation marks; an
  informal or neutral form of address, never stiff; no exclamation marks unless the English has one. Buttons,
  tabs and pills are narrow, so prefer the shorter of two good wordings.

## What each file contains

1. A **Style** section: form of address, quotation marks, casing, plurals and which technical words stay in
   English. It is adapted from Vigilant Vole's glossary for the same language so the family reads alike.
2. A **Terms** table (`| English | <language> | Note |`). The note gives what isn't obvious: grammatical gender,
   plurals, a shorter form for buttons and pills, or a clash with a word Vigilant Vole uses for something else.

## Words that stay in English everywhere

| Term | Why |
|---|---|
| DSN | The Sentry connection string. Users copy it into SDK configuration and search for it by this name. |
| dSYM | Apple's debug-symbol bundle format; Xcode and every Apple tool use the name untranslated. |
| SDK | Universal abbreviation; the SDKs themselves are named in English (sentry-cocoa, KestrelKit). |
| `sentry-cli` | A command name, always written as code. |
| symbolication | The established term in Apple and Sentry tooling; a translation would make it harder to search for help. Status labels may still use a natural phrase ("symbolicated" / "not symbolicated"), and a sentence can usually say what happened without the noun. Each file gives that phrase. |

Product, protocol and format names (Sentry, Xcode, fastlane, SymCache, Mach-O, DWARF, JSON, UTM, HTTP …), API scope
names (`difs:write`), search keys (`is:unresolved`, `release:`), signal types (`App.launched`) and SDK field names
(`fingerprint`, `in_app`) are never translated either.
