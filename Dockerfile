FROM rust:1.75.0-bookworm as chef
RUN cargo install cargo-chef --locked

WORKDIR /app
RUN apt update && apt install libssl-dev pkg-config -y

FROM chef as planner
COPY Cargo.* rust-toolchain.toml ./
COPY src src
# Compute a lock-like file for our project
RUN cargo chef prepare --recipe-path recipe.json

FROM chef as builder
COPY --from=planner /app/recipe.json recipe.json
# Build our project dependencies, not our application!
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.* rust-toolchain.toml ./
COPY src src

FROM builder AS builder
RUN cargo build --release --bin splash

FROM debian:bookworm-slim AS base
WORKDIR /app
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends libssl-dev ca-certificates \
    # Clean up
    && apt-get autoremove -y \
    && apt-get clean -y \
    && rm -rf /var/lib/apt/lists/*

FROM base AS splash
COPY --from=builder /app/target/release/splash splash
ENTRYPOINT ["./splash"]
