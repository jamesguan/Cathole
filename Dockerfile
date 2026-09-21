FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 cathole
COPY --from=builder /app/target/release/cathole /usr/local/bin/cathole
USER 10001
ENTRYPOINT ["/usr/local/bin/cathole"]
CMD ["/config/server.toml"]
