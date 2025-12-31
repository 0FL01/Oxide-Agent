# Builder stage - Rust 1.92.0 (2025-12-08)
FROM rust:1.92-slim-trixie AS builder

WORKDIR /app

COPY . .
RUN cargo build --release

# Runtime stage - Debian Trixie (stable)
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/another-chat-rs /app/another-chat-rs

# Set environment variables
ENV RUST_LOG=info

CMD ["./another-chat-rs"]
