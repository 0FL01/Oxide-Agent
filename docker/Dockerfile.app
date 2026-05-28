# syntax=docker/dockerfile:1.7

FROM rust:1.94-slim-trixie AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef

FROM chef AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /app

ARG CARGO_FEATURES="oxide-agent-telegram-bot/profile-embedded-opencode-local"
ARG PACKAGES="oxide-agent-telegram-bot"
ARG BINARIES="oxide-agent-telegram-bot"
ARG BUILD_WEB_UI="false"

COPY --from=planner /app/recipe.json recipe.json
RUN if [ -n "${CARGO_FEATURES}" ]; then \
      cargo chef cook --release --workspace --no-default-features --features "${CARGO_FEATURES}" --recipe-path recipe.json; \
    else \
      cargo chef cook --release --workspace --no-default-features --recipe-path recipe.json; \
    fi

COPY . .
RUN set -eux; \
    package_args=""; \
    for package in ${PACKAGES}; do package_args="${package_args} -p ${package}"; done; \
    if [ -n "${CARGO_FEATURES}" ]; then \
      cargo build --release --no-default-features ${package_args} --features "${CARGO_FEATURES}"; \
    else \
      cargo build --release --no-default-features ${package_args}; \
    fi; \
    mkdir -p /runtime/bin /runtime/web; \
    for binary in ${BINARIES}; do \
      test -x "/app/target/release/${binary}"; \
      cp "/app/target/release/${binary}" "/runtime/bin/${binary}"; \
    done; \
    if [ "${BUILD_WEB_UI}" = "true" ]; then \
      rustup target add wasm32-unknown-unknown; \
      cargo install trunk --version 0.21.14 --locked; \
      cd /app/crates/oxide-agent-web-ui; \
      env -u NO_COLOR trunk build --release; \
      cp -R /app/crates/oxide-agent-web-ui/dist/. /runtime/web/; \
    fi

FROM debian:trixie-slim AS external-runtime-binaries

ARG MCP_BINARIES=""
ARG SSH_MCP_VERSION=v2.0.4
ARG SSH_MCP_LINUX_X86_64_SHA256=ac77c6b0908fbc2e41b9d300432f32e4ccfe9174df5b6a0ed92274fc76f83ca2
ARG JIRA_MCP_VERSION=0.1.2
ARG MATTERMOST_MCP_VERSION=0.1.2

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    tar \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    mkdir -p /runtime/bin; \
    for binary in ${MCP_BINARIES}; do \
      case "${binary}" in \
        ssh-mcp) \
          curl -fsSL "https://github.com/0FL01/ssh-mcp-rs/releases/download/${SSH_MCP_VERSION}/ssh-mcp-linux-x86_64" -o /runtime/bin/ssh-mcp; \
          echo "${SSH_MCP_LINUX_X86_64_SHA256}  /runtime/bin/ssh-mcp" | sha256sum -c -; \
          chmod +x /runtime/bin/ssh-mcp; \
          ;; \
        jira-mcp) \
          curl -fsSL "https://github.com/0FL01/jira-mcp/releases/download/${JIRA_MCP_VERSION}/jira-mcp_linux_amd64.tar.gz" -o /tmp/jira-mcp.tar.gz; \
          tar -xzf /tmp/jira-mcp.tar.gz -C /runtime/bin jira-mcp; \
          rm /tmp/jira-mcp.tar.gz; \
          chmod +x /runtime/bin/jira-mcp; \
          ;; \
        mattermost-mcp) \
          curl -fsSL "https://github.com/0FL01/mcp-server-mattermost/releases/download/${MATTERMOST_MCP_VERSION}/mcp-server-mattermost" -o /runtime/bin/mattermost-mcp; \
          chmod +x /runtime/bin/mattermost-mcp; \
          ;; \
        *) \
          echo "unknown MCP binary '${binary}'" >&2; \
          exit 1; \
          ;; \
      esac; \
    done

FROM debian:trixie-slim AS runtime

ARG RUNTIME_APT_PACKAGES=""
ARG ENTRYPOINT_BINARY="oxide-agent-telegram-bot"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    tzdata \
    ${RUNTIME_APT_PACKAGES} \
    && rm -rf /var/lib/apt/lists/*

RUN ln -snf /usr/share/zoneinfo/Europe/Moscow /etc/localtime \
    && echo "Europe/Moscow" > /etc/timezone

RUN groupadd --system --gid 10001 oxide \
    && useradd --system --uid 10001 --gid 10001 --create-home --home-dir /home/oxide oxide

WORKDIR /app
COPY --from=builder /runtime/bin/ /app/
COPY --from=builder /runtime/web/ /app/web/
COPY --from=external-runtime-binaries /runtime/bin/ /app/
RUN chown -R oxide:oxide /app /home/oxide

ENV TZ=Europe/Moscow
ENV RUST_LOG=oxide_agent_core=info,oxide_agent_transport_telegram=info,oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn
ENV DEBUG_MODE=false
ENV OXIDE_ENTRYPOINT_BINARY=${ENTRYPOINT_BINARY}

USER oxide

CMD ["/bin/sh", "-c", "exec /app/${OXIDE_ENTRYPOINT_BINARY}"]
