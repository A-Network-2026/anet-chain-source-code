FROM rust:1.86-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml ./
COPY src ./src
# `src/rpc.rs` embeds Revelation Block 0 assets via include_bytes!/include_str!
# from ../config/genesis/revelation_block_0/, so the config dir must be present
# at build time.
COPY config ./config

RUN cargo build --release

FROM debian:bookworm-slim AS runtime

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app anet

COPY --from=builder /app/target/release/anet-private-mainnet /usr/local/bin/anet-private-mainnet
COPY config ./config

RUN mkdir -p /app/data /var/data/anet/config /var/data/anet/data \
    && chown -R anet:anet /app

ENV PORT=8080
EXPOSE 8080

CMD ["anet-private-mainnet", "--bootstrap", "--start-node"]