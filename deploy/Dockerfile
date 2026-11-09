# syntax=docker/dockerfile:1.7
# MuseHub product images (fixed toolchain versions). API container does NOT mount docker.sock.

ARG RUST_VERSION=1.89-bookworm
ARG NODE_VERSION=20-bookworm-slim
ARG DEBIAN_VERSION=bookworm-slim
ARG CADDY_VERSION=2.8.4-alpine

FROM rust:${RUST_VERSION} AS rust-builder
WORKDIR /build
RUN apt-get update && apt-get install -y pkg-config libssl-dev git && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY VERSION ./
RUN cargo build --release -p musehub-server -p musehub-migrate \
  && cargo build --release -p musehub-agent-adapter \
    --bin musehub-agent-reference \
    --bin musehub-agent-codex-sim \
    --bin musehub-pipeline-reference \
    --bin musehub-pipeline-oci \
    --bin musehub-compose-mock

FROM node:${NODE_VERSION} AS web-builder
WORKDIR /web
COPY web/package.json web/yarn.lock ./
RUN corepack enable && yarn install --frozen-lockfile
COPY web/ ./
COPY VERSION /VERSION
RUN yarn build

FROM debian:${DEBIAN_VERSION} AS server-runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git tini wget \
  && rm -rf /var/lib/apt/lists/* \
  && useradd -r -u 10001 -d /var/lib/musehub musehub \
  && mkdir -p /opt/musehub/bin /var/lib/musehub/git /var/lib/musehub/artifacts /migrations \
  && chown -R musehub:musehub /var/lib/musehub
COPY --from=rust-builder /build/target/release/musehub-server /usr/local/bin/
COPY --from=rust-builder /build/target/release/musehub-migrate /usr/local/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-reference /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-codex-sim /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-pipeline-reference /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-pipeline-oci /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-compose-mock /opt/musehub/bin/
COPY migrations /migrations
COPY deploy/docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh
USER musehub
ENV MUSEHUB_BIND=0.0.0.0:8080 \
    MUSEHUB_GIT_ROOT=/var/lib/musehub/git \
    MUSEHUB_ARTIFACT_ROOT=/var/lib/musehub/artifacts \
    MUSEHUB_REFERENCE_ADAPTER_BIN=/opt/musehub/bin/musehub-agent-reference \
    MUSEHUB_AGENT_CODEX_SIM_BIN=/opt/musehub/bin/musehub-agent-codex-sim \
    MUSEHUB_PIPELINE_ADAPTER_BIN=/opt/musehub/bin/musehub-pipeline-reference \
    MUSEHUB_COMPOSE_ADAPTER_BIN=/opt/musehub/bin/musehub-compose-mock \
    MUSEHUB_COMPOSE_RUNNER_BIN=/opt/musehub/bin/musehub-compose-mock \
    MUSEHUB_COMPOSE_ADAPTER_MODE=mock
VOLUME ["/var/lib/musehub/git", "/var/lib/musehub/artifacts"]
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --start-period=40s --retries=20 \
  CMD wget -q -O- http://127.0.0.1:8080/readyz | grep -q '"ready":true' || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/docker-entrypoint.sh"]
CMD ["server"]

FROM caddy:${CADDY_VERSION} AS gateway-runtime
COPY --from=web-builder /web/dist /srv/web
COPY deploy/Caddyfile /etc/caddy/Caddyfile
EXPOSE 443 80
