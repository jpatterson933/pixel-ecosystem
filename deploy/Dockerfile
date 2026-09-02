# All Rust services in one image. Pick the binary with the container command.
FROM rust:1-bookworm AS build
WORKDIR /work
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
RUN cargo build --release --workspace --exclude sim-wasm

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /work/target/release/worldgen \
                  /work/target/release/region-server \
                  /work/target/release/action-interface \
                  /work/target/release/driver-random \
                  /work/target/release/gateway \
                  /work/target/release/summary-service \
                  /usr/local/bin/
ENV RUST_LOG=info
