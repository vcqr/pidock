//! 服务端出网统一出口：出站代理 + 企业 CA。
//!
//! 配置优先级：pidock.toml `[proxy]` / 环境变量 PIDOCK_PROXY_* > 标准
//! HTTPS_PROXY/HTTP_PROXY/NO_PROXY 环境变量 > 直连。reqwest 的 HTTP 请求
//! 默认就认标准环境变量，这里在显式配置时接管（行为可预期），并统一追加
//! ca_path（rustls 不读系统信任库，企业 MITM 部署必须显式提供）。
//! IM 长连接（钉钉 Stream / 飞书 WS）走 pidock-net 的 CONNECT 隧道——
//! tokio-tungstenite 自身不具备代理能力。

use std::path::PathBuf;

use crate::config::Config;

/// 归一后的出站代理设置
pub struct ProxyConf {
    pub proxy_url: Option<String>,
    pub no_proxy: Option<String>,
    pub ca_paths: Vec<PathBuf>,
}

/// 解析出站代理：显式配置优先，未配置时回落标准环境变量
pub fn proxy_conf(proxy_url: &str, proxy_no_proxy: &str, proxy_ca_path: &str) -> ProxyConf {
    let mut url = proxy_url.trim().to_string();
    let mut no_proxy = proxy_no_proxy.trim().to_string();
    let ca = proxy_ca_path.trim().to_string();
    if url.is_empty() {
        // 与 reqwest 的回落一致：https 场景优先 HTTPS_PROXY
        url = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .unwrap_or_default();
    }
    if no_proxy.is_empty() {
        no_proxy = ["NO_PROXY", "no_proxy"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .unwrap_or_default();
    }
    ProxyConf {
        proxy_url: (!url.is_empty()).then_some(url),
        no_proxy: pidock_net::with_loopback((!no_proxy.is_empty()).then_some(no_proxy.as_str())),
        ca_paths: (!ca.is_empty())
            .then(|| vec![PathBuf::from(ca)])
            .unwrap_or_default(),
    }
}

/// 共享 reqwest 客户端构建器：调用方继续追加 timeout 等站点特有配置。
/// 未显式配置代理时不加 proxy，保留 reqwest 的环境变量探测。
pub fn base_client(cfg: &Config) -> Result<reqwest::ClientBuilder, String> {
    let p = proxy_conf(&cfg.proxy_url, &cfg.proxy_no_proxy, &cfg.proxy_ca_path);
    let mut builder = reqwest::Client::builder();
    if let Some(url) = &p.proxy_url {
        let proxy = reqwest::Proxy::all(url)
            .map_err(|e| format!("代理地址无效 {url}: {e}"))?;
        let proxy = match &p.no_proxy {
            Some(list) => match reqwest::NoProxy::from_string(list) {
                Some(np) => proxy.no_proxy(Some(np)),
                None => proxy,
            },
            None => proxy,
        };
        builder = builder.proxy(proxy);
    }
    for path in &p.ca_paths {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("CA 文件 {} 不可读: {e}", path.display()))?;
        let cert = reqwest::tls::Certificate::from_pem(&bytes)
            .map_err(|e| format!("CA 文件 {} 解析失败: {e}", path.display()))?;
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder)
}

/// IM 长连接 WS：命中 no_proxy/回环直连，否则 CONNECT 隧道，并追加企业 CA
pub async fn connect_ws(
    cfg: &Config,
    ws_url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let p = proxy_conf(&cfg.proxy_url, &cfg.proxy_no_proxy, &cfg.proxy_ca_path);
    pidock_net::WsConnect {
        proxy_url: p.proxy_url,
        no_proxy: p.no_proxy,
        ca_pem_paths: p.ca_paths,
    }
    .connect(ws_url)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量改动必须串行（同进程其他测试也会读 env）
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 未显式配置时回落标准环境变量
    #[test]
    fn proxy_conf_falls_back_to_env() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("HTTPS_PROXY", "http://env-proxy:3128");
        std::env::set_var("NO_PROXY", ".internal");
        std::env::remove_var("PIDOCK_PROXY_URL");
        let p = proxy_conf("", "", "");
        std::env::remove_var("HTTPS_PROXY");
        std::env::remove_var("NO_PROXY");
        assert_eq!(p.proxy_url.as_deref(), Some("http://env-proxy:3128"));
        assert_eq!(
            p.no_proxy.as_deref(),
            Some("localhost,127.0.0.1,::1,.internal")
        );
        assert!(p.ca_paths.is_empty());
    }

    /// 显式配置优先于环境变量
    #[test]
    fn proxy_conf_explicit_wins() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PIDOCK_PROXY_URL", "http://explicit:8080");
        std::env::set_var("HTTPS_PROXY", "http://env-proxy:3128");
        let p = proxy_conf("http://explicit:8080", "a.internal", "/tmp/ca.pem");
        std::env::remove_var("PIDOCK_PROXY_URL");
        std::env::remove_var("HTTPS_PROXY");
        assert_eq!(p.proxy_url.as_deref(), Some("http://explicit:8080"));
        assert_eq!(
            p.no_proxy.as_deref(),
            Some("localhost,127.0.0.1,::1,a.internal")
        );
        assert_eq!(p.ca_paths.len(), 1);
    }
}
