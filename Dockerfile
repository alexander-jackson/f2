FROM rust:1.71.1-alpine3.18 AS builder

WORKDIR /app
COPY . .

RUN apk add --no-cache musl build-base
RUN rustup target add x86_64-unknown-linux-musl

RUN cargo install --target x86_64-unknown-linux-musl --path .

FROM gcr.io/distroless/static
COPY --from=builder /usr/local/cargo/bin/f2 .
ENTRYPOINT ["./f2"]
