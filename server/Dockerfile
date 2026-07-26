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

# positions.json is `include_str!`d by the game module, so it has to be here
# before the real build, not only in the runtime image.
COPY server/positions.json ./positions.json
COPY server/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
# Stockfish scores the puzzle positions. Bookworm ships 15.1, ample at the
# 400 ms per ply the game asks for. Without it the game still runs: eval falls
# back to a material count, labelled as such on screen.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates stockfish \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/arcade-chess-server /usr/local/bin/arcade-chess-server
# The puzzle deck, which is also compiled into the binary as the fallback.
# `position-miner/data/` is gitignored and CapRover builds from git, so a mined
# deck reaches production by overwriting this committed file:
#   arcpos dump verified.arcpos --json -n 5000 > server/positions.json
# (note `-n` defaults to 20). Mounting a volume over /srv/positions.json works
# too. Either way, a deck that fails to load leaves the embedded one in place
# and says `positions_embedded` in `degraded`.
COPY server/positions.json /srv/positions.json
EXPOSE 8080
ENV PORT=8080 \
    STOCKFISH_PATH=/usr/games/stockfish \
    POSITIONS_PATH=/srv/positions.json
CMD ["arcade-chess-server"]
