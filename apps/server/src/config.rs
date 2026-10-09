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
    /// CORS 白名单 origin（如 https://pidock.example.com）；留空 = permissive。
    /// 公网部署建议配置；反代同域部署可不配（同源请求不受 CORS 约束）
    pub web_origin: String,
    /// 反代后部署置 true：限流取 X-Forwarded-For 的客户端 IP；默认取直连对端
    pub trust_proxy: bool,
    /// Cloudflare Turnstile 人机校验：site key 下发前端渲染 widget，secret 留服务端
    /// 调 siteverify；两者都非空才启用，留空 = 关闭（登录/注册行为不变）
    pub turnstile_site_key: String,
    pub turnstile_secret_key: String,
    pub email: EmailCfg,
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

/// SMTP 邮件配置（登录邮箱验证码）。开关语义：login_code = true 且 smtp_host
/// 非空才启用；只开开关不配 SMTP 视为关闭，启动打警告（同 Turnstile 模式）。
#[derive(Clone, Debug, Deserialize)]
pub struct EmailCfg {
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default)]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_user: String,
    #[serde(default)]
    pub smtp_pass: String,
    /// tls（隐式 TLS，465）| starttls（587）| none（明文，仅内网调试）
    #[serde(default)]
    pub smtp_tls: String,
    /// 发件人，如 "PiDock <noreply@example.com>"；留空回落到 smtp_user
    #[serde(default)]
    pub from: String,
    /// 登录邮箱验证码（两步验证）开关
    #[serde(default)]
    pub login_code: bool,
    /// 注册邮箱验证码开关：注册先验邮箱所有权再建号（开放注册时建议开启）
    #[serde(default)]
    pub register_code: bool,
}

impl Default for EmailCfg {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: 465,
            smtp_user: String::new(),
            smtp_pass: String::new(),
            smtp_tls: "tls".into(),
            from: String::new(),
            login_code: false,
            register_code: false,
        }
    }
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
    email: EmailCfg,
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
    #[serde(default)]
    turnstile_site_key: String,
    #[serde(default)]
    turnstile_secret_key: String,
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
    #[serde(default)]
    origin: String,
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
            turnstile_site_key: pick("PIDOCK_TURNSTILE_SITE_KEY", &file.server.turnstile_site_key, ""),
            turnstile_secret_key: pick(
                "PIDOCK_TURNSTILE_SECRET_KEY",
                &file.server.turnstile_secret_key,
                "",
            ),
            email: EmailCfg {
                smtp_host: pick("PIDOCK_EMAIL_SMTP_HOST", &file.email.smtp_host, ""),
                smtp_port: {
                    let v = pick(
                        "PIDOCK_EMAIL_SMTP_PORT",
                        &file.email.smtp_port.to_string(),
                        "465",
                    );
                    v.parse().unwrap_or(465)
                },
                smtp_user: pick("PIDOCK_EMAIL_SMTP_USER", &file.email.smtp_user, ""),
                smtp_pass: pick("PIDOCK_EMAIL_SMTP_PASS", &file.email.smtp_pass, ""),
                smtp_tls: pick("PIDOCK_EMAIL_SMTP_TLS", &file.email.smtp_tls, "tls"),
                from: pick("PIDOCK_EMAIL_FROM", &file.email.from, ""),
                login_code: {
                    let v = pick(
                        "PIDOCK_EMAIL_LOGIN_CODE",
                        &file.email.login_code.to_string(),
                        "false",
                    );
                    v.eq_ignore_ascii_case("true") || v == "1"
                },
                register_code: {
                    let v = pick(
                        "PIDOCK_EMAIL_REGISTER_CODE",
                        &file.email.register_code.to_string(),
                        "false",
                    );
                    v.eq_ignore_ascii_case("true") || v == "1"
                },
            },
            web_dir: pick("PIDOCK_WEB_DIR", &file.web.dir, ""),
            web_origin: pick("PIDOCK_WEB_ORIGIN", &file.web.origin, ""),
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

        match (
            cfg.turnstile_site_key.trim().is_empty(),
            cfg.turnstile_secret_key.trim().is_empty(),
        ) {
            (false, false) => tracing::info!("config: Turnstile 登录人机校验已启用"),
            (false, true) | (true, false) => tracing::warn!(
                "config: Turnstile 只配置了 site_key / secret_key 其中一个，视为关闭（两个都配置才启用）"
            ),
            (true, true) => {}
        }

        if (cfg.email.login_code || cfg.email.register_code) && cfg.email.smtp_host.trim().is_empty()
        {
            tracing::warn!(
                "config: email.login_code / email.register_code 已开启但未配置 SMTP（email.smtp_host），邮箱验证码视为关闭"
            );
        } else if cfg.email.login_code || cfg.email.register_code {
            tracing::info!(
                "config: 邮箱验证码已启用（登录={}，注册={}；SMTP {}:{}，tls 模式 {}）",
                cfg.email.login_code,
                cfg.email.register_code,
                cfg.email.smtp_host,
                cfg.email.smtp_port,
                cfg.email.smtp_tls
            );
        }

        ensure_jwt_secret(&cfg.jwt_secret, &cfg.bind)?;

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

/// jwt_secret 仍为内置默认值（或空）时的启动门槛：非本地绑定直接拒绝启动——
/// 密钥是开源默认值时，公网暴露等同任何人可离线伪造任意用户/管理员的会话；
/// 本地回环放行但打警告，保住开箱即用的开发体验
fn ensure_jwt_secret(secret: &str, bind: &str) -> anyhow::Result<()> {
    const DEFAULT_JWT: &str = "dev-secret-change-me";
    let unset = secret.trim().is_empty() || secret == DEFAULT_JWT;
    if !unset {
        return Ok(());
    }
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let local = matches!(host, "" | "localhost" | "127.0.0.1" | "[::1]" | "::1");
    if local {
        tracing::warn!(
            "config: jwt_secret 为内置默认值，仅限本地开发使用；对外服务必须更换（openssl rand -hex 32）"
        );
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "拒绝启动：jwt_secret 仍为内置默认值，而监听地址 {bind} 非本地回环，\
             公网暴露等同任何人可伪造会话。请在 pidock.toml [server] jwt_secret \
             或环境变量 PIDOCK_JWT_SECRET 配置强随机密钥（openssl rand -hex 32）"
        ))
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

    /// jwt 启动门槛：默认密钥 + 非本地绑定拒绝；本地回环放行；显式配置放行
    #[test]
    fn jwt_gate_blocks_default_secret_on_nonlocal_bind() {
        assert!(ensure_jwt_secret("dev-secret-change-me", "0.0.0.0:8080").is_err());
        assert!(ensure_jwt_secret("", "0.0.0.0:8080").is_err());
        assert!(ensure_jwt_secret("dev-secret-change-me", "127.0.0.1:8080").is_ok());
        assert!(ensure_jwt_secret("dev-secret-change-me", "localhost:8080").is_ok());
        assert!(ensure_jwt_secret("a-real-random-secret", "0.0.0.0:8080").is_ok());
    }
}
