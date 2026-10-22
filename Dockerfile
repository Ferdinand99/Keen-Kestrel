# syntax=docker/dockerfile:1
#
# Keen Kestrel: one static musl binary on Alpine, rustls only (no OpenSSL).
#
#   docker build -t keen-kestrel:latest .
#
# Needs BuildKit (the default since Docker 23). The cargo registry, the git
# checkout of vole-kit and the target directory live in cache mounts, so
# rebuilds after a source change only recompile Kestrel itself.

ARG RUST_VERSION=1.99
ARG ALPINE_VERSION=3.22

# --- Builder -----------------------------------------------------------------
FROM rust:${RUST_VERSION}-alpine AS builder

# build-base: C toolchain for the bundled SQLite and aws-lc-rs (rustls crypto).
RUN apk add --no-cache build-base

WORKDIR /app

# Smaller binary without touching Cargo.toml: no symbols, and link-time
# optimisation across all crates in one unit (keeps the image under the
# 40 MB the design sets, docs/design.md §8).
ENV CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_PROFILE_RELEASE_LTO=true \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_TERM_COLOR=never

# Everything the build reads: templates, static files, migrations and locales
# are compiled into the binary. vole-kit comes from its git tag (Cargo.lock).
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations
COPY locales ./locales

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release --locked \
    && cp target/release/kestrel /usr/local/bin/kestrel

# --- Runtime -----------------------------------------------------------------
FROM alpine:${ALPINE_VERSION} AS runtime

ARG VERSION=0.0.0
ARG REVISION=unknown
ARG CREATED=unknown

LABEL org.opencontainers.image.title="Keen Kestrel" \
      org.opencontainers.image.description="Self-hosted error tracking and privacy-friendly analytics: one Rust binary, SQLite" \
      org.opencontainers.image.url="https://github.com/Ferdinand99/Keen-Kestrel" \
      org.opencontainers.image.source="https://github.com/Ferdinand99/Keen-Kestrel" \
      org.opencontainers.image.documentation="https://github.com/Ferdinand99/Keen-Kestrel#readme" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.created="${CREATED}"

# ca-certificates: TLS trust store for outgoing HTTPS (notifications).
# tzdata: time zones for display.
# su-exec: the entrypoint drops from root to `kestrel` after fixing /data ownership.
# Unprivileged user `kestrel` (uid/gid 1000) owns /data and runs the app.
RUN apk add --no-cache ca-certificates tzdata su-exec \
    && addgroup -S -g 1000 kestrel \
    && adduser -S -D -H -u 1000 -G kestrel -h /data -s /sbin/nologin kestrel \
    && mkdir -p /data \
    && chown kestrel:kestrel /data

COPY --from=builder /usr/local/bin/kestrel /usr/local/bin/kestrel
COPY --chmod=755 docker/entrypoint.sh /usr/local/bin/docker-entrypoint.sh

ENV KK_DATA_DIR=/data \
    KK_PORT=3002

# No `USER` here on purpose: the entrypoint starts as root only to chown /data,
# then runs the app as `kestrel` via su-exec. Setting `user:` / `--user` skips
# that step entirely.
WORKDIR /data
VOLUME ["/data"]
EXPOSE 3002

# The server answers /healthz (busybox wget, no curl needed).
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${KK_PORT:-3002}/healthz" || exit 1

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
