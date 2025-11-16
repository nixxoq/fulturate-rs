FROM rust:latest AS planner
WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM rust:latest AS cacher
WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM rust:latest AS builder
WORKDIR /app
COPY . .
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
RUN cargo build --release --workspace

FROM debian:sid-slim AS fulturate-release
RUN apt-get update && apt-get install -y --no-install-recommends openssl ca-certificates libfontconfig1 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/fulturate/config.json .
COPY --from=builder /app/fulturate/currencies.json .
COPY --from=builder /app/target/release/fulturate .
CMD ["./fulturate"]

FROM debian:sid AS api-release
RUN apt-get update && apt-get install -y --no-install-recommends openssl ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/fulturate_api .
EXPOSE 3000
CMD ["./fulturate_api"]