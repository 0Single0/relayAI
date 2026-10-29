# syntax=docker/dockerfile:1
FROM node:24-bookworm-slim AS build
RUN npm install --global pnpm@11.21.0
WORKDIR /src/frontend
COPY frontend/package.json frontend/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/pnpm/store \
    pnpm install --frozen-lockfile --store-dir /pnpm/store
COPY frontend/ ./
ENV VITE_BASE_URL=/api/v1
RUN pnpm build

FROM nginx:1.28-alpine
COPY deploy/docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /src/frontend/dist /usr/share/nginx/html
EXPOSE 80
