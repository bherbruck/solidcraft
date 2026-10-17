# syntax=docker/dockerfile:1
# SolidCraft in the browser as a container: the static site (the one packaging/web/package.sh
# zips) served by nginx on port 8080, as an unprivileged user. From the repository root:
#
#   docker build -f packaging/docker/Dockerfile -t solidcraft-web .
#   docker run --rm -p 8080:8080 solidcraft-web          then open http://localhost:8080/
#
# Or `docker compose up` with packaging/docker/compose.yaml. Saving designs needs HTTPS or
# localhost (the browser's private file storage), so put a TLS reverse proxy in front of it on a
# server; the site works under any URL path.
#
# The site is wasm, the same on every platform, so it is built once on the build machine's own
# platform even for a multi-platform image. CI builds it outside Docker (with its Rust cache and
# git commit for Help › About) and passes it in as the `site` stage: --build-context site=dist/web.

ARG RUST_VERSION=1.95
ARG TRUNK_VERSION=0.21.14

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-bookworm AS build
ARG TRUNK_VERSION
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
 && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-$(uname -m)-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin trunk
WORKDIR /src
COPY . .
WORKDIR /src/apps/solidcraft-web
# Trunk downloads the wasm-bindgen and wasm-opt its build needs; its post-build hook writes sw.js.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    trunk build --release

FROM scratch AS site
COPY --from=build /src/dist/web/ /

FROM nginxinc/nginx-unprivileged:1.29-alpine
COPY packaging/docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY LICENSE-MIT LICENSE-APACHE NOTICE THIRD-PARTY-LICENSES.txt /usr/share/doc/solidcraft/
COPY --from=site / /srv/solidcraft/
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s CMD wget -q --spider http://127.0.0.1:8080/ || exit 1
