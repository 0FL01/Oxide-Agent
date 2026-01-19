# Chef stage - install cargo-chef for dependency caching
FROM rust:1.92-slim-trixie AS chef
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
RUN cargo chef cook --release --features crawl4ai --recipe-path recipe.json

# Build application - this layer is rebuilt when source changes
COPY . .
RUN cargo build --release --features crawl4ai

# Runtime stage - Debian Trixie (stable)
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/oxide-agent /app/oxide-agent
COPY skills/ /app/skills/


# Set environment variables
ENV RUST_LOG=oxide_agent=info,zai_rs=debug,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn,async_openai=warn
ENV DEBUG_MODE=false

CMD ["./oxide-agent"]
