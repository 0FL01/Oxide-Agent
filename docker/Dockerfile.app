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
    mkdir -p /runtime/bin; \
    for binary in ${BINARIES}; do \
      test -x "/app/target/release/${binary}"; \
      cp "/app/target/release/${binary}" "/runtime/bin/${binary}"; \
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
RUN chown -R oxide:oxide /app /home/oxide

ENV TZ=Europe/Moscow
ENV RUST_LOG=oxide_agent_core=info,oxide_agent_transport_telegram=info,oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn
ENV DEBUG_MODE=false
ENV OXIDE_ENTRYPOINT_BINARY=${ENTRYPOINT_BINARY}

USER oxide

CMD ["/bin/sh", "-c", "exec /app/${OXIDE_ENTRYPOINT_BINARY}"]
