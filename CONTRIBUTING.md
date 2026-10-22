# Contributing

Thanks for your interest in Keen Kestrel.

## Status

Kestrel is **in early development**: the skeleton (sign-in, users, tokens, projects and keys) runs, ingest
doesn't exist yet. The design is in [`docs/design.md`](docs/design.md); it is the reference for everything that
gets built, and it changes when a decision changes. Questions and comments on the design are welcome as issues.
How to build, run and test: [`docs/development.md`](docs/development.md).

## Translations

The interface will ship in 26 languages. Each language has a glossary in
[`locales/glossary/`](locales/glossary/) that fixes the tone and the words for Kestrel's concepts (issue, event,
release, crash, signal …). Kestrel's strings are written from English with those words; core strings shared with
[Vigilant Vole](https://github.com/Ferdinand99/Vigilant-Vole) come from vole-kit and follow Vigilant Vole's
glossaries. The glossaries are drafts, so a native speaker's corrections are one of the most useful contributions
right now. See the [glossary README](locales/glossary/README.md).

## Builds

Heavy builds and tests (the static musl binary, the container image, load tests) run in Docker, not on the host.
