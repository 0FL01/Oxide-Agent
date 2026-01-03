# Chef stage - install cargo-chef for dependency caching
FROM rust:1.92-slim-trixie AS chef
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
RUN cargo chef cook --release --recipe-path recipe.json

# Build application - this layer is rebuilt when source changes
COPY . .
RUN cargo build --release

# Runtime stage - Debian Trixie (stable)
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/another-chat-rs /app/another-chat-rs
COPY AGENT.md /app/AGENT.md


# Set environment variables
ENV RUST_LOG=info

CMD ["./another-chat-rs"]
