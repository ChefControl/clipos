# syntax=docker/dockerfile:1.7
# clipos api: Rust server + the built SPA, served from one origin.
#
# Builds everything from source. deploy.yml builds the binary and the SPA on the runner
# instead (with warm caches) and passes them in as the `bin` and `spa` stages:
#   --build-context bin=<dir with clipos-api> --build-context spa=web/dist

FROM node:26-trixie-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,id=pnpm-store,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM scratch AS spa
COPY --from=web /src/web/dist /

FROM rust:1.98-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p clipos-api --bin clipos-api \
    && cp target/release/clipos-api /usr/local/bin/clipos-api

FROM scratch AS bin
COPY --from=build /usr/local/bin/clipos-api /clipos-api

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home clipos
COPY --from=bin /clipos-api /usr/local/bin/clipos-api
COPY --from=spa / /srv/web
ENV STATIC_DIR=/srv/web \
    BIND_ADDR=0.0.0.0:8080 \
    LOG_FORMAT=json
# Set by deploy.yml to the git SHA; reported in the x-clipos-version header on /healthz.
ARG CLIPOS_VERSION=dev
ENV CLIPOS_VERSION=${CLIPOS_VERSION}
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/clipos-api"]
