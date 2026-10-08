//! Server configuration：优先级 环境变量 > 配置文件 > 内置默认值。
//! 配置文件定位：--config 参数 > PIDOCK_CONFIG 环境变量 > 当前目录 pidock.toml。
//! 文件是 TOML，分节镜像环境变量（见 pidock.example.toml），缺节/缺字段 = 未设置。

use std::env;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    pub mongo_uri: String,
    pub redis_url: String,
    // consumed by the kafka pipeline (feature-gated)
    #[allow(dead_code)]
    pub kafka_brokers: String,
    #[allow(dead_code)]
    pub kafka_topic: String,
    pub pipeline: String,
    pub jwt_secret: String,
    /// 首个管理员的引导邀请码：用它注册即成为 admin；一旦存在任何管理员即失效
    pub bootstrap_invite: String,
    pub web_dir: String,
    /// 反代后部署置 true：限流取 X-Forwarded-For 的客户端 IP；默认取直连对端
    pub trust_proxy: bool,
    pub rustfs: RustfsCfg,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RustfsCfg {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
}

/// pidock.toml 的 schema；字段缺省为空串 = 文件里未设置
#[derive(Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    server: ServerSection,
    mongo: MongoSection,
    redis: RedisSection,
    kafka: KafkaSection,
    rustfs: RustfsCfg,
    web: WebSection,
}

#[derive(Default, Deserialize)]
struct ServerSection {
    #[serde(default)]
    bind: String,
    #[serde(default)]
    pipeline: String,
    #[serde(default)]
    jwt_secret: String,
    #[serde(default)]
    bootstrap_invite: String,
    #[serde(default)]
    trust_proxy: String,
}

#[derive(Default, Deserialize)]
struct MongoSection {
    #[serde(default)]
    uri: String,
}

#[derive(Default, Deserialize)]
struct RedisSection {
    #[serde(default)]
    url: String,
}

#[derive(Default, Deserialize)]
struct KafkaSection {
    #[serde(default)]
    brokers: String,
    #[serde(default)]
    topic: String,
}

#[derive(Default, Deserialize)]
struct WebSection {
    #[serde(default)]
    dir: String,
}

impl Config {
    pub fn load(cli_path: Option<String>) -> anyhow::Result<Self> {
        let file_path = Self::locate(cli_path);
        let (file, loaded_from) = match &file_path {
            Some(p) => {
                let raw = std::fs::read_to_string(p)
                    .map_err(|e| anyhow::anyhow!("无法读取配置文件 {p:?}: {e}"))?;
                let f: FileConfig = toml::from_str(&raw)
                    .map_err(|e| anyhow::anyhow!("配置文件解析失败 {p:?}: {e}"))?;
                (f, Some(p.display().to_string()))
            }
            None => (FileConfig::default(), None),
        };

        // env 覆盖 file，file 覆盖 default；file 字段为空串 = 未设置
        let pick = |env_key: &str, file_val: &str, default: &str| -> String {
            env::var(env_key).unwrap_or_else(|_| {
                if file_val.is_empty() {
                    default.to_string()
                } else {
                    file_val.to_string()
                }
            })
        };

        let cfg = Config {
            bind: pick("PIDOCK_BIND", &file.server.bind, "0.0.0.0:8080"),
            mongo_uri: pick(
                "PIDOCK_MONGO_URI",
                &file.mongo.uri,
                "mongodb://localhost:27017",
            ),
            redis_url: pick(
                "PIDOCK_REDIS_URL",
                &file.redis.url,
                "redis://localhost:6379",
            ),
            kafka_brokers: pick(
                "PIDOCK_KAFKA_BROKERS",
                &file.kafka.brokers,
                "localhost:9092",
            ),
            kafka_topic: pick("PIDOCK_KAFKA_TOPIC", &file.kafka.topic, "pidock.events"),
            pipeline: pick("PIDOCK_PIPELINE", &file.server.pipeline, "kafka"),
            jwt_secret: pick(
                "PIDOCK_JWT_SECRET",
                &file.server.jwt_secret,
                "dev-secret-change-me",
            ),
            bootstrap_invite: pick("PIDOCK_BOOTSTRAP_INVITE", &file.server.bootstrap_invite, ""),
            trust_proxy: {
                let v = pick("PIDOCK_TRUST_PROXY", &file.server.trust_proxy, "false");
                v.eq_ignore_ascii_case("true") || v == "1"
            },
            web_dir: pick("PIDOCK_WEB_DIR", &file.web.dir, ""),
            rustfs: RustfsCfg {
                endpoint: pick(
                    "PIDOCK_RUSTFS_ENDPOINT",
                    &file.rustfs.endpoint,
                    "http://localhost:7000",
                ),
                bucket: pick(
                    "PIDOCK_RUSTFS_BUCKET",
                    &file.rustfs.bucket,
                    "pidock-attachments",
                ),
                access_key: pick(
                    "PIDOCK_RUSTFS_ACCESS_KEY",
                    &file.rustfs.access_key,
                    "pidock",
                ),
                secret_key: pick(
                    "PIDOCK_RUSTFS_SECRET_KEY",
                    &file.rustfs.secret_key,
                    "pidock-secret",
                ),
            },
        };

        match loaded_from {
            Some(p) => tracing::info!("config: 已加载配置文件 {p}（环境变量优先于文件值）"),
            None => tracing::info!("config: 未找到配置文件，使用环境变量 / 内置默认值"),
        }
        Ok(cfg)
    }

    fn locate(cli_path: Option<String>) -> Option<PathBuf> {
        if let Some(p) = cli_path {
            return Some(PathBuf::from(p));
        }
        if let Ok(p) = env::var("PIDOCK_CONFIG") {
            return Some(PathBuf::from(p));
        }
        let cwd = PathBuf::from("pidock.toml");
        cwd.exists().then_some(cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 仓库里的示例文件必须始终可解析（发布包随附）
    #[test]
    fn example_file_parses() {
        let raw =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/pidock.example.toml"))
                .unwrap();
        let f: FileConfig = toml::from_str(&raw).unwrap();
        assert_eq!(f.server.bind, "0.0.0.0:8080");
        assert_eq!(f.redis.url, "redis://localhost:6379");
        assert_eq!(f.rustfs.bucket, "pidock-attachments");
        assert!(f.web.dir.is_empty());
        assert!(f.server.bootstrap_invite.is_empty());
    }

    /// 优先级：env > file > default（env 只在本测试内改，别并行读 env）
    #[test]
    fn merge_precedence_env_over_file_over_default() {
        let tmp = std::env::temp_dir().join(format!("pidock-cfg-test-{}.toml", std::process::id()));
        std::fs::write(
            &tmp,
            "[server]\nbind = \"127.0.0.1:9999\"\n[redis]\nurl = \"redis://file-host:6379\"\n",
        )
        .unwrap();
        std::env::set_var("PIDOCK_BIND", "127.0.0.1:7777");
        let cfg = Config::load(Some(tmp.to_string_lossy().into_owned())).unwrap();
        std::env::remove_var("PIDOCK_BIND");
        std::fs::remove_file(&tmp).ok();
        assert_eq!(cfg.bind, "127.0.0.1:7777"); // env 赢过 file
        assert_eq!(cfg.redis_url, "redis://file-host:6379"); // file 赢过 default
        assert_eq!(cfg.mongo_uri, "mongodb://localhost:27017"); // 全缺 → default
    }

    /// 文件不存在且显式指定 → 报错而不是静默用默认值
    #[test]
    fn explicit_missing_file_errors() {
        assert!(Config::load(Some("definitely/nope.toml".into())).is_err());
    }
}
