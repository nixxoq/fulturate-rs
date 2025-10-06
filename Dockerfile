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

FROM debian:bookworm-slim AS fulturate-release
RUN apt-get update && apt-get install -y openssl ca-certificates && ldconfig /usr/local/lib64/
WORKDIR /app
#COPY --from=builder /app/fulturate/.env /usr/local/bin/fulturate
COPY --from=builder /app/fulturate/config.json .
COPY --from=builder /app/fulturate/currencies.json .
COPY --from=builder /app/target/release/fulturate .
CMD ["./fulturate"]

FROM debian:bookworm AS api-release
RUN apt-get update && apt-get install -y openssl ca-certificates && ldconfig /usr/local/lib64/
WORKDIR /app
COPY --from=builder /app/target/release/fulturate_api .
EXPOSE 3000
CMD ["./fulturate_api"]