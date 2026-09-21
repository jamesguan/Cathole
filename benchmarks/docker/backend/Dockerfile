FROM rust:1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 benchmark
COPY --from=builder /build/target/release/cathole-benchmark-backend /usr/local/bin/backend
USER 10001
EXPOSE 3000 3001 3002/udp 3443
ENTRYPOINT ["/usr/local/bin/backend"]
