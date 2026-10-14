//! PiDock 共享出网辅助：带 HTTP CONNECT 代理隧道的 WebSocket 连接。
//!
//! tokio-tungstenite 的 `connect_async` 只会直连目标主机，不具备代理能力；
//! 企业环境要求全部流量经代理时，这里自行对代理发起 CONNECT 建立隧道流，
//! 再交给 tungstenite 在隧道上完成 WS/可选 TLS 握手
//!（`client_async_tls_with_config` 支持传入已建立的流）。
//!
//! no_proxy 语义与 reqwest 的 NO_PROXY 对齐常用形态（精确 / 后缀 / 通配 /
//! `<local>`）；自签与企业根证书经 `ca_pem_paths` 追加进 rustls 根证书库
//!（rustls 不读系统信任库，MITM 类企业代理必须显式提供）。

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use percent_encoding::percent_decode_str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use url::Url;

/// NO_PROXY 里始终隐含的回环例外：发往本机的流量永不走代理
pub const LOOPBACK_NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// WebSocket 连接参数：代理与自签 CA
#[derive(Debug, Clone, Default)]
pub struct WsConnect {
    /// HTTP 代理地址（`http://[user:pass@]host:port`）；None 直连
    pub proxy_url: Option<String>,
    /// 不走代理的主机列表（逗号分隔，语义与 NO_PROXY 一致）；回环地址始终隐含
    pub no_proxy: Option<String>,
    /// 追加信任的自签/企业根证书（PEM 文件路径）
    pub ca_pem_paths: Vec<PathBuf>,
}

impl WsConnect {
    pub fn direct() -> Self {
        Self::default()
    }

    /// 目标主机是否应绕过代理（回环地址始终绕过）
    pub fn should_bypass(&self, host: &str) -> bool {
        if host_matches_no_proxy(host, LOOPBACK_NO_PROXY) {
            return true;
        }
        self.no_proxy
            .as_deref()
            .map(|list| host_matches_no_proxy(host, list))
            .unwrap_or(false)
    }

    /// 解析 ws:// 或 wss:// 并建立连接：命中 no_proxy 或未配代理时直连，
    /// 否则经代理 CONNECT 隧道。返回的流类型与 connect_async 相同，可无缝 split。
    pub async fn connect(
        &self,
        ws_url: &str,
    ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, String> {
        let url = Url::parse(ws_url).map_err(|e| format!("bad ws url {ws_url:?}: {e}"))?;
        let secure = match url.scheme() {
            "ws" => false,
            "wss" => true,
            other => return Err(format!("unsupported ws scheme {other:?}")),
        };
        let host = url
            .host_str()
            .ok_or_else(|| format!("ws url missing host: {ws_url:?}"))?
            .to_string();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| format!("ws url missing port: {ws_url:?}"))?;
        let hostport = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };

        // 代理解析：配置了代理且目标不在绕过列表 → 隧道；否则直连
        let proxy = match &self.proxy_url {
            Some(p) if !p.trim().is_empty() && !self.should_bypass(&host) => {
                Some(Url::parse(p.trim()).map_err(|e| format!("bad proxy url {p:?}: {e}"))?)
            }
            _ => None,
        };

        let stream = match &proxy {
            Some(p) => {
                let proxy_host = p
                    .host_str()
                    .ok_or_else(|| format!("proxy url missing host: {p}"))?
                    .to_string();
                let proxy_port = p
                    .port_or_known_default()
                    .ok_or_else(|| format!("proxy url missing port: {p}"))?;
                let mut tcp = TcpStream::connect((proxy_host.as_str(), proxy_port))
                    .await
                    .map_err(|e| format!("proxy {proxy_host}:{proxy_port} connect: {e}"))?;
                http_connect(&mut tcp, &host, port, p).await?;
                tcp
            }
            None => TcpStream::connect(hostport.as_str())
                .await
                .map_err(|e| format!("{hostport} connect: {e}"))?,
        };

        let request = ws_url
            .into_client_request()
            .map_err(|e| format!("bad ws request: {e}"))?;
        let connector = if secure {
            Connector::Rustls(Arc::new(tls_config(&self.ca_pem_paths)?))
        } else {
            Connector::Plain
        };
        let (ws, _resp) = tokio_tungstenite::client_async_tls_with_config(
            request,
            stream,
            None,
            Some(connector),
        )
        .await
        .map_err(|e| format!("ws handshake: {e}"))?;
        Ok(ws)
    }
}

/// 对代理发起 HTTP CONNECT，建立到 host:port 的隧道。
/// 成功的判据是状态码 2xx；失败时透传代理返回的状态行，便于定位（407 等）。
async fn http_connect(stream: &mut TcpStream, host: &str, port: u16, proxy: &Url) -> Result<(), String> {
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if !proxy.username().is_empty() {
        let user = percent_decode_str(proxy.username()).decode_utf8_lossy();
        let pass = proxy
            .password()
            .map(|p| percent_decode_str(p).decode_utf8_lossy().into_owned())
            .unwrap_or_default();
        let cred = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        req.push_str(&format!("Proxy-Authorization: Basic {cred}\r\n"));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("proxy CONNECT write: {e}"))?;

    // 读到响应头结束；代理对 CONNECT 只回头部，不会吞 WS 握手字节
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("proxy CONNECT read: {e}"))?;
        if n == 0 {
            return Err("proxy closed connection during CONNECT".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err("proxy CONNECT response too large".into());
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    if !status.starts_with('2') {
        let reason = head.lines().next().unwrap_or("").trim().to_string();
        return Err(format!("proxy CONNECT refused: {reason}"));
    }
    Ok(())
}

/// 构造 rustls 客户端配置：webpki 内置根 + 追加的自签/企业根
fn tls_config(ca_pem_paths: &[PathBuf]) -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for path in ca_pem_paths {
        let pem = std::fs::read(path)
            .map_err(|e| format!("ca file {} unreadable: {e}", path.display()))?;
        let mut cursor = std::io::Cursor::new(pem);
        for cert in rustls_pemfile::certs(&mut cursor) {
            let der = cert.map_err(|e| format!("ca file {} parse: {e}", path.display()))?;
            roots
                .add(der)
                .map_err(|e| format!("ca file {} add root: {e}", path.display()))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

/// NO_PROXY 主机匹配（与 reqwest/curl 语义对齐的常用子集）：
/// 精确匹配、后缀（`.example.com` 或 `example.com` 含子域）、
/// 通配前缀（`*.example.com`）、`<local>`（不含点的主机名）、`*` 全部。
/// 逗号/分号分隔；比较不区分大小写。
pub fn host_matches_no_proxy(host: &str, list: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    // IPv6 字面量可能带方括号
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return false;
    }
    for raw in list.split([',', ';']) {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        if entry.eq_ignore_ascii_case("<local>") {
            if !host.contains('.') {
                return true;
            }
            continue;
        }
        if entry == "*" {
            return true;
        }
        let entry = entry
            .trim_start_matches('*')
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if host == entry || host.ends_with(&format!(".{entry}")) {
            return true;
        }
    }
    false
}

/// 在用户 no_proxy 列表上强制附加回环例外；None → 仅回环
pub fn with_loopback(list: Option<&str>) -> Option<String> {
    let mut parts: Vec<&str> = LOOPBACK_NO_PROXY.split(',').collect();
    if let Some(list) = list {
        for item in list.split([',', ';']) {
            let item = item.trim();
            if !item.is_empty() && !parts.iter().any(|p| p.eq_ignore_ascii_case(item)) {
                parts.push(item);
            }
        }
    }
    Some(parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_matching_covers_common_forms() {
        let list = "localhost, .internal, *.corp.example, 10.0.0.5, <local>";
        assert!(host_matches_no_proxy("localhost", list));
        assert!(host_matches_no_proxy("LOCALHOST", list));
        assert!(host_matches_no_proxy("mongo.internal", list));
        assert!(host_matches_no_proxy("internal", list));
        assert!(host_matches_no_proxy("a.b.corp.example", list));
        assert!(host_matches_no_proxy("10.0.0.5", list));
        assert!(host_matches_no_proxy("myhost", list)); // <local>
        assert!(host_matches_no_proxy("localhost.", "localhost")); // FQDN 尾点无关
        assert!(!host_matches_no_proxy("pidock.example.com", list));
        assert!(host_matches_no_proxy("notlocalhost", list)); // <local>：任意不含点的单标签主机
        assert!(host_matches_no_proxy("anything", "*"));
        assert!(!host_matches_no_proxy("anything", ""));
    }

    #[test]
    fn loopback_is_always_appended() {
        assert_eq!(
            with_loopback(Some("example.com")).as_deref(),
            Some("localhost,127.0.0.1,::1,example.com")
        );
        assert_eq!(
            with_loopback(None).as_deref(),
            Some("localhost,127.0.0.1,::1")
        );
        // 去重：用户已写过的项不重复
        assert_eq!(
            with_loopback(Some("localhost, example.com")).as_deref(),
            Some("localhost,127.0.0.1,::1,example.com")
        );
    }

    #[test]
    fn bypass_covers_loopback_without_config() {
        let wc = WsConnect::direct();
        assert!(wc.should_bypass("localhost"));
        assert!(wc.should_bypass("127.0.0.1"));
        assert!(!wc.should_bypass("sync.example.com"));
        let wc = WsConnect {
            proxy_url: Some("http://10.0.0.1:8080".into()),
            no_proxy: Some("sync.example.com".into()),
            ca_pem_paths: vec![],
        };
        assert!(wc.should_bypass("sync.example.com"));
        assert!(wc.should_bypass("localhost"));
        assert!(!wc.should_bypass("api.example.com"));
    }

    /// 端到端：本地伪造 CONNECT 代理 + 本地 ws 回显服务，
    /// 验证 隧道建立 → WS 握手 → 收发 全链路
    #[tokio::test]
    async fn ws_through_connect_tunnel() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = target.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let (mut sink, mut stream) = ws.split();
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(t) = msg {
                    let _ = sink.send(Message::Text(t)).await;
                }
            }
        });

        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut client, _) = proxy.accept().await.unwrap();
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 512];
            // 读 CONNECT 头
            loop {
                let n = client.read(&mut chunk).await.unwrap();
                assert!(n > 0, "proxy: client closed before CONNECT header end");
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf);
            assert!(head.starts_with("CONNECT "), "unexpected proxy request: {head}");
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .unwrap();
            // 连到真实目标并双向转发
            let hostline = head
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            let mut target = TcpStream::connect(hostline).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut client, &mut target).await;
        });

        let wc = WsConnect {
            proxy_url: Some(format!("http://{proxy_addr}")),
            no_proxy: None,
            ca_pem_paths: vec![],
        };
        let mut ws = wc
            .connect(&format!("ws://{target_addr}/ws?token=abc"))
            .await
            .expect("ws through tunnel");
        ws.send(Message::Text("ping".into())).await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("echo timeout")
            .expect("stream ended")
            .expect("ws error");
        assert_eq!(reply, Message::Text("ping".into()));
    }

    /// 目标在 no_proxy 里时不经过代理（代理拒绝连接也能直连成功）
    #[tokio::test]
    async fn ws_bypasses_proxy_on_no_proxy_match() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = target.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let (mut sink, mut stream) = ws.split();
            if let Some(Ok(Message::Text(t))) = stream.next().await {
                let _ = sink.send(Message::Text(t)).await;
            }
        });

        let wc = WsConnect {
            proxy_url: Some("http://127.0.0.1:1".into()), // 不可能连上的代理
            no_proxy: Some(target_addr.ip().to_string()),
            ca_pem_paths: vec![],
        };
        let mut ws = wc.connect(&format!("ws://{target_addr}/x")).await.expect("direct ws");
        ws.send(Message::Text("hi".into())).await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply, Message::Text("hi".into()));
    }

    /// 代理拒绝 CONNECT 时错误要透出代理的状态行
    #[tokio::test]
    async fn ws_proxy_reject_reports_status() {
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut client, _) = proxy.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = client.read(&mut buf).await;
            let _ = client
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await;
        });
        let wc = WsConnect {
            proxy_url: Some(format!("http://{proxy_addr}")),
            no_proxy: None,
            ca_pem_paths: vec![],
        };
        let err = wc.connect("ws://example.invalid/ws").await.unwrap_err();
        assert!(err.contains("407"), "should surface proxy status: {err}");
    }
}
