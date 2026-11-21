# Build: docker build -t my-bot .
FROM rust:1.100.0-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
COPY --from=build /src/target/release/mechbot /usr/local/bin/mechbot
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/mechbot"]
