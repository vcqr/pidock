# PiDock server stack（WSL / Docker）

```bash
cd docker
# 只起基础设施（kafka + mongo + redis + rustfs）
docker compose up -d

# 起 server（首次会编译 Rust 镜像，几分钟）
docker compose up -d --build server

# 状态 / 日志
docker compose ps
docker compose logs -f server
```

端口（映射到 localhost，Windows 宿主可直接访问）：

| 服务 | 端口 | 说明 |
|---|---|---|
| server | 8080 | HTTP API + WS（/ws/desktop、/ws/web） |
| kafka | 9092 | 仅调试用（server 内部走 kafka:29092） |
| mongo | 27017 | 调试用 |
| redis | 6379 | 运行时状态（无持久化，状态可自愈） |
| rustfs | 7000 | S3 兼容对象存储（附件） |

凭据在 `.env`（**开发默认值，上生产前必须改**）。

流水线切换：`PIDOCK_PIPELINE=kafka`（默认）或 `redis`（Redis Streams 平替，
接口一致：单 stream 内有序 = 单分区有序，按 machine_id 分片）。

搬到 VPS：改 `.env` 凭据 + 前面加 Caddy 做 WSS；RustFS 可换云 S3（只改
endpoint/密钥）。
