FROM lukemathwalker/cargo-chef:latest-rust-latest AS chef
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .

RUN cargo build --release --bin f2

FROM debian:buster-slim AS runtime
WORKDIR app
COPY --from=builder /app/target/release/f2 /usr/local/bin
ENTRYPOINT ["/usr/local/bin/f2"]
