# Build: docker build -t my-bot .
# Static musl binary in an empty image: nothing to install at runtime, starts in milliseconds.
FROM rust:1.100.0-alpine3.22 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM scratch
COPY --from=build /src/target/release/mechbot /mechbot
USER 65534:65534
ENTRYPOINT ["/mechbot"]
