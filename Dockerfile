FROM rust:1.71.1-alpine3.18 AS builder

RUN apk add --no-cache musl build-base clang llvm14
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY . .

ENV CC_x86_64_unknown_linux_musl=clang
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-Clink-self-contained=yes -Clinker=rust-lld"

RUN cargo install --target x86_64-unknown-linux-musl --path .

FROM gcr.io/distroless/static
COPY --from=builder /usr/local/cargo/bin/f2 .
ENTRYPOINT ["./f2"]
