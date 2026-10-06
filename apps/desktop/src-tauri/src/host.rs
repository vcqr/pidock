//! pi-host supervisor: spawn the host daemon, bridge stdio JSONL to the
//! frontend (requests via `host_request` command, events via `pidock:event`).

use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use pidock_protocol::{CoreFrame, HostFrame, Request};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::oneshot,
};
use uuid::Uuid;

type PendingMap = Arc<Mutex<HashMap<Uuid, oneshot::Sender<Result<Value, String>>>>>;

struct Running {
    child: Mutex<Child>,
    /// None = 管道已关闭（优雅关闭第一步），后续请求直接失败
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pending: PendingMap,
}

#[derive(Default)]
pub struct Supervisor {
    inner: Mutex<Option<Arc<Running>>>,
}

impl Supervisor {
    fn spawn(&self, app: AppHandle) -> Result<(), String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if let Some(running) = guard.as_ref() {
            if running.child.lock().map_err(|e| e.to_string())?.id().is_some() {
                return Ok(()); // already running
            }
        }

        // release builds spawn the bundled pi-host sidecar that sits next to
        // the main executable; dev builds run `bun src/main.ts` from the repo
        let (default_dir, default_cmd, default_args) = if cfg!(debug_assertions) {
            (
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../host")
                    .to_string_lossy()
                    .into_owned(),
                "bun".to_string(),
                "src/main.ts".to_string(),
            )
        } else {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_default();
            (String::new(), exe_dir.join("pidock-host.exe").to_string_lossy().into_owned(), String::new())
        };
        let host_dir = normalize_path(std::path::Path::new(
            &std::env::var("PIDOCK_HOST_DIR").unwrap_or(default_dir),
        ))
        .to_string_lossy()
        .into_owned();
        let host_cmd = std::env::var("PIDOCK_HOST_CMD").unwrap_or(default_cmd);
        let host_args: Vec<String> = std::env::var("PIDOCK_HOST_ARGS")
            .unwrap_or(default_args)
            .split_whitespace()
            .map(str::to_string)
            .collect();

        // Windows: CreateProcess only finds .exe on PATH, never .cmd shims
        // (npm-global bun ships bun.cmd, not bun.exe). Resolve manually and
        // wrap shim scripts in `cmd /C` so they run with PATHEXT semantics.
        let (program, cmd_prefix) = resolve_command(&host_cmd);
        let mut command = Command::new(&program);
        command.args(&cmd_prefix).args(&host_args);
        // release sidecar mode has no host dir; empty cwd must not be passed
        if !host_dir.is_empty() {
            command.current_dir(&host_dir);
        }
        // 代理设置（设置中心·常规）：以环境变量形式注入 pi-host，
        // Bun 的 fetch 会遵循 HTTP(S)_PROXY/NO_PROXY，证书走 NODE_EXTRA_CA_CERTS
        for key in proxy_clear_keys() {
            command.env_remove(key);
        }
        for (key, value) in proxy_env_overrides() {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to spawn {program} (dir={host_dir}): {e}"))?;

        let stdin = child.stdin.take().ok_or("host stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("host stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("host stderr unavailable")?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // stderr -> app log (host logs and extension console output)
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        eprint!("[pi-host] {}", line.trim_end());
                    }
                }
            }
        });

        // stdout -> frame dispatch: responses resolve pending requests,
        // envelopes are emitted to the webview as `pidock:event` and fanned
        // out to the sync-agent via the managed broadcast channel
        let event_tx = app
            .state::<tokio::sync::broadcast::Sender<pidock_protocol::Envelope>>()
            .inner()
            .clone();
        let pending_for_reader = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<HostFrame>(trimmed) {
                            Ok(HostFrame::Response(resp)) => {
                                let sender = pending_for_reader
                                    .lock()
                                    .ok()
                                    .and_then(|mut map| map.remove(&resp.id));
                                if let Some(sender) = sender {
                                    if resp.ok {
                                        let _ = sender.send(Ok(resp.result.unwrap_or(Value::Null)));
                                    } else {
                                        let err = resp
                                            .error
                                            .map(|e| format!("{}: {}", e.code, e.message))
                                            .unwrap_or_else(|| "unknown error".into());
                                        let _ = sender.send(Err(err));
                                    }
                                }
                            }
                            Ok(HostFrame::Event(envelope)) => {
                                let _ = app.emit("pidock:event", &envelope);
                                let _ = event_tx.send(envelope);
                            }
                            Err(e) => {
                                eprintln!("[pi-host] unparseable frame: {e}: {trimmed:.200}");
                            }
                        }
                    }
                }
            }
            // host died: fail all pending requests
            if let Ok(mut map) = pending_for_reader.lock() {
                for (_, sender) in map.drain() {
                    let _ = sender.send(Err("pi-host exited unexpectedly".into()));
                }
            }
            eprintln!("[pi-host] stdout closed");
        });

        *guard = Some(Arc::new(Running {
            child: Mutex::new(child),
            stdin: Arc::new(tokio::sync::Mutex::new(Some(stdin))),
            pending,
        }));
        eprintln!("[supervisor] pi-host spawned: {host_cmd} {}", host_args.join(" "));
        Ok(())
    }

    pub async fn request(&self, app: AppHandle, method: String, params: Value) -> Result<Value, String> {
        if let Err(e) = self.spawn(app) {
            return Err(e);
        }
        let running = {
            let guard = self.inner.lock().map_err(|e| e.to_string())?;
            guard.as_ref().cloned().ok_or("host not running")?
        };

        let id = Uuid::now_v7();
        let frame = CoreFrame::Request(Request { id, method, params });
        let line = serde_json::to_string(&frame).map_err(|e| e.to_string())? + "\n";

        let (tx, rx) = oneshot::channel();
        running
            .pending
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id, tx);

        {
            let mut slot = running.stdin.lock().await;
            let Some(stdin) = slot.as_mut() else {
                return Err("pi-host is shutting down".into());
            };
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| format!("write to host failed: {e}"))?;
            stdin.flush().await.map_err(|e| e.to_string())?;
        }

        match tokio::time::timeout(Duration::from_secs(180), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("host dropped the request channel".into()),
            Err(_) => {
                running.pending.lock().ok().and_then(|mut m| m.remove(&id));
                Err("host request timed out (180s)".into())
            }
        }
    }

    /// 平滑关闭：① 关闭 host stdin（EOF → host 的 readline 'close' →
    /// disposeAll → 进程自退）② 宽限期内轮询等待 ③ 超时 taskkill /T /F 兜底。
    /// 在 RunEvent::Exit（主线程、非 tokio runtime 上下文）同步调用。
    pub fn shutdown(&self) {
        let running = self.inner.lock().ok().and_then(|mut g| g.take());
        let Some(running) = running else { return };

        // ① 尽力关闭 stdin：写请求正持有锁时放弃优雅路径，直接走兜底
        let closed = match running.stdin.try_lock() {
            Ok(mut slot) => {
                slot.take(); // drop ChildStdin = 管道 EOF
                true
            }
            Err(_) => false,
        };

        // ② 宽限期轮询（host disposeAll + 50ms flush 后自退，通常 <200ms）
        if closed {
            let deadline = std::time::Instant::now() + Duration::from_millis(1200);
            while std::time::Instant::now() < deadline {
                match running.child.lock().ok().and_then(|mut c| c.try_wait().ok()) {
                    Some(Some(_)) => return, // host 已自行退出
                    Some(None) => std::thread::sleep(Duration::from_millis(50)),
                    None => break,
                }
            }
        }

        // ③ 兜底强杀
        Self::force_kill(&running.child);
    }

    /// Force kill the host process tree (Windows: taskkill /T /F, else SIGKILL).
    fn force_kill(child: &Mutex<Child>) {
        #[cfg(target_os = "windows")]
        if let Ok(c) = child.lock() {
            if let Some(pid) = c.id() {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .status();
            }
        }
        #[cfg(not(target_os = "windows"))]
        if let Ok(mut c) = child.lock() {
            let _ = c.start_kill();
        }
    }
}

#[tauri::command]
pub async fn host_request(
    app: AppHandle,
    state: tauri::State<'_, Supervisor>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    state.request(app, method, params).await
}

/// Resolve `cmd` against PATH the way a shell would (PATHEXT included).
/// Returns the program to spawn plus argument prefix (empty, or `/C <shim>`
/// when the command only exists as a `.cmd`/`.bat` script).
#[cfg(target_os = "windows")]
fn resolve_command(cmd: &str) -> (String, Vec<String>) {
    use std::path::PathBuf;

    let looks_like_path = cmd.contains('\\') || cmd.contains('/');
    if looks_like_path || PathBuf::from(cmd).extension().is_some() {
        return (cmd.to_string(), Vec::new()); // explicit path or extension: use as-is
    }

    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(';').filter(|d| !d.is_empty()) {
        let base = PathBuf::from(dir.trim_matches('"')).join(cmd);
        if let Some(exe) = [&base.with_extension("exe")].into_iter().find(|p| p.is_file()) {
            return (exe.to_string_lossy().into_owned(), Vec::new());
        }
        // extensionless files on PATH (sh scripts) are not spawnable from
        // CreateProcess; prefer .cmd/.bat shims via cmd.exe
        let script = base.with_extension("cmd");
        if script.is_file() {
            return (
                "cmd.exe".to_string(),
                vec!["/C".to_string(), script.to_string_lossy().into_owned()],
            );
        }
        let bat = base.with_extension("bat");
        if bat.is_file() {
            return (
                "cmd.exe".to_string(),
                vec!["/C".to_string(), bat.to_string_lossy().into_owned()],
            );
        }
    }
    (cmd.to_string(), Vec::new()) // not found: let spawn produce the error
}

#[cfg(not(target_os = "windows"))]
fn resolve_command(cmd: &str) -> (String, Vec<String>) {
    (cmd.to_string(), Vec::new())
}

/// Lexically normalize a path (resolve `..` and `.` without touching the FS).
fn normalize_path(p: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ------------------------------------------------------------------ proxy ---
// 设置中心「常规 · 代理设置」→ pi-host 环境变量。代理属于进程级配置
//（Bun 在 spawn 时读取），修改后需重启应用生效。

/// 代理模式下需要先清掉的继承变量：mode=direct 必须真正直连
fn proxy_clear_keys() -> &'static [&'static str] {
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

#[derive(Debug, PartialEq)]
struct ProxyConf {
    mode: String,
    url: Option<String>,
    no_proxy: Option<String>,
    ca_path: Option<String>,
}

fn load_proxy_conf() -> Option<ProxyConf> {
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let path = std::path::Path::new(&base)
        .join(".pi")
        .join("agent")
        .join("pidock")
        .join("settings.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let p = v.get("proxy")?;
    Some(ProxyConf {
        mode: p.get("mode")?.as_str()?.to_string(),
        url: p.get("url").and_then(|v| v.as_str()).map(str::to_string),
        no_proxy: p.get("noProxy").and_then(|v| v.as_str()).map(str::to_string),
        ca_path: p.get("caPath").and_then(|v| v.as_str()).map(str::to_string),
    })
}

/// 解析 `reg query` 的值行（`    Name    REG_SZ    value`），取最后一列
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

#[cfg(not(target_os = "windows"))]
fn system_proxy() -> Option<(String, Option<String>)> {
    // 非 Windows 暂不读取桌面环境代理设置
    None
}

/// 由代理配置计算要注入 pi-host 的环境变量
fn proxy_env_overrides() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(conf) = load_proxy_conf() else {
        return out;
    };
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
    if let Some(url) = proxy_url {
        out.push(("HTTP_PROXY".into(), url.clone()));
        out.push(("HTTPS_PROXY".into(), url));
    }
    if let Some(list) = no_proxy {
        if !list.trim().is_empty() {
            out.push(("NO_PROXY".into(), list.clone()));
            out.push(("no_proxy".into(), list));
        }
    }
    if let Some(ca) = conf.ca_path {
        let ca = ca.trim();
        if !ca.is_empty() && std::path::Path::new(ca).is_file() {
            out.push(("NODE_EXTRA_CA_CERTS".into(), ca.to_string()));
        } else {
            eprintln!("[supervisor] proxy.caPath 不存在，忽略：{ca}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_parent_dirs() {
        let p = normalize_path(std::path::Path::new("D:/a/b/c/../../host"));
        assert_eq!(p, std::path::PathBuf::from("D:/a/host"));
    }

    #[test]
    fn reg_value_parses_last_column() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x1\r\n\r\n";
        assert_eq!(parse_reg_value(out, "ProxyEnable").as_deref(), Some("0x1"));
        assert_eq!(parse_reg_value(out, "ProxyServer"), None);
    }

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
            merge_no_proxy(Some("localhost, 127.0.0.1"), Some("*.local;localhost;<local>")).as_deref(),
            Some("localhost,127.0.0.1,*.local,<local>")
        );
        assert_eq!(merge_no_proxy(None, None), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_finds_cmd_shim() {
        // on this machine bun is an npm shim (bun.cmd), not an exe
        let (program, prefix) = resolve_command("bun");
        if program.eq_ignore_ascii_case("cmd.exe") {
            assert_eq!(prefix.len(), 2);
            assert!(prefix[1].to_lowercase().ends_with("bun.cmd"));
        } else {
            assert!(program.to_lowercase().ends_with(".exe"), "unexpected: {program}");
        }
    }
}
