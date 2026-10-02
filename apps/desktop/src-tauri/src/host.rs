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
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
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
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
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
            let mut stdin = running.stdin.lock().await;
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

    /// Kill the host process tree (Windows: taskkill /T /F, else SIGKILL).
    pub fn kill(&self) {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(running) = guard.as_ref() {
            #[cfg(target_os = "windows")]
            if let Ok(child) = running.child.lock() {
                if let Some(pid) = child.id() {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                        .status();
                }
            }
            #[cfg(not(target_os = "windows"))]
            if let Ok(mut child) = running.child.lock() {
                let _ = child.start_kill();
            }
        }
        if let Ok(mut g) = self.inner.lock() {
            *g = None;
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
            assert!(program.to_lowercase().ends_with(".exe"), "unexpected: {program}");
        }
    }
}
