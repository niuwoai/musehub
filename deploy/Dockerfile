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
RUN cargo build --release -p musehub-server -p musehub-migrate -p musehub-runnerd -p musehub-git-ssh \
  && cargo build --release -p musehub-agent-adapter \
    --bin musehub-agent-reference \
    --bin musehub-agent-codex-sim \
    --bin musehub-pipeline-reference \
    --bin musehub-pipeline-oci \
    --bin musehub-compose-mock \
    --bin musehub-compose-runner

FROM node:${NODE_VERSION} AS web-builder
WORKDIR /web
COPY web/package.json web/yarn.lock ./
RUN sed -i 's|https://registry.npmmirror.com/|https://registry.npmjs.org/|g' yarn.lock \
  && corepack enable \
  && yarn config set npmRegistryServer https://registry.npmjs.org \
  && yarn install --frozen-lockfile
COPY web/ ./
COPY VERSION /VERSION
RUN yarn build

FROM debian:${DEBIAN_VERSION} AS server-runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git tini wget \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd -r -g 998 musehub-git \
  && useradd -r -u 10001 -d /var/lib/musehub musehub \
  && usermod -aG musehub-git musehub \
  && mkdir -p /opt/musehub/bin /var/lib/musehub/git /var/lib/musehub/artifacts /migrations \
  && chown -R musehub:musehub-git /var/lib/musehub/git \
  && chown musehub:musehub /var/lib/musehub/artifacts /var/lib/musehub \
  && chmod 2775 /var/lib/musehub/git
COPY --from=rust-builder /build/target/release/musehub-server /usr/local/bin/
COPY --from=rust-builder /build/target/release/musehub-migrate /usr/local/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-reference /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-codex-sim /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-pipeline-reference /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-pipeline-oci /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-compose-mock /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-compose-runner /opt/musehub/bin/
COPY migrations /migrations
COPY deploy/docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh
USER musehub
ENV MUSEHUB_PRODUCT_PROFILE=demo \
    MUSEHUB_BIND=0.0.0.0:8080 \
    MUSEHUB_GIT_ROOT=/var/lib/musehub/git \
    MUSEHUB_GIT_STORAGE_GROUP=musehub-git \
    MUSEHUB_ARTIFACT_ROOT=/var/lib/musehub/artifacts \
    MUSEHUB_REFERENCE_ADAPTER_BIN=/opt/musehub/bin/musehub-agent-reference \
    MUSEHUB_AGENT_CODEX_SIM_BIN=/opt/musehub/bin/musehub-agent-codex-sim \
    MUSEHUB_PIPELINE_ADAPTER_BIN=/opt/musehub/bin/musehub-pipeline-reference \
    MUSEHUB_COMPOSE_ADAPTER_BIN=/opt/musehub/bin/musehub-compose-mock \
    MUSEHUB_COMPOSE_RUNNER_BIN=/opt/musehub/bin/musehub-compose-runner \
    MUSEHUB_COMPOSE_ADAPTER_MODE=mock \
    MUSEHUB_AGENT_EXECUTOR=inprocess \
    MUSEHUB_PIPELINE_EXECUTOR=inprocess
VOLUME ["/var/lib/musehub/git", "/var/lib/musehub/artifacts"]
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --start-period=40s --retries=20 \
  CMD wget -q -O- http://127.0.0.1:8080/readyz | grep -q '"product_api":"ok"' || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/docker-entrypoint.sh"]
CMD ["server"]

FROM debian:${DEBIAN_VERSION} AS runner-runtime
ARG DOCKER_CLI_VERSION=27.3.1
ARG COMPOSE_VERSION=2.29.7
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates git curl tini \
  && curl -fsSL "https://download.docker.com/linux/static/stable/$(uname -m)/docker-${DOCKER_CLI_VERSION}.tgz" \
    | tar xz -C /usr/local/bin --strip-components=1 docker/docker \
  && mkdir -p /usr/local/lib/docker/cli-plugins \
  && curl -fsSL "https://github.com/docker/compose/releases/download/v${COMPOSE_VERSION}/docker-compose-linux-$(uname -m)" \
    -o /usr/local/lib/docker/cli-plugins/docker-compose \
  && chmod +x /usr/local/lib/docker/cli-plugins/docker-compose \
  && rm -rf /var/lib/apt/lists/* \
  && useradd -r -u 10001 -d /var/lib/musehub musehub \
  && mkdir -p /opt/musehub/bin /var/lib/musehub/git /var/lib/musehub/artifacts \
  && chown -R musehub:musehub /var/lib/musehub \
  && git config --system --add safe.directory '*'
COPY --from=rust-builder /build/target/release/musehub-runnerd /usr/local/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-reference /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-agent-codex-sim /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-pipeline-oci /opt/musehub/bin/
COPY --from=rust-builder /build/target/release/musehub-compose-runner /opt/musehub/bin/
USER musehub
ENV MUSEHUB_GIT_ROOT=/var/lib/musehub/git \
    MUSEHUB_ARTIFACT_ROOT=/var/lib/musehub/artifacts
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/musehub-runnerd"]

FROM debian:${DEBIAN_VERSION} AS git-ssh-runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends openssh-server git ca-certificates tini \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd -r -g 998 musehub-git \
  && useradd -r -u 997 -M -s /bin/false git \
  && passwd -d git \
  && usermod -aG musehub-git git \
  && useradd -r -u 10001 -d /var/lib/musehub musehub \
  && usermod -aG musehub-git musehub \
  && mkdir -p /var/lib/musehub/git /var/lib/musehub/git-ssh /run/sshd \
  && chown root:git /var/lib/musehub/git-ssh \
  && chown -R musehub:musehub-git /var/lib/musehub/git \
  && chmod 2775 /var/lib/musehub/git \
  && chmod 0750 /var/lib/musehub/git-ssh
COPY --from=rust-builder /build/target/release/musehub-git-ssh /usr/local/bin/
ENV MUSEHUB_GIT_ROOT=/var/lib/musehub/git \
    MUSEHUB_GIT_STORAGE_GROUP=musehub-git \
    MUSEHUB_GIT_SSH_RUN_DIR=/var/lib/musehub/git-ssh \
    MUSEHUB_GIT_SSH_LISTEN=0.0.0.0 \
    MUSEHUB_GIT_SSH_PORT=2222 \
    MUSEHUB_GIT_SSH_AKC_USER=git \
    MUSEHUB_GIT_SSH_RUNTIME_GROUP=git
VOLUME ["/var/lib/musehub/git", "/var/lib/musehub/git-ssh"]
EXPOSE 2222
# sshd supervisor must start as root; AKC/gateway drop to `git` / unprivileged paths.
USER root
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/musehub-git-ssh", "serve"]

FROM caddy:${CADDY_VERSION} AS gateway-runtime
COPY --from=web-builder /web/dist /srv/web
COPY deploy/Caddyfile /etc/caddy/Caddyfile
EXPOSE 443 80
