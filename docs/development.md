# Development

Keen Kestrel is one Rust crate (`keen-kestrel`, binary `kestrel`) on top of
[vole-kit](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/docs/vole-kit.md), the shared kit that lives in the
Vigilant Vole repository. The kit brings sign-in, users, API tokens, Security, the API reference, i18n, the
database plumbing and the design system; Kestrel adds its own pages, API, migrations and strings.

## Layout

```
Cargo.toml            # keen-kestrel; vole-kit by git tag
build.rs              # copies the kit's templates into templates/kit/ (git-ignored)
rust-toolchain.toml   # Rust 1.99
src/
  main.rs             # `kestrel`: flags/env, logging, server
  lib.rs              # start() (config, database, state) and router()
  product.rs          # KitConfig: names, cookies, token prefix, nav, locales, static files
  config.rs           # flags and KK_* environment variables
  server.rs           # listener and graceful shutdown
  project.rs          # project and key rules (names, slugs, origins, rate limits)
  db/                 # migration set, projects and project_keys
  dsn.rs              # public address, DSNs, SDK snippets
  pages.rs            # project pages
  api.rs              # REST API v1 and its route table (API reference)
  mcp.rs              # MCP tools
templates/            # Kestrel's pages (extend kit/layout.html)
static/               # favicon, home-screen icons, web manifest
migrations/           # Kestrel's migrations (after the kit's core set)
locales/              # Kestrel's strings, 26 languages; glossaries in locales/glossary/
tests/                # in-process app tests, translation coverage
```

## Build, run, test

```bash
cargo run                      # http://localhost:3002, data in ./data
cargo run -- --help            # every flag and its KK_ variable
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The first start opens `/setup`, which creates the admin account.

| Flag | Variable | Default | What |
|---|---|---|---|
| `--data-dir` | `KK_DATA_DIR` | `./data` | `kestrel.sqlite`, `session.key`, `backups/` |
| `--host` | `KK_HOST` | `0.0.0.0` | listen address |
| `--port` | `KK_PORT` | `3002` | listen port |
| `--public-url` | `KK_PUBLIC_URL` | (none) | the address in DSNs and API examples, e.g. `https://kestrel.example.com`; without it each request's own address is used |
| | `KK_TRUST_PROXY` | off | honour `X-Forwarded-*` from a reverse proxy |
| | `KK_SETUP_TOKEN` | (none) | `/setup` opens only with `?token=…` |
| | `RUST_LOG` | `info` | log filter |

`KK_PUBLIC_URL` is stored as the `base_url` setting at start-up (and removed when unset), so the kit's same-origin
checks, the API reference and the MCP origin check use the same address as the DSNs.

## Working on vole-kit at the same time

Cargo.toml uses the kit's release tag. To try a kit change, point Cargo at a checkout of the Vigilant Vole
repository next to this one with a local, uncommitted `.cargo/config.toml` (it is in `.gitignore`):

```toml
[patch."https://github.com/Ferdinand99/Vigilant-Vole"]
vole-kit = { path = "../Vigilant-Vole/kit" }
vole-kit-build = { path = "../Vigilant-Vole/kit/kit-build" }
```

Remove it (or move the tag in Cargo.toml once the kit is released) before you commit `Cargo.lock`. Edit the kit's
templates in the Vigilant Vole repository; `templates/kit/` is a build-time copy and is overwritten.

## Docker

```bash
docker build -t keen-kestrel:latest .
docker run -d --name keen-kestrel -p 3002:3002 -v ./data:/data keen-kestrel:latest
docker compose up -d           # the same with docker-compose.yml
```

The image is a static musl binary on Alpine. The container starts as root only to hand `/data` to the `kestrel`
user (uid 1000, or `PUID`/`PGID`), then runs as that user. `HEALTHCHECK` calls `/healthz`.

## CI and releases

- `.github/workflows/ci.yml`: `cargo fmt --check`, clippy with `-D warnings` and `cargo test` on Rust 1.99; on pull
  requests also a Docker build for amd64 and arm64 (cache only).
- `.github/workflows/docker-publish.yml`: multi-arch images on GHCR, `:edge` from main and `:X.Y.Z`, `:X.Y`, `:X`,
  `:latest` for releases.
- `.github/workflows/release-please.yml`: one release PR for `keen-kestrel` (tags `vX.Y.Z`, `CHANGELOG.md`),
  starting from 0.0.0, so the first `feat` releases 0.1.0. Merging it publishes the image.

## Translations

`locales/en.toml` holds Kestrel's own English strings; the other 25 files translate every key, with the words from
`locales/glossary/<code>.md`. `tests/i18n.rs` fails when a language misses a key, has an extra one, changes a
placeholder or lacks a plural form its language needs. Core strings (sign-in, users, tokens, Security, the layout)
come from vole-kit; a key may not be in both catalogs.
