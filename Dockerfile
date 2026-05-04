FROM lukemathwalker/cargo-chef:0.1.68-rust-1.82.0-alpine3.20 AS chef

WORKDIR /app

# Plan the dependencies
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Build the dependencies themselves
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Build the binary
COPY . .
RUN cargo build --release --bin f2

# Copy over to the minimal image
FROM gcr.io/distroless/static
WORKDIR /app
COPY --from=builder /app/target/release/f2 /app/f2
ENTRYPOINT ["/app/f2"]
