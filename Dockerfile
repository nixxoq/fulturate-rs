FROM rust:1-bookworm AS base
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssl ca-certificates libfontconfig1 ffmpeg build-essential \
    clang lld mold \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"

FROM base AS dev-env
WORKDIR /app

FROM base AS planner
WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM base AS cacher
WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM base AS builder
WORKDIR /app
COPY . .
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
RUN cargo build --release --workspace

FROM debian:bookworm-slim AS fulturate-release
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssl ca-certificates libfontconfig1 ffmpeg \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/fulturate/config.json .
COPY --from=builder /app/fulturate/currencies.json .
COPY --from=builder /app/target/release/fulturate .
COPY --from=builder /app/fulturate/locales ./locales
CMD ["./fulturate"]