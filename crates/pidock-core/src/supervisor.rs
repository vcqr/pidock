//! pi-host supervisor：拉起 host 守护进程，stdio JSONL 桥接到壳层
//! （请求经 [`Supervisor::request`] 转发，事件广播到 [`Supervisor::events`]，
//! 由壳层分发给 webview / 浏览器 WS 客户端 / sync-agent）。

use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use pidock_protocol::{CoreFrame, HostFrame, Request};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, oneshot},
};
use uuid::Uuid;

use crate::proxy::{proxy_clear_keys, proxy_env_overrides};

type PendingMap = Arc<Mutex<HashMap<Uuid, oneshot::Sender<Result<Value, String>>>>>;

struct Running {
    child: Mutex<Child>,
    /// None = 管道已关闭（优雅关闭第一步），后续请求直接失败
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pending: PendingMap,
}

pub struct Supervisor {
    /// host 事件广播：sync-agent、调度器 watcher、壳层事件泵都订阅这里
    pub events: broadcast::Sender<pidock_protocol::Envelope>,
    inner: Mutex<Option<Arc<Running>>>,
}

impl Supervisor {
    pub fn new(events: broadcast::Sender<pidock_protocol::Envelope>) -> Self {
        Self {
            events,
            inner: Mutex::new(None),
        }
    }

    fn spawn(&self) -> Result<(), String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if let Some(running) = guard.as_ref() {
            // try_wait 会回收僵尸进程：已退出则清掉旧条目，走下面的重新拉起
            let alive = running
                .child
                .lock()
                .map_err(|e| e.to_string())?
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(true);
            if alive {
                return Ok(()); // already running
            }
            eprintln!("[supervisor] pi-host exited, respawning on next request");
            *guard = None;
        }

        // 壳层 release 构建拉起与主程序同目录的 pi-host sidecar；
        // debug 构建直接跑仓库里的 `bun src/main.ts`
        let (default_dir, default_cmd, default_args) = if cfg!(debug_assertions) {
            (
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../host")
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
            // 桌面包 tauri externalBin 落盘名与 webhost 镜像内的 sidecar 名一致：
            // unix 是 "pidock-host"，Windows 才带 .exe
            let sidecar = if cfg!(windows) {
                "pidock-host.exe"
            } else {
                "pidock-host"
            };
            (
                String::new(),
                exe_dir.join(sidecar).to_string_lossy().into_owned(),
                String::new(),
            )
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
        // 主程序是 GUI 子系统（无控制台），spawn 控制台子进程会被 Windows
        // 分配新终端窗口；CREATE_NO_WINDOW 使其无窗运行（管道 IO 不受影响）
        #[cfg(target_os = "windows")]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
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
        // 设置中心「常规 · 环境变量」：在代理派生变量之后写入，
        // 同名的用户显式配置覆盖代理派生值
        for (key, value) in crate::envs::env_overrides() {
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
        // envelopes broadcast to shell event pumps and the sync-agent
        let events = self.events.clone();
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
                                let _ = events.send(envelope);
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
        eprintln!(
            "[supervisor] pi-host spawned: {host_cmd} {}",
            host_args.join(" ")
        );
        Ok(())
    }

    pub async fn request(&self, method: String, params: Value) -> Result<Value, String> {
        if let Err(e) = self.spawn() {
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
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                // host 已死（管道破裂）：清掉条目，下一次请求自动重新拉起
                if let Ok(mut guard) = self.inner.lock() {
                    guard.take();
                }
                return Err(format!("write to host failed: {e}"));
            }
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
                match running
                    .child
                    .lock()
                    .ok()
                    .and_then(|mut c| c.try_wait().ok())
                {
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
        if let Some(exe) = [&base.with_extension("exe")]
            .into_iter()
            .find(|p| p.is_file())
        {
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
// 设置中心「常规 · 代理设置」的解析与消费方适配全部在 [`crate::proxy`]：
// 这里只在 spawn pi-host 时取环境变量注入（Bun fetch 遵循 HTTP(S)_PROXY，
// 企业 CA 走 NODE_EXTRA_CA_CERTS；standalone 二进制的 CA 补充由 host 侧
// Bun.loadExtraCACerts 负责，见 host/src/main.ts）。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_parent_dirs() {
        let p = normalize_path(std::path::Path::new("D:/a/b/c/../../host"));
        assert_eq!(p, std::path::PathBuf::from("D:/a/host"));
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
            assert!(
                program.to_lowercase().ends_with(".exe"),
                "unexpected: {program}"
            );
        }
    }
}
