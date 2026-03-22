# Chef stage - install cargo-chef for dependency caching
FROM rust:1.94-slim-trixie AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef

# Planner stage - create dependency recipe
FROM chef AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Builder stage - build dependencies first (cached layer)
FROM chef AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this layer is cached unless dependencies change
RUN cargo chef cook --release --workspace --features oxide-agent-core/crawl4ai --recipe-path recipe.json

# Build application - this layer is rebuilt when source changes
COPY . .
RUN cargo build --release -p oxide-agent-telegram-bot -p oxide-agent-sandboxd -F oxide-agent-core/crawl4ai

FROM debian:trixie-slim AS ssh-mcp-binary

ARG SSH_MCP_VERSION=v2.0.4
ARG SSH_MCP_LINUX_X86_64_SHA256=ac77c6b0908fbc2e41b9d300432f32e4ccfe9174df5b6a0ed92274fc76f83ca2

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL "https://github.com/0FL01/ssh-mcp-rs/releases/download/${SSH_MCP_VERSION}/ssh-mcp-linux-x86_64" -o /usr/local/bin/ssh-mcp \
    && echo "${SSH_MCP_LINUX_X86_64_SHA256}  /usr/local/bin/ssh-mcp" | sha256sum -c - \
    && chmod +x /usr/local/bin/ssh-mcp

FROM debian:trixie-slim AS jira-mcp-binary

ARG JIRA_MCP_VERSION=0.1.0
ARG JIRA_MCP_LINUX_AMD64_SHA256=a4f7e7c8e3f9d2b1c0a9e8f7d6c5b4a3e2d1c0b9a8f7e6d5c4b3a2e1d0c9b8

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL "https://github.com/0FL01/jira-mcp/releases/download/${JIRA_MCP_VERSION}/jira-mcp_linux_amd64.tar.gz" -o /tmp/jira-mcp.tar.gz \
    && echo "${JIRA_MCP_LINUX_AMD64_SHA256}  /tmp/jira-mcp.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/jira-mcp.tar.gz -C /usr/local/bin jira-mcp \
    && rm /tmp/jira-mcp.tar.gz \
    && chmod +x /usr/local/bin/jira-mcp

# Runtime stage - Debian Trixie (stable)
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    openssh-client \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

RUN ln -snf /usr/share/zoneinfo/Europe/Moscow /etc/localtime && \
    echo "Europe/Moscow" > /etc/timezone

RUN groupadd --system --gid 10001 oxide \
    && useradd --system --uid 10001 --gid 10001 --create-home --home-dir /home/oxide oxide

WORKDIR /app
COPY --from=builder /app/target/release/oxide-agent-telegram-bot /app/oxide-agent-telegram-bot
COPY --from=builder /app/target/release/oxide-agent-sandboxd /app/oxide-agent-sandboxd
COPY --from=ssh-mcp-binary /usr/local/bin/ssh-mcp /usr/local/bin/ssh-mcp
COPY --from=jira-mcp-binary /build/jira-mcp /usr/local/bin/jira-mcp
RUN chmod +x /usr/local/bin/jira-mcp
COPY skills/ /app/skills/

RUN chown -R oxide:oxide /app /home/oxide


# Set environment variables
ENV TZ=Europe/Moscow
ENV RUST_LOG=oxide_agent_core=info,oxide_agent_transport_telegram=info,oxide_agent_runtime=info,zai_rs=debug,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn,async_openai=warn
ENV DEBUG_MODE=false

USER oxide

CMD ["./oxide-agent-telegram-bot"]
