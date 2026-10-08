# PiDock server image (builds from the repo root context).
# web 前端在 node stage 构建并在 cargo build 时嵌入 pidock-server，产物是单文件，
# 容器无需再挂 PIDOCK_WEB_DIR（仍可用该 env 指向外部目录覆盖）。

FROM node:22-slim AS web
WORKDIR /src
RUN corepack enable
# pnpm frozen-lockfile 要求 lockfile 里全部 importer 的 package.json 在场
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
COPY apps/desktop/package.json apps/desktop/package.json
COPY host/package.json host/package.json
COPY packages/protocol/package.json packages/protocol/package.json
COPY packages/ui/package.json packages/ui/package.json
RUN pnpm install --frozen-lockfile --filter @pidock/web...
COPY packages ./packages
COPY apps/web ./apps/web
RUN pnpm --filter @pidock/web build

# bookworm 变体：与运行时 debian:bookworm-slim 的 glibc 匹配（裸 -slim 已是
# trixie 基底，编出的二进制在 bookworm 上报 GLIBC_2.39 not found）
FROM rust:1.96-bookworm-slim AS build
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
COPY --from=web /src/apps/web/dist ./apps/web/dist
RUN cargo build --release -p pidock-server

# trixie：与构建阶段 rust:1.96-slim（trixie 基底）的 glibc 对齐
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/pidock-server /usr/local/bin/pidock-server
EXPOSE 8080
CMD ["pidock-server"]
