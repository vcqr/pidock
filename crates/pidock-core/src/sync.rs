//! Desktop sync-agent: WebSocket uplink to the PiDock server.
//!
//! - upstream: host events (filtered: persist + live-state kinds) and
//!   command_result envelopes
//! - downstream: user commands -> host requests -> result back as
//!   command_result
//! - lifecycle: login (HTTP) -> WS connect with JWT -> register machine ->
//!   heartbeat 30s -> backfill open sessions on (re)connect -> reconnect with
//!   backoff

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::scheduler::SchedulerManager;
use crate::supervisor::Supervisor;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyncConfig {
    pub server_url: String,
    pub email: String,
    pub refresh_token: String,
    /// 个人访问令牌（pd_ 前缀）：设置后跳过 refresh 轮换，直接作为 access 使用
    #[serde(default)]
    pub pat: String,
    pub machine_id: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SyncStatus {
    pub enabled: bool,
    pub connected: bool,
    pub machine_id: String,
    pub error: Option<String>,
}

pub enum SyncControl {
    Restart,
    Stop,
}

pub struct SyncManager {
    pub cfg_path: std::path::PathBuf,
    pub cfg: Mutex<SyncConfig>,
    pub status: Arc<Mutex<SyncStatus>>,
    pub control: mpsc::Sender<SyncControl>,
}

impl SyncManager {
    pub fn new(cfg_path: std::path::PathBuf, control: mpsc::Sender<SyncControl>) -> Self {
        let cfg = std::fs::read(&cfg_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SyncConfig>(&bytes).ok())
            .unwrap_or_default();
        Self {
            cfg_path,
            cfg: Mutex::new(cfg),
            status: Arc::new(Mutex::new(SyncStatus::default())),
            control,
        }
    }

    pub fn save(&self, cfg: &SyncConfig) {
        if let Some(parent) = self.cfg_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(cfg) {
            let _ = std::fs::write(&self.cfg_path, bytes);
        }
    }

    pub async fn set_status(&self, patch: SyncStatus) {
        let mut st = self.status.lock().await;
        if patch.connected != st.connected {
            st.connected = patch.connected;
        }
        st.machine_id = patch.machine_id;
        st.error = patch.error;
        st.enabled = patch.enabled;
    }

    /// 配置并开启同步（设置中心「云同步」页 / webhost 同名命令的共用实现）
    pub async fn configure(self: &Arc<Self>, body: SyncConfigureBody) -> Result<Value, String> {
        let server_url = body.server_url.trim_end_matches('/').to_string();
        let mut cfg = self.cfg.lock().await.clone();
        cfg.server_url = server_url.clone();
        cfg.email = body.email.clone();
        cfg.enabled = true;

        let token = body.token.trim().to_string();
        if !token.is_empty() {
            if !token.starts_with("pd_") {
                return Err("访问令牌应以 pd_ 开头，请在 Web 端「访问令牌」页重新复制".into());
            }
            // 先对 /me 验一次再落盘，坏令牌当场报错；顺带把 email 修正为令牌属主
            let me = http_json("GET", &format!("{server_url}/me?token={token}"), &json!({}))
                .await
                .map_err(|e| format!("访问令牌验证失败：{e}"))?;
            if let Some(owner) = me.get("email").and_then(|v| v.as_str()) {
                cfg.email = owner.to_string();
            }
            cfg.pat = token;
            cfg.refresh_token.clear();
        } else if !body.password.is_empty() {
            let data = http_json(
                "POST",
                &format!("{server_url}/auth/login"),
                &json!({"email": body.email, "password": body.password}),
            )
            .await?;
            cfg.refresh_token = data
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .ok_or("login response missing refresh_token")?
                .to_string();
            cfg.pat.clear();
        }
        if cfg.refresh_token.is_empty() && cfg.pat.is_empty() {
            return Err("没有可用的登录凭据，请输入密码或访问令牌".into());
        }

        self.save(&cfg);
        *self.cfg.lock().await = cfg;
        let _ = self.control.send(SyncControl::Restart).await;
        Ok(json!({"ok": true}))
    }

    pub async fn disable(&self) -> Result<Value, String> {
        let mut cfg = self.cfg.lock().await.clone();
        cfg.enabled = false;
        self.save(&cfg);
        *self.cfg.lock().await = cfg;
        let _ = self.control.send(SyncControl::Stop).await;
        Ok(json!({"ok": true}))
    }

    pub async fn status(&self) -> Result<Value, String> {
        let cfg = self.cfg.lock().await.clone();
        let st = self.status.lock().await.clone();
        Ok(json!({
            "enabled": st.enabled,
            "connected": st.connected,
            "machine_id": st.machine_id,
            "email": cfg.email,
            "server_url": cfg.server_url,
            "error": st.error,
        }))
    }
}

#[derive(Deserialize)]
pub struct SyncConfigureBody {
    pub server_url: String,
    pub email: String,
    #[serde(default)]
    pub password: String,
    /// 访问令牌（Web 端「访问令牌」页生成）；填了就优先于密码登录
    #[serde(default)]
    pub token: String,
}

async fn http_json(method: &str, url: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let req = match method {
        "POST" => client.post(url),
        _ => client.get(url),
    };
    let resp = req
        .json(body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("{method} {url} failed: {e}"))?;
    let status = resp.status();
    let data: Value = resp
        .json()
        .await
        .map_err(|e| format!("bad json from {url}: {e}"))?;
    if !status.is_success() {
        let msg = data
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("request failed");
        return Err(format!("{status}: {msg}"));
    }
    Ok(data)
}

/// refresh the access token; returns (access, new_refresh) — the server
/// ROTATES refresh tokens on every call, so the new one must be persisted.
/// PAT 模式下令牌即长期 access，直接返回、不发请求。
async fn refresh_access(cfg: &SyncConfig) -> Result<(String, String), String> {
    if !cfg.pat.is_empty() {
        return Ok((cfg.pat.clone(), cfg.refresh_token.clone()));
    }
    let data = http_json(
        "POST",
        &format!("{}/auth/refresh", cfg.server_url.trim_end_matches('/')),
        &json!({"refresh_token": cfg.refresh_token}),
    )
    .await?;
    let access = data
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("no access_token in refresh response")?
        .to_string();
    let new_refresh = data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or("no refresh_token in refresh response")?
        .to_string();
    Ok((access, new_refresh))
}

/// kinds that get mirrored to the cloud (persist events are always mirrored)
fn cloud_worthy(kind: &str) -> bool {
    matches!(
        kind,
        "message_snapshot"
            | "session_meta"
            | "agent_state_changed"
            | "tool_execution_start"
            | "tool_execution_end"
            | "tool_approval"
            | "ask_user_question"
            | "auto_retry"
            | "compaction_lifecycle"
            | "todo_updated"
            | "session_removed"
            | "session_settings_changed"
            | "command_result"
            | "context_usage"
            | "queue_changed"
            | "error"
    )
}

/// collect content-addressed attachment refs from an arbitrary payload
fn collect_attachments(value: &Value, out: &mut Vec<(String, i64)>) {
    match value {
        Value::Object(map) => {
            let sha = map.get("attachment_id").and_then(|v| v.as_str());
            let truncated = map
                .get("truncated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let (Some(sha), Some(_)) = (sha, map.get("size")) {
                if truncated && sha.len() == 64 {
                    out.push((
                        sha.to_string(),
                        map.get("size").and_then(|v| v.as_i64()).unwrap_or(0),
                    ));
                    return;
                }
            }
            for (_, v) in map {
                collect_attachments(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_attachments(v, out);
            }
        }
        _ => {}
    }
}

async fn upload_attachments(
    supervisor: &Supervisor,
    access: &str,
    server_url: &str,
    envelope: &Value,
    uploaded: &mut HashSet<String>,
) {
    let mut refs = Vec::new();
    collect_attachments(envelope, &mut refs);
    if refs.is_empty() {
        return;
    }
    let agent_dir = match supervisor.request("config.get".into(), json!({})).await {
        Ok(v) => v
            .get("agent_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => return,
    };
    let client = reqwest::Client::new();
    for (sha, size) in refs {
        if uploaded.contains(sha.as_str()) {
            continue;
        }
        let path = PathBuf::from(&agent_dir)
            .join("pidock")
            .join("attachments")
            .join(&sha);
        let presign = json!({"token": access, "sha256": sha, "size": size});
        let resp = client
            .post(format!("{server_url}/attachments/presign"))
            .json(&presign)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await;
        let data = match resp {
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("attachment presign parse failed: {e}");
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("attachment presign failed: {e}");
                continue;
            }
        };
        let Some(upload_url) = data.get("upload_url").and_then(|v| v.as_str()) else {
            continue;
        };
        if data.get("exists").and_then(|v| v.as_bool()) != Some(true) {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if let Err(e) = client.put(upload_url).body(bytes).send().await {
                        tracing::warn!("attachment upload failed: {e}");
                        continue;
                    }
                }
                Err(e) => {
                    tracing::warn!("attachment local file missing ({}): {e}", path.display());
                    continue;
                }
            }
        }
        let sha_short: String = sha.chars().take(12).collect();
        uploaded.insert(sha);
        tracing::info!("attachment {sha_short}… uploaded");
    }
}

fn command_result_envelope(
    session_id: &str,
    command_id: &str,
    result: &Result<Option<Value>, String>,
) -> Value {
    let payload = match result {
        Ok(r) => {
            json!({"command_id": command_id, "ok": true, "result": r.clone().unwrap_or(json!({}))})
        }
        Err(e) => json!({"command_id": command_id, "ok": false, "error": e}),
    };
    json!({
        "event_id": Uuid::now_v7().to_string(),
        "session_id": session_id,
        "persist": false,
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": "command_result",
        "payload": payload,
    })
}

/// translate a server command into a host request
async fn execute_command(
    supervisor: &Supervisor,
    scheduler: &Arc<SchedulerManager>,
    session_id: &str,
    cmd_type: &str,
    payload: &Value,
) -> Result<Option<Value>, String> {
    let (method, params) = match cmd_type {
        // 图片/文档附件原样透传（web 端共享同一 UI，发送载荷含 images/attachments）
        "agent.prompt" => {
            let mut p = json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")});
            for key in ["images", "attachments"] {
                if let Some(v) = payload.get(key) {
                    if v.is_array() && !v.as_array().unwrap().is_empty() {
                        p[key] = v.clone();
                    }
                }
            }
            ("agent.prompt", p)
        }
        "agent.steer" => (
            "agent.steer",
            json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")}),
        ),
        "agent.follow_up" => (
            "agent.follow_up",
            json!({"session_id": session_id, "text": payload.get("text").and_then(|t| t.as_str()).unwrap_or("")}),
        ),
        "agent.abort" => ("agent.abort", json!({"session_id": session_id})),
        "session.create" => {
            let mut p = json!({"cwd": payload.get("cwd").and_then(|t| t.as_str()).unwrap_or(""), "model": payload.get("model")});
            // 雇佣专家建会话（web 端雇佣 chip 与桌面共享 UI）
            if let Some(v) = payload.get("expert_id") {
                if v.is_string() {
                    p["expert_id"] = v.clone();
                }
            }
            ("session.create", p)
        }
        "session.resolve_approval" => (
            "session.resolve_approval",
            json!({
                "approval_id": payload.get("approval_id").and_then(|v| v.as_str()).unwrap_or(""),
                "approved": payload.get("approved").and_then(|v| v.as_bool()).unwrap_or(false),
            }),
        ),
        "session.resolve_ask" => ("session.resolve_ask", {
            let mut p = json!({
                "ask_id": payload.get("ask_id").and_then(|v| v.as_str()).unwrap_or(""),
                "interrupted": payload.get("interrupted").and_then(|v| v.as_bool()).unwrap_or(false),
            });
            // labels/text 可选：取消回答时两者皆缺
            if let Some(v) = payload.get("labels") {
                if v.is_array() {
                    p["labels"] = v.clone();
                }
            }
            if let Some(v) = payload.get("text") {
                if v.is_string() {
                    p["text"] = v.clone();
                }
            }
            p
        }),
        "session.rename" => (
            "session.rename",
            json!({"session_id": session_id, "name": payload.get("name").and_then(|t| t.as_str()).unwrap_or("")}),
        ),
        // 移除项目/删除会话：session_ids 数组原样透传
        "session.remove" => (
            "session.remove",
            json!({"session_ids": payload.get("session_ids").cloned().unwrap_or(json!([]))}),
        ),
        // 文件浏览（@ 提及与文件面板共用）
        "workspace.files" => (
            "workspace.files",
            json!({"cwd": payload.get("cwd").and_then(|t| t.as_str()).unwrap_or("")}),
        ),
        "workspace.read_file" => (
            "workspace.read_file",
            json!({
                "cwd": payload.get("cwd").and_then(|t| t.as_str()).unwrap_or(""),
                "path": payload.get("path").and_then(|t| t.as_str()).unwrap_or(""),
            }),
        ),
        // 会话级新命令（上下文用量/压缩/队列/导出/开关/工具/树/prompts/thinking/信任/
        // 分叉/权限模式/思考级别/模型/文件变更审查）：与 host TS Method 同名，
        // 注入 session_id 后原样透传
        passthrough @ ("agent.context_usage"
        | "agent.compact"
        | "agent.clear_queue"
        | "agent.abort_retry"
        | "session.export"
        | "session.set_auto_compaction"
        | "session.set_auto_retry"
        | "session.list_tools"
        | "session.set_active_tools"
        | "session.tree"
        | "session.navigate_tree"
        | "config.prompts.list"
        | "config.reload_runtime"
        | "session.thinking_info"
        | "session.trust"
        | "session.fork"
        | "session.set_permission_mode"
        | "session.set_thinking_level"
        | "session.set_model"
        | "session.file_changes"
        | "session.file_diff"
        | "session.revert_files"
        | "session.pending") => {
            let mut p = payload.clone();
            p["session_id"] = json!(session_id);
            (passthrough, p)
        }
        // 非会话级命令（配置中心/供应商/模型/技能/扩展/MCP/专家/用量/代理设置/列表查询）：
        // 无 session_id，载荷原样透传。web 与桌面共享同一控制面 —— web 能发
        // agent.prompt 即可在桌面执行代码，config 写不构成新增权限面
        passthrough @ ("config.get"
        | "config.settings.set"
        | "config.providers.list"
        | "config.providers.set_key"
        | "config.providers.remove_key"
        | "config.providers.custom.get"
        | "config.providers.custom.set"
        | "config.providers.custom.remove"
        | "config.providers.fetch_models"
        | "config.models.list"
        | "config.models.set_default"
        | "config.model_override.set"
        | "config.extensions.list"
        | "config.extensions.toggle"
        | "config.extensions.read"
        | "config.extensions.install"
        | "config.skills.list"
        | "config.skills.toggle"
        | "config.skills.files"
        | "config.skills.read"
        | "config.skills.install"
        | "config.mcp.get"
        | "config.mcp.set"
        | "config.agents.read"
        | "config.agents.write"
        | "stats.usage"
        | "pidock.settings.get"
        | "pidock.settings.set"
        | "experts.list"
        | "experts.get"
        | "experts.save"
        | "experts.delete"
        | "experts.private_list"
        | "experts.read_avatar_file"
        | "experts.install_resource"
        | "experts.remove_resource") => (passthrough, payload.clone()),
        // 本地定时任务管理：调度器在核心 Rust 侧，不经 host —— 直连 SchedulerManager
        automation @ ("automation.list"
        | "automation.save"
        | "automation.delete"
        | "automation.set_enabled"
        | "automation.run_now"
        | "automation.peek") => {
            let result =
                crate::scheduler::dispatch_automation(scheduler, automation, payload.clone())
                    .await?;
            return Ok(Some(result));
        }
        // 本机命令：目录列举 / git 分支信息与切换。同样不经 host —— web console 的
        // 「打开文件夹」弹层与分支 chip 中继到目标机器，在核心 Rust 侧执行
        // （与桌面 ipc 的 fs_list / git_* 是同一份实现）
        local @ ("fs_list" | "git_info" | "git_branches" | "git_checkout") => {
            let result = match local {
                "fs_list" => crate::fs::list(payload.clone()).await,
                "git_info" => crate::git::info(payload.clone()).await,
                "git_branches" => crate::git::branches(payload.clone()).await,
                _ => crate::git::checkout(payload.clone()).await,
            };
            return result.map(Some);
        }
        other => {
            return Err(format!(
                "command type \"{other}\" not supported by this desktop"
            ));
        }
    };
    supervisor
        .request(method.to_string(), params)
        .await
        .map(Some)
}

/// run the sync loop until disabled or control message
async fn sync_loop(
    supervisor: Arc<Supervisor>,
    scheduler: Arc<SchedulerManager>,
    sync: Arc<SyncManager>,
    mut event_rx: broadcast::Receiver<pidock_protocol::Envelope>,
    mut control_rx: mpsc::Receiver<SyncControl>,
) {
    let mut backoff_secs = 1u64;
    let mut uploaded: HashSet<String> = HashSet::new();
    // 附件去重缓存按账号区分：检测到换号（服务器/邮箱/令牌变化）即作废，
    // 否则旧账号传过的附件会被跳过，新账号云端缺文件
    let mut uploaded_for = String::new();

    'outer: loop {
        let (mut cfg, enabled) = {
            let c = sync.cfg.lock().await;
            (
                c.clone(),
                c.enabled && (!c.refresh_token.is_empty() || !c.pat.is_empty()),
            )
        };
        let cred_id = format!("{}|{}|{}", cfg.server_url, cfg.email, cfg.pat);
        if cred_id != uploaded_for {
            uploaded.clear();
            uploaded_for = cred_id;
        }
        if !enabled {
            sync.set_status(SyncStatus {
                enabled: false,
                connected: false,
                machine_id: cfg.machine_id.clone(),
                error: None,
            })
            .await;
            // wait for re-enable
            match control_rx.recv().await {
                Some(SyncControl::Restart) => continue,
                _ => continue,
            }
        }

        // 1) access token — persist the rotated refresh token immediately,
        // otherwise the next reconnect fails with "revoked or expired".
        // 换号竞态防护：刷新期间 configure 可能刚写入新账号凭据，此时旧账号
        // 轮换出的 refresh_token 绝不能落盘覆盖新凭据——只在存储里的
        // refresh_token 仍是本次刷新所用的那条时才写回，否则按新配置重来
        let old_refresh = cfg.refresh_token.clone();
        let (access, new_refresh) = match refresh_access(&cfg).await {
            Ok(pair) => pair,
            Err(e) => {
                if sync.cfg.lock().await.refresh_token.clone() != old_refresh {
                    continue 'outer;
                }
                let user_msg =
                    if e.contains("revoked") || e.contains("expired") || e.contains("Unauthorized")
                    {
                        // the stored token is dead: drop it so the next configure
                        // requires a fresh password login
                        let mut c = sync.cfg.lock().await;
                        c.refresh_token.clear();
                        sync.save(&c);
                        cfg.refresh_token.clear();
                        "登录已过期，请重新输入密码后点击「连接并同步」".to_string()
                    } else {
                        e.clone()
                    };
                sync.set_status(SyncStatus {
                    enabled: true,
                    connected: false,
                    machine_id: cfg.machine_id.clone(),
                    error: Some(user_msg.clone()),
                })
                .await;
                tracing::warn!("sync: refresh failed: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
                backoff_secs *= 2;
                continue;
            }
        };
        {
            let mut c = sync.cfg.lock().await;
            if c.refresh_token != old_refresh {
                continue 'outer;
            }
            c.refresh_token = new_refresh;
            sync.save(&c);
            cfg = c.clone();
        }

        // 2) ws connect
        let ws_url = format!(
            "{}/ws/desktop?token={}",
            cfg.server_url
                .trim_end_matches('/')
                .replacen("http", "ws", 1),
            access
        );
        let (ws, _) = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok(pair) => pair,
            Err(e) => {
                // PAT 模式下 401/403 基本等于令牌被吊销，给出可行动的提示
                let msg = format!("connect: {e}");
                let user_msg =
                    if !cfg.pat.is_empty() && (msg.contains("401") || msg.contains("403")) {
                        "访问令牌已失效，请在 Web 端「访问令牌」页重新生成".to_string()
                    } else {
                        msg
                    };
                sync.set_status(SyncStatus {
                    enabled: true,
                    connected: false,
                    machine_id: cfg.machine_id.clone(),
                    error: Some(user_msg),
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
                backoff_secs *= 2;
                continue;
            }
        };
        backoff_secs = 1;
        let (mut ws_sink, mut ws_stream) = ws.split();

        // 3) register machine（home_dir 供云端镜像做「项目/任务」拆分，与 host homedir 同源）
        let hostname = hostname();
        let home_dir = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let register = json!({
            "ctrl": "register",
            "machine_id": cfg.machine_id,
            "hostname": hostname,
            "os": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION"),
            "home_dir": home_dir,
            "local_ip": local_ip(),
        });
        if ws_sink
            .send(WsMessage::Text(register.to_string().into()))
            .await
            .is_err()
        {
            continue;
        }
        sync.set_status(SyncStatus {
            enabled: true,
            connected: true,
            machine_id: cfg.machine_id.clone(),
            error: None,
        })
        .await;
        tracing::info!("sync: connected to {}", cfg.server_url);

        // 4) backfill open sessions (server dedupes via (session_id, seq) upsert)
        {
            if let Ok(list) = supervisor.request("session.list".into(), json!({})).await {
                if let Some(sessions) = list.get("sessions").and_then(|v| v.as_array()) {
                    for s in sessions {
                        // backfill ALL known sessions so the web list is complete,
                        // not just the ones currently open on this desktop
                        let sid = s
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if sid.is_empty() {
                            continue;
                        }
                        // 镜像不存 cwd：补发一条 session_meta（ephemeral，仅 upsert 会话行），
                        // web 端项目分组与文件浏览才有真实工作目录
                        if let Some(cwd) = s.get("cwd").and_then(|v| v.as_str()) {
                            let mut meta_payload = json!({ "cwd": cwd });
                            if let Some(m) = s.get("model").and_then(|v| v.as_str()) {
                                meta_payload["model"] = json!(m);
                            }
                            let meta = json!({
                                "event_id": Uuid::now_v7().to_string(),
                                "session_id": sid,
                                "persist": false,
                                "ts": chrono::Utc::now().to_rfc3339(),
                                "kind": "session_meta",
                                "payload": meta_payload,
                            });
                            let _ = ws_sink.send(WsMessage::Text(meta.to_string().into())).await;
                        }
                        if let Ok(replay) = supervisor
                            .request("session.events".into(), json!({"session_id": sid}))
                            .await
                        {
                            if let Some(events) = replay.get("events").and_then(|v| v.as_array()) {
                                for ev in events {
                                    let env = json!({
                                        "event_id": Uuid::now_v7().to_string(),
                                        "session_id": sid,
                                        "seq": ev.get("seq").cloned().unwrap_or(json!(0)),
                                        "persist": true,
                                        "ts": ev.get("ts").cloned().unwrap_or(json!("")),
                                        "kind": ev.get("kind").cloned().unwrap_or(json!("")),
                                        "payload": ev.get("payload").cloned().unwrap_or(json!({})),
                                    });
                                    upload_attachments(
                                        &supervisor,
                                        &access,
                                        &cfg.server_url,
                                        &env,
                                        &mut uploaded,
                                    )
                                    .await;
                                    let _ =
                                        ws_sink.send(WsMessage::Text(env.to_string().into())).await;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 5) pump: host events + heartbeats + downstream commands
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut closed = false;
        // 命令回包队列：命令在独立任务并发执行（SplitSink 不可 clone），
        // 回包经此通道由主循环统一发送
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if ws_sink.send(WsMessage::Text(json!({"ctrl":"heartbeat"}).to_string().into())).await.is_err() {
                        closed = true;
                    }
                }
                reply = cmd_rx.recv() => {
                    if let Some(frame) = reply {
                        if ws_sink.send(WsMessage::Text(frame.into())).await.is_err() {
                            closed = true;
                        }
                    }
                }
                ev = event_rx.recv() => {
                    match ev {
                        Ok(envelope) => {
                            let persist = envelope.persist;
                            if persist || cloud_worthy(&envelope.kind) {
                                let serialized = serde_json::to_value(&envelope).unwrap_or(json!({}));
                                upload_attachments(&supervisor, &access, &cfg.server_url, &serialized, &mut uploaded).await;
                                if ws_sink.send(WsMessage::Text(serialized.to_string().into())).await.is_err() {
                                    closed = true;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("sync: lagged, skipped {n} host events");
                        }
                        Err(broadcast::error::RecvError::Closed) => closed = true,
                    }
                }
                frame = ws_stream.next() => {
                    match frame {
                        Some(Ok(WsMessage::Text(text))) => {
                            if let Ok(frame) = serde_json::from_str::<Value>(&text) {
                                if frame.get("ctrl").and_then(|v| v.as_str()) == Some("command") {
                                    let command_id = frame.get("command_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let command = frame.get("command").cloned().unwrap_or(json!({}));
                                    let session_id = command.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let cmd_type = command.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    // 命令并发执行：host 端 dispatch 本就是异步并发派发，
                                    // 这里若串行 await，单条慢命令会堵死命令通道与心跳，
                                    // web 端表现为成片「命令超时」（file_changes 等快照拉取也全部失败）
                                    let supervisor = supervisor.clone();
                                    let scheduler = scheduler.clone();
                                    let cmd_tx = cmd_tx.clone();
                                    tokio::spawn(async move {
                                        let result = execute_command(&supervisor, &scheduler, &session_id, &cmd_type, command.get("payload").unwrap_or(&json!({}))).await;
                                        let reply = command_result_envelope(&session_id, &command_id, &result);
                                        let _ = cmd_tx.send(reply.to_string());
                                    });
                                } else if frame.get("ctrl").and_then(|v| v.as_str()) == Some("registered") {
                                    if let Some(mid) = frame.get("machine_id").and_then(|v| v.as_str()) {
                                        let mut c = sync.cfg.lock().await;
                                        if c.machine_id != mid {
                                            c.machine_id = mid.to_string();
                                            sync.save(&c.clone());
                                            drop(c);
                                            sync.set_status(SyncStatus{enabled: true, connected: true, machine_id: mid.to_string(), error: None}).await;
                                        }
                                    }
                                }
                            }
                        }
                        Some(Ok(_)) => {}
                        _ => { closed = true; }
                    }
                }
                ctrl = control_rx.recv() => {
                    match ctrl {
                        Some(SyncControl::Stop) => {
                            let _ = ws_sink.send(WsMessage::Close(None)).await;
                            sync.set_status(SyncStatus{enabled: false, connected: false, machine_id: cfg.machine_id.clone(), error: None}).await;
                            continue 'outer; // back to the idle wait; receiver stays alive
                        }
                        Some(SyncControl::Restart) => {
                            let _ = ws_sink.send(WsMessage::Close(None)).await;
                            continue 'outer;
                        }
                        None => break 'outer,
                    }
                }
            }
            if closed {
                sync.set_status(SyncStatus {
                    enabled: true,
                    connected: false,
                    machine_id: cfg.machine_id.clone(),
                    error: Some("connection lost".into()),
                })
                .await;
                break; // reconnect via backoff
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs.min(30))).await;
        backoff_secs = (backoff_secs * 2).min(30);
    }
}

fn hostname() -> String {
    // 环境变量只有交互式 shell 才有（macOS GUI 进程拿不到 HOSTNAME），
    // 主路径走 gethostname 系统调用；去掉可能的 FQDN 尾点
    hostname::get()
        .ok()
        .map(|h| h.to_string_lossy().trim_end_matches('.').to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".into())
}

/// 探测本机局域网 IP：对公共地址做 UDP connect（不发包），取默认路由出口网卡的源地址；
/// 探测失败（完全离线等）回退 127.0.0.1
fn local_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

/// spawn the sync loop; lives until the process exits (called by CoreCtx::start)
pub async fn run_sync_loop(
    supervisor: Arc<Supervisor>,
    scheduler: Arc<SchedulerManager>,
    sync: Arc<SyncManager>,
    event_rx: broadcast::Receiver<pidock_protocol::Envelope>,
    control_rx: mpsc::Receiver<SyncControl>,
) {
    sync_loop(supervisor, scheduler, sync, event_rx, control_rx).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gethostname 系统调用在任何正常主机/容器里都拿得到名字；
    /// 老实现只查环境变量，macOS GUI 进程下恒为 "unknown"（回归护栏）
    #[test]
    fn hostname_resolves_without_env() {
        let h = hostname();
        assert!(!h.is_empty(), "hostname 不应为空");
        assert_ne!(h, "unknown", "应通过 gethostname 取到真实主机名");
        assert!(!h.contains(' '), "主机名不应含空格: {h}");
    }
}
