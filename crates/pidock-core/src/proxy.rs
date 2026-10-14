//! 设置中心「常规 · 代理设置」的统一出口。
//!
//! 同一份配置（`~/.pi/agent/pidock/settings.json` 的 `proxy` 字段）有三类消费方：
//! - pi-host 子进程：spawn 时转成环境变量注入（Bun fetch 遵循 HTTP(S)_PROXY）
//! - core 进程内 HTTP（云同步）：`reqwest_client()` 构建的共享客户端
//! - core 进程内 WebSocket（云同步上行）：`connect_ws()` 走 pidock-net 的
//!   CONNECT 隧道（tokio-tungstenite 自身不支持代理）
//!
//! 代理属于进程级配置，修改后需重启应用生效。主进程在配置缺失时保持历史
//! 行为（HTTP 认进程环境变量，WS 直连）；配置存在时行为完全由配置决定
//!（mode=direct 显式屏蔽环境变量代理）。

use std::path::PathBuf;

use pidock_net::WsConnect;

// ------------------------------------------------------------------ 配置读取 ---

#[derive(Debug, PartialEq)]
pub(crate) struct ProxyConf {
    pub mode: String,
    pub url: Option<String>,
    pub no_proxy: Option<String>,
    pub ca_path: Option<String>,
}

pub(crate) fn load_proxy_conf() -> Option<ProxyConf> {
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let path = std::path::Path::new(&base)
        .join(".pi")
        .join("agent")
        .join("pidock")
        .join("settings.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let p = v.get("proxy")?;
    Some(ProxyConf {
        mode: p.get("mode")?.as_str()?.to_string(),
        url: p.get("url").and_then(|v| v.as_str()).map(str::to_string),
        no_proxy: p
            .get("noProxy")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ca_path: p.get("caPath").and_then(|v| v.as_str()).map(str::to_string),
    })
}

/// 解析 `reg query` 的值行（`    Name    REG_SZ    value`），取最后一列
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_reg_value(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        if !key.eq_ignore_ascii_case(name) {
            return None;
        }
        parts.next()?; // REG_SZ / REG_DWORD
        let value = parts.next()?; // 值本身不含空格（代理地址/0x1）
        Some(value.to_string())
    })
}

/// 解析 WinINET ProxyServer：`host:port` 或 `http=…;https=…;ftp=…`（优先 https）
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let picked = if raw.contains('=') {
        let mut https = None;
        let mut http = None;
        for pair in raw.split(';') {
            let (k, v) = pair.split_once('=')?;
            match k.trim().to_ascii_lowercase().as_str() {
                "https" => https = Some(v.trim().to_string()),
                "http" => http = Some(v.trim().to_string()),
                _ => {}
            }
        }
        https.or(http)?
    } else {
        raw.to_string()
    };
    if picked.is_empty() {
        return None;
    }
    // scheme 补全（socks= 前缀的场景上面未命中，直接带上 http 也可被 Bun 解析）
    if picked.contains("://") {
        Some(picked)
    } else {
        Some(format!("http://{picked}"))
    }
}

/// 代理绕过列表合并：用户列表 + （跟随系统时的）系统 ProxyOverride
fn merge_no_proxy(user: Option<&str>, system: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for source in [user, system].into_iter().flatten() {
        for item in source.split([',', ';']) {
            let item = item.trim();
            if item.is_empty() || parts.iter().any(|p| p.eq_ignore_ascii_case(item)) {
                continue;
            }
            parts.push(item.to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

// ------------------------------------------------------------------ 系统代理 ---

/// Windows 系统代理（WinINET）：读注册表 ProxyEnable/ProxyServer/ProxyOverride
#[cfg(target_os = "windows")]
fn system_proxy() -> Option<(String, Option<String>)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let query = |v: &str| -> Option<String> {
        let out = std::process::Command::new("reg")
            .args(["query", key, "/v", v])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_reg_value(&String::from_utf8_lossy(&out.stdout), v)
    };
    if query("ProxyEnable")?.to_ascii_lowercase() != "0x1" {
        return None;
    }
    let server = parse_proxy_server(&query("ProxyServer")?)?;
    let bypass = query("ProxyOverride");
    Some((server, bypass))
}

/// macOS 系统代理：解析 `scutil --proxy`（GUI 进程拿不到 shell 环境变量，
/// 系统设置里的代理只能走 SystemConfiguration）。HTTPS 优先，回退 HTTP；
/// ExceptionsList 并入绕过列表。PAC（ProxyAutoConfigEnable）暂不支持。
#[cfg(target_os = "macos")]
fn system_proxy() -> Option<(String, Option<String>)> {
    let out = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_scutil_proxy(&String::from_utf8_lossy(&out.stdout))
}

/// 解析 `scutil --proxy` 的键值/数组结构：
/// `Key : value` 与 ExceptionsList 的多行数组块（`N : value` 直到 `}`）
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_scutil_proxy(text: &str) -> Option<(String, Option<String>)> {
    let mut keys: std::collections::HashMap<String, String> = Default::default();
    let mut exceptions: Vec<String> = Vec::new();
    let mut in_exceptions = false;
    for line in text.lines() {
        let line = line.trim();
        if line.ends_with('{') {
            in_exceptions = line.starts_with("ExceptionsList");
            continue;
        }
        if line.starts_with('}') {
            in_exceptions = false;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if in_exceptions {
            // 数组条目形如 `0 : *.local`，键是索引，值即例外项
            if !value.is_empty() {
                exceptions.push(value);
            }
            continue;
        }
        keys.insert(key, value);
    }
    let enabled = |name: &str| keys.get(name).map(|v| v == "1").unwrap_or(false);
    let server_of = |enable: &str, host: &str, port: &str| -> Option<String> {
        if !enabled(enable) {
            return None;
        }
        let host = keys.get(host)?.trim().to_string();
        if host.is_empty() {
            return None;
        }
        let port = keys.get(port).and_then(|p| p.parse::<u16>().ok());
        Some(match port {
            Some(p) => format!("http://{host}:{p}"),
            None => format!("http://{host}"),
        })
    };
    let server = server_of("HTTPSEnable", "HTTPSProxy", "HTTPSPort")
        .or_else(|| server_of("HTTPEnable", "HTTPProxy", "HTTPPort"))?;
    let bypass = (!exceptions.is_empty()).then(|| exceptions.join(","));
    Some((server, bypass))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_proxy() -> Option<(String, Option<String>)> {
    // 其他平台暂不读取桌面环境代理设置
    None
}

// ------------------------------------------------------------------ 统一解析 ---

/// 按消费方归一的代理解析结果；`proxy_url` 为 None 且本结构存在 = 显式直连
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub proxy_url: Option<String>,
    pub no_proxy: Option<String>,
    pub ca_path: Option<String>,
}

/// 读取并解析代理设置。返回 None = 用户从未保存过设置（保持历史行为）；
/// 返回 Some 且 proxy_url 为 None = 显式直连。
pub fn resolve() -> Option<Resolved> {
    Some(resolve_conf(load_proxy_conf()?))
}

/// 纯解析逻辑（与磁盘读取分离，便于测试）
fn resolve_conf(conf: ProxyConf) -> Resolved {
    let mut no_proxy = conf.no_proxy.clone();
    let proxy_url = match conf.mode.as_str() {
        "http" => conf.url.clone().filter(|u| !u.trim().is_empty()),
        "system" => match system_proxy() {
            Some((url, bypass)) => {
                no_proxy = merge_no_proxy(conf.no_proxy.as_deref(), bypass.as_deref());
                Some(url)
            }
            // 系统未开代理：保持直连
            None => None,
        },
        _ => None, // direct
    };
    Resolved {
        proxy_url,
        no_proxy: pidock_net::with_loopback(no_proxy.as_deref()),
        ca_path: conf.ca_path,
    }
}

// ------------------------------------------------------------------ HTTP ---

/// core 进程内共享 HTTP 客户端：应用代理设置与企业 CA。
/// 每次调用重新读盘解析（设置变更 + 应用重启后自然生效）。
pub fn reqwest_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if let Some(r) = resolve() {
        match &r.proxy_url {
            Some(url) => {
                if let Ok(proxy) = reqwest::Proxy::all(url) {
                    let proxy = match r.no_proxy.as_deref() {
                        Some(list) => match reqwest::NoProxy::from_string(list) {
                            Some(np) => proxy.no_proxy(Some(np)),
                            None => proxy,
                        },
                        None => proxy,
                    };
                    builder = builder.proxy(proxy);
                } else {
                    eprintln!("[proxy] 代理地址无法解析，忽略：{url}");
                }
            }
            // 显式直连：屏蔽继承的环境变量代理，与 pi-host 注入语义一致
            None => builder = builder.no_proxy(),
        }
        if let Some(ca) = ca_file(&r.ca_path) {
            builder = builder.add_root_certificate(ca);
        }
    }
    builder.build().unwrap_or_default()
}

// ------------------------------------------------------------------ WebSocket ---

/// 云同步 WS 上行：配置了代理时经 CONNECT 隧道，并追加企业 CA 到 rustls 根证书
pub async fn connect_ws(
    ws_url: &str,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String>
{
    let (proxy_url, no_proxy, ca) = match resolve() {
        Some(r) => (
            r.proxy_url,
            r.no_proxy,
            r.ca_path.map(PathBuf::from).into_iter().collect::<Vec<_>>(),
        ),
        None => (None, None, Vec::new()),
    };
    WsConnect {
        proxy_url,
        no_proxy,
        ca_pem_paths: ca,
    }
    .connect(ws_url)
    .await
}

// ------------------------------------------------------------------ pi-host 注入 ---

/// 代理模式下需要先清掉的继承变量：mode=direct 必须真正直连
pub(crate) fn proxy_clear_keys() -> &'static [&'static str] {
    &[
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "NODE_EXTRA_CA_CERTS",
    ]
}

/// 由代理配置计算要注入 pi-host 的环境变量
pub fn proxy_env_overrides() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(r) = resolve() else {
        return out;
    };
    if let Some(url) = &r.proxy_url {
        out.push(("HTTP_PROXY".into(), url.clone()));
        out.push(("HTTPS_PROXY".into(), url.clone()));
    }
    if let Some(list) = &r.no_proxy {
        if !list.trim().is_empty() {
            out.push(("NO_PROXY".into(), list.clone()));
            out.push(("no_proxy".into(), list.clone()));
        }
    }
    if let Some(ca) = &r.ca_path {
        let ca = ca.trim();
        if !ca.is_empty() && std::path::Path::new(ca).is_file() {
            out.push(("NODE_EXTRA_CA_CERTS".into(), ca.to_string()));
        } else {
            eprintln!("[supervisor] proxy.caPath 不存在，忽略：{ca}");
        }
    }
    out
}

fn ca_file(ca_path: &Option<String>) -> Option<reqwest::tls::Certificate> {
    let ca = ca_path.as_deref()?.trim();
    if ca.is_empty() {
        return None;
    }
    match std::fs::read(ca) {
        Ok(bytes) => match reqwest::tls::Certificate::from_pem(&bytes) {
            Ok(cert) => Some(cert),
            Err(e) => {
                eprintln!("[proxy] proxy.caPath 解析失败，忽略：{ca} ({e})");
                None
            }
        },
        Err(_) => {
            eprintln!("[proxy] proxy.caPath 不存在，忽略：{ca}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_server_handles_both_formats() {
        assert_eq!(
            parse_proxy_server("127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            parse_proxy_server("http=10.0.0.1:8080;https=10.0.0.1:8443;ftp=10.0.0.1:21").as_deref(),
            Some("http://10.0.0.1:8443")
        );
        assert_eq!(parse_proxy_server(""), None);
        assert_eq!(parse_proxy_server("https="), None);
    }

    #[test]
    fn no_proxy_merges_and_dedupes() {
        assert_eq!(
            merge_no_proxy(
                Some("localhost, 127.0.0.1"),
                Some("*.local;localhost;<local>")
            )
            .as_deref(),
            Some("localhost,127.0.0.1,*.local,<local>")
        );
        assert_eq!(merge_no_proxy(None, None), None);
    }

    #[test]
    fn reg_value_parses_last_column() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x1\r\n\r\n";
        assert_eq!(parse_reg_value(out, "ProxyEnable").as_deref(), Some("0x1"));
        assert_eq!(parse_reg_value(out, "ProxyServer"), None);
    }

    /// scutil --proxy 输出（macOS Sequoia 实测格式）：HTTPS 优先，数组并入
    #[test]
    fn scutil_proxy_parses_keys_and_exceptions() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 1\n  HTTPSPort : 8443\n  HTTPSProxy : 10.0.0.2\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n    2 : .corp.example\n    3 : localhost\n    4 : 127.0.0.1\n  }\n  SOCKSEnable : 0\n}\n";
        let (server, bypass) = parse_scutil_proxy(out).unwrap();
        assert_eq!(server, "http://10.0.0.2:8443");
        assert_eq!(
            bypass.as_deref(),
            Some("*.local,169.254/16,.corp.example,localhost,127.0.0.1")
        );
    }

    #[test]
    fn scutil_proxy_falls_back_to_http_and_disabled_means_none() {
        let http_only = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(http_only).unwrap().0,
            "http://127.0.0.1:7890"
        );
        let disabled = "<dictionary> {\n  HTTPEnable : 0\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 7890\n  SOCKSEnable : 0\n}\n";
        assert!(parse_scutil_proxy(disabled).is_none());
        let socks_only = "<dictionary> {\n  SOCKSEnable : 1\n  SOCKSProxy : 127.0.0.1\n  SOCKSPort : 1080\n}\n";
        assert!(parse_scutil_proxy(socks_only).is_none());
    }

    /// resolve：direct 模式 = 显式直连（proxy_url None 但结构存在），
    /// 且 no_proxy 强制携带回环例外
    #[test]
    fn resolve_direct_marks_explicit_direct_with_loopback() {
        let r = resolve_conf(ProxyConf {
            mode: "direct".into(),
            url: None,
            no_proxy: Some("example.com".into()),
            ca_path: None,
        });
        assert_eq!(r.proxy_url, None);
        assert_eq!(
            r.no_proxy.as_deref(),
            Some("localhost,127.0.0.1,::1,example.com")
        );
    }

    /// resolve：http 模式透传 url；system 模式解析不到系统代理时回落直连
    #[test]
    fn resolve_http_and_system_modes() {
        let r = resolve_conf(ProxyConf {
            mode: "http".into(),
            url: Some("http://10.0.0.1:8080".into()),
            no_proxy: None,
            ca_path: Some("/tmp/no-such-ca.pem".into()),
        });
        assert_eq!(r.proxy_url.as_deref(), Some("http://10.0.0.1:8080"));
        assert_eq!(r.no_proxy.as_deref(), Some("localhost,127.0.0.1,::1"));
        assert_eq!(r.ca_path.as_deref(), Some("/tmp/no-such-ca.pem"));
        let empty_url = resolve_conf(ProxyConf {
            mode: "http".into(),
            url: Some("  ".into()),
            no_proxy: None,
            ca_path: None,
        });
        assert_eq!(empty_url.proxy_url, None); // 空 url = 直连
    }
}
