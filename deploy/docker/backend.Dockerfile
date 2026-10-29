FROM golang:1.25-bookworm AS build
ARG GOPROXY=https://proxy.golang.org,direct
ENV GOPROXY=$GOPROXY
WORKDIR /src/backend
COPY backend/go.mod backend/go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod go mod download
COPY backend/ ./
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/relayai-backend .

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl openssl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /out/relayai-backend /app/relayai-backend
COPY deploy/docker/backend.yaml /app/etc/backend-api.yaml
COPY deploy/docker/init-secrets.sh deploy/docker/service-entrypoint.sh /app/docker/
EXPOSE 8888
ENTRYPOINT ["/bin/sh", "/app/docker/service-entrypoint.sh"]
CMD ["/app/relayai-backend", "-f", "/app/etc/backend-api.yaml"]
