# syntax=docker/dockerfile:1
FROM oven/bun:1.4.4 AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

FROM rust:1-bookworm AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY deploy/ ./deploy/
COPY --from=web /src/web/dist ./web/dist
ARG CARGO_BUILD_JOBS=2
ARG APSP_RELEASE_PUBLIC_KEYS
ENV APSP_RELEASE_PUBLIC_KEYS=$APSP_RELEASE_PUBLIC_KEYS
RUN CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" cargo build --locked --release -p apsp-panel

FROM postgres:16-bookworm AS postgres-client
RUN mkdir -p /pg-client/bin /pg-client/lib \
    && cp /usr/lib/postgresql/16/bin/pg_dump /pg-client/bin/ \
    && cp -L /usr/lib/*-linux-gnu/libpq.so.5 /pg-client/lib/libpq.so.5

FROM goacme/lego:v5.6.0@sha256:6f1583fc026a455ea46a7a5858c576105bb69c2e2de69db391cab7f14750350c AS lego

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libssl3 libpq5 liblz4-1 libzstd1 zlib1g age \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 apsp \
    && useradd --uid 10001 --gid 10001 --no-log-init --no-create-home --shell /usr/sbin/nologin apsp \
    && mkdir -p /data/artifacts /usr/local/libexec/apsp \
    && chown -R 10001:10001 /data
COPY --from=builder /src/target/release/apsp-panel /usr/local/bin/apsp-panel
COPY --from=postgres-client /pg-client/bin/pg_dump /usr/local/bin/pg_dump
COPY --from=postgres-client /pg-client/lib/libpq.so.5 /usr/local/lib/libpq.so.5
COPY --from=lego /lego /usr/local/libexec/apsp/lego
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
        amd64) expected_sha256="9ba1a7f9ed951e6a506cf7581fcb0373ee3ae9b6fd58d4572a08400c1e905ba8" ;; \
        arm64) expected_sha256="815ffd1f3ebd65bfbb9cd790bfd18f9cca91f48d8aec3a08732b3fc3b39056d7" ;; \
        *) echo "unsupported runtime architecture for builtin lego: $arch" >&2; exit 1 ;; \
    esac; \
    echo "${expected_sha256}  /usr/local/libexec/apsp/lego" | sha256sum -c -; \
    chown 0:0 /usr/local/libexec /usr/local/libexec/apsp /usr/local/libexec/apsp/lego; \
    chmod 0755 /usr/local/libexec /usr/local/libexec/apsp; \
    chmod 0555 /usr/local/libexec/apsp/lego; \
    test "$(/usr/local/libexec/apsp/lego --version)" = "lego version 5.6.0 linux/${arch}"; \
    ldconfig
USER 10001:10001
WORKDIR /data
ENV APSP_LISTEN=0.0.0.0:8080 \
    APSP_DATA_DIR=/data \
    RUST_LOG=info
EXPOSE 8080
STOPSIGNAL SIGINT
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=12 \
    CMD curl --fail --silent http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/apsp-panel"]
