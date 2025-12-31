# Builder stage
FROM rust:1.80-slim-bookworm as builder

WORKDIR /app
# Pre-install dependencies to cache them
# COPY Cargo.toml Cargo.lock ./
# RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/another-chat-rs /app/another-chat-rs

# Set environment variables
ENV RUST_LOG=info

CMD ["./another-chat-rs"]
