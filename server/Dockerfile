# Build context is the git repo root (Caprover inherits it), so every COPY path
# is prefixed with server/. Build locally with: docker build -f server/Dockerfile .
FROM rust:1.93-slim-bookworm AS builder
WORKDIR /build

# Warm the dependency cache before copying real sources.
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src \
       target/release/arcade-chess-server \
       target/release/deps/arcade_chess_server* \
       target/release/.fingerprint/arcade-chess-server*

COPY server/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/arcade-chess-server /usr/local/bin/arcade-chess-server
EXPOSE 8080
ENV PORT=8080
CMD ["arcade-chess-server"]
