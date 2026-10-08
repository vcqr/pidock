# PiDock webhost image (builds from the repo root context).
# 无头 Web 接入形态：桌面核心（pidock-core）+ 浏览器版桌面 UI + pi-host sidecar。
# 单容器 = 静态 UI + WS IPC 桥 + 本地 pi agent 运行时，浏览器打开即用。

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
RUN pnpm install --frozen-lockfile --filter @pidock/desktop... --filter @pidock/host...
COPY packages ./packages
COPY apps/desktop ./apps/desktop
COPY host ./host
# 桌面前端（webhost 编译期嵌入）
RUN pnpm --filter @pidock/desktop build
# pi-host 单文件（bun compile；npm 全局 bun 在容器内可正常跑 postinstall）
RUN npm install -g bun \
 && cd host \
 && bun build ./src/main.ts --compile --outfile /out/pidock-host

# rust:1.96-slim 已是 trixie 基底，运行时必须同为 trixie（bookworm 的 glibc
# 过老，会报 GLIBC_2.39 not found）
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
# workspace 全体成员 manifest 必须在场才能解析 workspace
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps/server ./apps/server
COPY apps/desktop/src-tauri ./apps/desktop/src-tauri
COPY apps/webhost ./apps/webhost
# 桌面前端产物（rust-embed release 构建真嵌入）
COPY --from=web /src/apps/desktop/dist ./apps/desktop/dist
RUN cargo build --release -p pidock-webhost

FROM debian:trixie-slim
# ca-certificates: pi-host 访问 LLM API；tzdata: 定时任务本地时区（容器内默认 UTC，设 TZ 覆盖）
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*
# HOME=/data：pi agent 目录（/data/.pi）与 webhost 数据目录（/data/.pidock-web）
# 都落在同一个可挂卷的目录里，重建容器不丢会话与配置
ENV HOME=/data \
    PIDOCK_WEB_BIND=0.0.0.0:8091
RUN mkdir -p /data
VOLUME ["/data"]
COPY --from=build /build/target/release/pidock-webhost /app/pidock-webhost
COPY --from=web /out/pidock-host /app/pidock-host
WORKDIR /app
EXPOSE 8091
CMD ["/app/pidock-webhost"]
