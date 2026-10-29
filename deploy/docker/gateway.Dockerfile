FROM rust:1.96-bookworm AS build
ARG CARGO_MIRROR=
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src/gateway
RUN if [ -n "$CARGO_MIRROR" ]; then \
      mkdir -p /usr/local/cargo \
      && printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' \
        "$CARGO_MIRROR" > /usr/local/cargo/config.toml; \
    fi
COPY gateway/Cargo.toml gateway/Cargo.lock ./
COPY gateway/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/gateway/target \
    cargo build --release --locked \
    && mkdir -p /out \
    && cp target/release/relayai-gateway /out/relayai-gateway

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /out/relayai-gateway /app/relayai-gateway
COPY deploy/docker/service-entrypoint.sh /app/docker/service-entrypoint.sh
EXPOSE 18788
ENTRYPOINT ["/bin/sh", "/app/docker/service-entrypoint.sh"]
CMD ["/app/relayai-gateway"]
