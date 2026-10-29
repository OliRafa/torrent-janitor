FROM rust:1.99.1-alpine AS build

# Alpine's Rust targets musl and links statically by default, so the binary
# needs nothing from the final image. reqwest is built without TLS: every
# endpoint is plain HTTP inside the cluster.
RUN apk add --no-cache musl-dev

WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked && cp target/release/torrent-janitor /torrent-janitor

FROM gcr.io/distroless/static-debian12:nonroot

COPY --from=build /torrent-janitor /torrent-janitor

EXPOSE 9090
USER nonroot:nonroot
ENTRYPOINT ["/torrent-janitor"]
