# PiDock server image (builds from the repo root context)
FROM rust:1.96-slim AS build
RUN sed -i 's|deb.debian.org|mirrors.tuna.tsinghua.edu.cn|g' /etc/apt/sources.list.d/debian.sources 2>/dev/null; \
    apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config libssl-dev zlib1g-dev \
 && rm -rf /var/lib/apt/lists/*
# crates.io via TUNA sparse mirror (fast in CN; harmless elsewhere)
RUN mkdir -p /usr/local/cargo \
 && printf '[source.crates-io]\nreplace-with = "tuna"\n\n[source.tuna]\nregistry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"\n' \
    > /usr/local/cargo/config.toml
WORKDIR /build
COPY Cargo.toml ./
COPY crates ./crates
COPY apps/server ./apps/server
COPY apps/desktop/src-tauri ./apps/desktop/src-tauri
COPY apps/web/dist /app/web
RUN cargo build --release -p pidock-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/pidock-server /usr/local/bin/pidock-server
ENV PIDOCK_WEB_DIR=/app/web
EXPOSE 8080
CMD ["pidock-server"]
