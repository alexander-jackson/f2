FROM lukemathwalker/cargo-chef:0.1.77-rust-1.95-alpine3.23 AS chef

WORKDIR /app

# Plan the dependencies
FROM chef AS planner
COPY Cargo.toml Cargo.lock .
COPY src src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Build the dependencies themselves
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build the binary
COPY Cargo.toml Cargo.lock .
COPY src src
RUN cargo build --release --bin f2

# Copy over to the minimal image
FROM gcr.io/distroless/static
WORKDIR /app
COPY --from=builder /app/target/release/f2 /app/f2
ENTRYPOINT ["/app/f2"]
