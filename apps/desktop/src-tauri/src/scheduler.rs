//! Scheduled-task automation (自动化 / 定时任务).
//!
//! Rust-side cron scheduler: each enabled job fires on a local-timezone cron
//! schedule and runs its prompt in a FRESH pi session created through the
//! Supervisor bridge (session.create -> set_permission_mode -> rename ->
//! agent.prompt). Run completion is tracked via the host event broadcast
//! channel (agent idle = success, error envelope = failure) with a hard
//! timeout guard; sessions surface in the sidebar like any other session.
//!
//! Storage: %APPDATA%/pidock/jobs.json (atomic tmp+rename).
//! Wire: the webview routes `automation.*` DataBus methods to the
//! `automation_request` command; run lifecycle is pushed as `pidock:event`
//! envelopes with kinds `automation.run_started` / `automation.run_finished`
//! (persist=false, so the cloud sync whitelist ignores them).

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Local, Utc};
use croner::parser::{CronParser, Seconds};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{broadcast, Mutex};
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use crate::host::Supervisor;
use pidock_protocol::{ephemeral, Envelope};

/// 单次运行的最长时长：超时未收到会话空闲/错误事件则记为失败
const RUN_TIMEOUT_SECS: u64 = 30 * 60;
/// 每个任务保留的运行记录条数
const KEEP_RUNS: usize = 20;
/// 与 Composer / store 一致的权限模式集合
const PERMISSION_MODES: [&str; 4] = ["plan", "confirm", "edit-auto", "full"];

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn default_permission_mode() -> String {
    "full".into()
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunRecord {
    pub run_id: String,
    /// cron | manual
    pub trigger: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// running | ok | failed | skipped
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JobImage {
    /// base64，不带 data: 前缀
    pub data: String,
    pub mime_type: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// 用户本地时间的 cron 表达式（含秒域，如 "0 30 9 * * *"）
    pub cron: String,
    /// 工作空间目录；空 = 主目录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// 模型；空 = 默认模型
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 思考级别（off/minimal/low/medium/high）；空 = 默认
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// 随提示词发送的图片附件（≤6 张）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<JobImage>>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// 时效窗口（epoch ms）；都为空 = 长期
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<RunRecord>,
    #[serde(default)]
    pub runs: Vec<RunRecord>,
}

enum Window {
    NotStarted,
    Expired,
    Active,
}

fn window_of(starts_at: Option<i64>, ends_at: Option<i64>, now: i64) -> Window {
    if starts_at.map_or(false, |s| now < s) {
        return Window::NotStarted;
    }
    if ends_at.map_or(false, |e| now >= e) {
        return Window::Expired;
    }
    Window::Active
}

/// 与 tokio-cron-scheduler 内部解析设置保持一致（必含秒域 + dom/dow 相与），
/// 这样保存时的校验结果和调度行为不会出现两套语义。
fn parse_cron(expr: &str) -> Result<croner::Cron, String> {
    CronParser::builder()
        .seconds(Seconds::Required)
        .dom_and_dow(true)
        .build()
        .parse(expr.trim())
        .map_err(|e| format!("无效的 cron 表达式「{expr}」: {e}"))
}

fn next_fire_ms(cron: &croner::Cron, after: DateTime<Local>) -> Option<i64> {
    cron.find_next_occurrence(&after, false).ok().map(|t| t.timestamp_millis())
}

fn next_n_fire_ms(cron: &croner::Cron, count: usize) -> Option<Vec<i64>> {
    let mut t = Local::now();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let next = cron.find_next_occurrence(&t, false).ok()?;
        out.push(next.timestamp_millis());
        t = next + Duration::milliseconds(1);
    }
    Some(out)
}

fn trim_runs(runs: &mut Vec<RunRecord>) {
    if runs.len() > KEEP_RUNS {
        let excess = runs.len() - KEEP_RUNS;
        runs.drain(0..excess);
    }
}

fn new_record(trigger: &str) -> RunRecord {
    RunRecord {
        run_id: Uuid::now_v7().to_string(),
        trigger: trigger.into(),
        started_at: now_ms(),
        finished_at: None,
        status: "running".into(),
        session_id: None,
        error: None,
    }
}

fn auto_envelope(session_id: &str, kind: &str, payload: Value) -> Envelope {
    Envelope {
        event_id: Uuid::now_v7(),
        session_id: session_id.to_string(),
        seq: None,
        persist: false,
        ts: Utc::now(),
        kind: kind.to_string(),
        payload,
    }
}

fn push_event(app: &AppHandle, env: Envelope) {
    let _ = app.emit("pidock:event", &env);
    if let Some(tx) = app.try_state::<broadcast::Sender<Envelope>>() {
        let _ = tx.send(env);
    }
}

#[derive(Clone)]
struct WatchEntry {
    job_id: String,
    run_id: String,
}

pub struct SchedulerManager {
    path: PathBuf,
    jobs: Mutex<Vec<ScheduledJob>>,
    sched: Mutex<Option<JobScheduler>>,
    /// 任务 id -> 调度器内部 uuid（用于 remove）
    sched_ids: Mutex<HashMap<String, Uuid>>,
    /// 正在运行的任务 id -> run id（防重入）
    in_flight: Mutex<HashMap<String, String>>,
    /// 运行中的会话 id -> watch 条目（完成追踪）
    watching: Mutex<HashMap<String, WatchEntry>>,
}

impl SchedulerManager {
    pub fn new(path: PathBuf) -> Self {
        let jobs = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<ScheduledJob>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            jobs: Mutex::new(jobs),
            sched: Mutex::new(None),
            sched_ids: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            watching: Mutex::new(HashMap::new()),
        }
    }

    pub fn default_jobs_path() -> PathBuf {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join("pidock").join("jobs.json")
    }

    fn save_locked(&self, jobs: &[ScheduledJob]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(jobs).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn get_job(&self, id: &str) -> Option<ScheduledJob> {
        self.jobs.lock().await.iter().find(|j| j.id == id).cloned()
    }

    /// 启动调度器并注册现有任务（幂等；setup 与首个 automation 请求都会触发）。
    pub async fn ensure_started(&self, app: &AppHandle) -> Result<(), String> {
        let mut guard = self.sched.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let sched = JobScheduler::new()
            .await
            .map_err(|e| format!("调度器初始化失败: {e}"))?;
        let jobs = self.jobs.lock().await.clone();
        for job in jobs.iter().filter(|j| j.enabled) {
            if let Err(e) = self.register(&sched, app, job).await {
                eprintln!("[scheduler] 注册任务「{}」失败: {e}", job.name);
            }
        }
        sched
            .start()
            .await
            .map_err(|e| format!("调度器启动失败: {e}"))?;
        *guard = Some(sched);
        Ok(())
    }

    /// 把一个任务注册进调度器（本地时区语义）。
    async fn register(&self, sched: &JobScheduler, app: &AppHandle, job: &ScheduledJob) -> Result<(), String> {
        parse_cron(&job.cron)?;
        let app2 = app.clone();
        let job_id = job.id.clone();
        let locked = Job::new_async_tz(job.cron.as_str(), Local, move |_uuid, _sched| {
            let app = app2.clone();
            let job_id = job_id.clone();
            Box::pin(async move {
                let mgr = app.state::<SchedulerManager>();
                if let Err(e) = mgr.on_fire(&app, &job_id).await {
                    eprintln!("[scheduler] 触发任务 {job_id} 失败: {e}");
                }
            })
        })
        .map_err(|e| format!("创建调度任务失败: {e}"))?;
        let uuid = sched
            .add(locked)
            .await
            .map_err(|e| format!("注册调度任务失败: {e}"))?;
        self.sched_ids.lock().await.insert(job.id.clone(), uuid);
        Ok(())
    }

    /// 从调度器摘除任务（存在才摘；幂等）。
    async fn unregister_job(&self, job_id: &str) {
        let uuid = self.sched_ids.lock().await.remove(job_id);
        if let Some(uuid) = uuid {
            let guard = self.sched.lock().await;
            if let Some(sched) = guard.as_ref() {
                let _ = sched.remove(&uuid).await;
            }
        }
    }

    async fn register_current(&self, app: &AppHandle, job: &ScheduledJob) -> Result<(), String> {
        let guard = self.sched.lock().await;
        let Some(sched) = guard.as_ref() else {
            // 调度器尚未启动：ensure_started 会按 jobs 现状注册
            return Ok(());
        };
        self.register(sched, app, job).await
    }

    /// cron 到点回调：时效判定 -> 防重入 -> 异步执行
    async fn on_fire(&self, app: &AppHandle, job_id: &str) -> Result<(), String> {
        let Some(job) = self.get_job(job_id).await else {
            return Ok(());
        };
        if !job.enabled {
            return Ok(());
        }
        let now = now_ms();
        match window_of(job.starts_at, job.ends_at, now) {
            Window::Expired => {
                // 已过有效期：从调度器摘除（保留在存储里，列表展示"已过期"）
                self.unregister_job(&job.id).await;
                return Ok(());
            }
            Window::NotStarted => return Ok(()),
            Window::Active => {}
        }
        let run = {
            let mut flight = self.in_flight.lock().await;
            if flight.contains_key(&job.id) {
                let mut rec = new_record("cron");
                rec.status = "skipped".into();
                rec.finished_at = Some(now);
                rec.error = Some("上一次运行尚未结束，本次跳过".into());
                drop(flight);
                self.push_record(app, &job.id, rec).await;
                return Ok(());
            }
            let rec = new_record("cron");
            flight.insert(job.id.clone(), rec.run_id.clone());
            rec
        };
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            execute_run(&app2, job, run).await;
        });
        Ok(())
    }

    /// 把一条运行记录写进任务（修剪条数、更新 last_run、保存、推事件）。
    async fn push_record(&self, app: &AppHandle, job_id: &str, rec: RunRecord) {
        let payload = {
            let mut jobs = self.jobs.lock().await;
            let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) else {
                return;
            };
            job.runs.push(rec.clone());
            trim_runs(&mut job.runs);
            job.last_run = Some(rec.clone());
            if let Err(e) = self.save_locked(&jobs) {
                eprintln!("[scheduler] 保存任务失败: {e}");
            }
            json!({
                "job_id": job_id,
                "run_id": rec.run_id,
                "trigger": rec.trigger,
                "status": rec.status,
                "error": rec.error,
                "session_id": rec.session_id,
            })
        };
        let kind = if rec.status == "running" {
            "automation.run_started"
        } else {
            "automation.run_finished"
        };
        push_event(app, auto_envelope(rec.session_id.as_deref().unwrap_or(""), kind, payload));
    }

    /// 登记完成追踪 + 超时兜底
    async fn watch_run(&self, app: &AppHandle, session_id: &str, job_id: String, run_id: String) {
        self.watching.lock().await.insert(
            session_id.to_string(),
            WatchEntry {
                job_id: job_id.clone(),
                run_id: run_id.clone(),
            },
        );
        let app2 = app.clone();
        let sid = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(RUN_TIMEOUT_SECS)).await;
            let mgr = app2.state::<SchedulerManager>();
            let still = {
                let w = mgr.watching.lock().await;
                w.get(&sid).map_or(false, |e| e.run_id == run_id)
            };
            if still {
                mgr.finalize(&app2, &job_id, &run_id, Some(&sid), "failed", Some("运行超时（30 分钟无响应）".into()))
                    .await;
            }
        });
    }

    /// 终结一次运行：更新记录、保存、推事件、清理追踪
    async fn finalize(&self, app: &AppHandle, job_id: &str, run_id: &str, session_id: Option<&str>, status: &str, error: Option<String>) {
        if let Some(sid) = session_id {
            self.watching.lock().await.remove(sid);
        }
        {
            let mut flight = self.in_flight.lock().await;
            if flight.get(job_id).map(|r| r.as_str()) == Some(run_id) {
                flight.remove(job_id);
            }
        }
        let payload = {
            let mut jobs = self.jobs.lock().await;
            let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) else {
                return;
            };
            let Some(rec) = job.runs.iter_mut().find(|r| r.run_id == run_id && r.status == "running") else {
                return;
            };
            rec.status = status.into();
            rec.finished_at = Some(now_ms());
            if let Some(sid) = session_id {
                rec.session_id = Some(sid.to_string());
            }
            if error.is_some() {
                rec.error = error.clone();
            }
            let payload = json!({
                "job_id": job_id,
                "run_id": run_id,
                "trigger": rec.trigger,
                "status": rec.status,
                "error": rec.error,
                "session_id": rec.session_id,
            });
            job.last_run = Some(rec.clone());
            if let Err(e) = self.save_locked(&jobs) {
                eprintln!("[scheduler] 保存任务失败: {e}");
            }
            payload
        };
        push_event(app, auto_envelope(session_id.unwrap_or(""), "automation.run_finished", payload));
    }

    /// 订阅 host 事件广播：agent 空闲 = 成功，error = 失败
    async fn handle_event(&self, app: &AppHandle, env: Envelope) {
        if env.session_id.is_empty() {
            return;
        }
        let entry = self.watching.lock().await.get(&env.session_id).cloned();
        let Some(entry) = entry else { return };
        let status = match env.kind.as_str() {
            k if k == ephemeral::AGENT_STATE_CHANGED => {
                if env.payload.get("state").and_then(|v| v.as_str()) == Some("idle") {
                    Some("ok")
                } else {
                    None
                }
            }
            k if k == ephemeral::ERROR => Some("failed"),
            _ => None,
        };
        if let Some(status) = status {
            let error = if status == "failed" {
                env.payload.get("message").and_then(|v| v.as_str()).map(|s| s.to_string())
            } else {
                None
            };
            self.finalize(app, &entry.job_id, &entry.run_id, Some(&env.session_id), status, error)
                .await;
        }
    }

    /// 事件循环：常驻订阅广播通道（lib.rs setup 里 spawn）
    pub fn spawn_event_watcher(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let tx = match app.try_state::<broadcast::Sender<Envelope>>() {
                Some(s) => s.inner().clone(),
                None => return,
            };
            let mut rx = tx.subscribe();
            loop {
                match rx.recv().await {
                    Ok(env) => {
                        let mgr = app.state::<SchedulerManager>();
                        mgr.handle_event(&app, env).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // ------------------------------------------------------------- CRUD

    async fn list(&self) -> Result<Value, String> {
        let jobs = self.jobs.lock().await.clone();
        let now = Local::now();
        let mut out = Vec::with_capacity(jobs.len());
        for job in jobs {
            let mut v = serde_json::to_value(&job).map_err(|e| e.to_string())?;
            let next = if job.enabled {
                parse_cron(&job.cron).ok().and_then(|c| c.find_next_occurrence(&now, false).ok()).map(|t| t.timestamp_millis())
            } else {
                None
            };
            v["next_run_at"] = next.map(|ms| json!(ms)).unwrap_or(Value::Null);
            out.push(v);
        }
        Ok(json!({ "jobs": out, "server_now": now_ms() }))
    }

    async fn save_job(&self, app: &AppHandle, params: &Value) -> Result<Value, String> {
        let mut job: ScheduledJob = serde_json::from_value(params.clone()).map_err(|e| format!("参数无效: {e}"))?;
        job.name = job.name.trim().to_string();
        job.prompt = job.prompt.trim().to_string();
        job.cron = job.cron.trim().to_string();
        if job.name.is_empty() {
            return Err("任务名称不能为空".into());
        }
        if job.prompt.is_empty() {
            return Err("提示词不能为空".into());
        }
        parse_cron(&job.cron)?;
        if !PERMISSION_MODES.contains(&job.permission_mode.as_str()) {
            return Err(format!("无效的权限模式「{}」", job.permission_mode));
        }
        if let (Some(s), Some(e)) = (job.starts_at, job.ends_at) {
            if s >= e {
                return Err("开始时间必须早于结束时间".into());
            }
        }
        {
            let mut jobs = self.jobs.lock().await;
            match jobs.iter().position(|j| j.id == job.id) {
                Some(idx) => {
                    let old = &jobs[idx];
                    job.created_at = old.created_at;
                    job.runs = old.runs.clone();
                    job.last_run = old.last_run.clone();
                    job.updated_at = now_ms();
                    jobs[idx] = job.clone();
                }
                None => {
                    if job.id.is_empty() {
                        job.id = Uuid::now_v7().to_string();
                    }
                    job.created_at = now_ms();
                    job.updated_at = now_ms();
                    jobs.push(job.clone());
                }
            }
            self.save_locked(&jobs)?;
        }
        self.unregister_job(&job.id).await;
        if job.enabled {
            self.register_current(app, &job).await?;
        }
        Ok(serde_json::to_value(&job).map_err(|e| e.to_string())?)
    }

    async fn delete_job(&self, _app: &AppHandle, params: &Value) -> Result<Value, String> {
        let id = params.get("id").and_then(|v| v.as_str()).ok_or("缺少 id")?;
        {
            let mut jobs = self.jobs.lock().await;
            let before = jobs.len();
            jobs.retain(|j| j.id != id);
            if jobs.len() == before {
                return Err("任务不存在".into());
            }
            self.save_locked(&jobs)?;
        }
        self.unregister_job(id).await;
        Ok(json!({ "ok": true }))
    }

    async fn set_enabled(&self, app: &AppHandle, params: &Value) -> Result<Value, String> {
        let id = params.get("id").and_then(|v| v.as_str()).ok_or("缺少 id")?;
        let enabled = params.get("enabled").and_then(|v| v.as_bool()).ok_or("缺少 enabled")?;
        let job = {
            let mut jobs = self.jobs.lock().await;
            let Some(j) = jobs.iter_mut().find(|j| j.id == id) else {
                return Err("任务不存在".into());
            };
            j.enabled = enabled;
            j.updated_at = now_ms();
            let job = j.clone();
            self.save_locked(&jobs)?;
            job
        };
        if enabled {
            self.register_current(app, &job).await?;
        } else {
            self.unregister_job(id).await;
        }
        Ok(json!({ "ok": true, "job": serde_json::to_value(&job).unwrap_or(Value::Null) }))
    }

    async fn run_now(&self, app: &AppHandle, params: &Value) -> Result<Value, String> {
        let id = params.get("id").and_then(|v| v.as_str()).ok_or("缺少 id")?;
        let job = self.get_job(id).await.ok_or("任务不存在")?;
        let run = {
            let mut flight = self.in_flight.lock().await;
            if flight.contains_key(&job.id) {
                return Err("该任务正在运行中".into());
            }
            let rec = new_record("manual");
            flight.insert(job.id.clone(), rec.run_id.clone());
            rec
        };
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            execute_run(&app2, job, run).await;
        });
        Ok(json!({ "ok": true }))
    }

    async fn peek(&self, params: &Value) -> Result<Value, String> {
        let expr = params.get("cron").and_then(|v| v.as_str()).ok_or("缺少 cron")?;
        let count = params.get("count").and_then(|v| v.as_u64()).unwrap_or(3).clamp(1, 10) as usize;
        let cron = parse_cron(expr)?;
        let times = next_n_fire_ms(&cron, count).ok_or("无法计算下一次触发时间")?;
        Ok(json!({ "times": times }))
    }
}

/// 真正执行一次：建会话 -> 权限 -> 命名 -> 发提示词，然后挂上完成追踪。
/// 任何一步失败都把运行记录终结为 failed。
async fn execute_run(app: &AppHandle, job: ScheduledJob, mut run: RunRecord) {
    let mgr = app.state::<SchedulerManager>();
    let supervisor = app.state::<Supervisor>();
    let exec = async {
        let created = supervisor
            .request(
                app.clone(),
                "session.create".into(),
                json!({
                    "cwd": job.workspace.clone().unwrap_or_default(),
                    "model": job.model,
                    "thinking_level": job.thinking_level,
                }),
            )
            .await?;
        let sid = created
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or("session.create 未返回 session_id")?
            .to_string();
        run.session_id = Some(sid.clone());
        // 新会话默认计划模式；无人值守只有放宽到对应模式才能用修改类工具
        if job.permission_mode != "plan" {
            supervisor
                .request(
                    app.clone(),
                    "session.set_permission_mode".into(),
                    json!({ "session_id": sid, "mode": job.permission_mode }),
                )
                .await?;
        }
        // 命名便于在侧栏识别定时任务的会话
        supervisor
            .request(app.clone(), "session.rename".into(), json!({ "session_id": sid, "name": format!("⏰ {}", job.name) }))
            .await?;
        let mut prompt_params = json!({ "session_id": sid, "text": job.prompt });
        if let Some(imgs) = &job.images {
            if !imgs.is_empty() {
                if let Ok(v) = serde_json::to_value(imgs) {
                    prompt_params["images"] = v;
                }
            }
        }
        supervisor
            .request(app.clone(), "agent.prompt".into(), prompt_params)
            .await?;
        Ok(sid)
    }
    .await;
    match exec {
        Ok(sid) => {
            mgr.push_record(app, &job.id, run.clone()).await;
            mgr.watch_run(app, &sid, job.id.clone(), run.run_id.clone()).await;
        }
        Err(e) => {
            mgr.push_record(
                app,
                &job.id,
                RunRecord {
                    status: "failed".into(),
                    finished_at: Some(now_ms()),
                    error: Some(e),
                    ..run
                },
            )
            .await;
        }
    }
}

#[tauri::command]
pub async fn automation_request(
    app: AppHandle,
    state: tauri::State<'_, SchedulerManager>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    state.ensure_started(&app).await?;
    match method.as_str() {
        "automation.list" => state.list().await,
        "automation.save" => state.save_job(&app, &params).await,
        "automation.delete" => state.delete_job(&app, &params).await,
        "automation.set_enabled" => state.set_enabled(&app, &params).await,
        "automation.run_now" => state.run_now(&app, &params).await,
        "automation.peek" => state.peek(&params).await,
        other => Err(format!("unknown automation method \"{other}\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn cron_parse_valid_and_invalid() {
        assert!(parse_cron("0 0 9 * * *").is_ok());
        assert!(parse_cron("*/30 * * * * *").is_ok());
        assert!(parse_cron("0 */2 * * * *").is_ok());
        assert!(parse_cron("0 30 9 * * Mon").is_ok());
        // 缺秒域 / 垃圾输入 / 越界值
        assert!(parse_cron("0 30 9 * *").is_err());
        assert!(parse_cron("not a cron").is_err());
        assert!(parse_cron("99 30 9 * * *").is_err());
    }

    #[test]
    fn next_fire_is_local_time() {
        // 每天 9 点 = 本地 09:00（tokio-cron-scheduler 用 new_async_tz(Local) 注册）
        let cron = parse_cron("0 0 9 * * *").unwrap();
        let now = Local::now();
        let next = cron.find_next_occurrence(&now, false).unwrap();
        assert!(next > now);
        assert_eq!(next.hour(), 9);
        assert_eq!(next.minute(), 0);
        assert_eq!(next.second(), 0);
    }

    #[test]
    fn next_n_fire_monotonic() {
        let cron = parse_cron("*/10 * * * * *").unwrap();
        let times = next_n_fire_ms(&cron, 3).unwrap();
        assert_eq!(times.len(), 3);
        assert!(times[0] < times[1] && times[1] < times[2]);
        assert!(times[0] > now_ms());
    }

    #[test]
    fn window_states() {
        let now = 1_000_000i64;
        assert!(matches!(window_of(None, None, now), Window::Active));
        assert!(matches!(window_of(Some(2_000_000), None, now), Window::NotStarted));
        assert!(matches!(window_of(None, Some(500_000), now), Window::Expired));
        assert!(matches!(window_of(Some(500_000), Some(2_000_000), now), Window::Active));
        // 边界：恰好等于结束时间视为过期
        assert!(matches!(window_of(None, Some(now), now), Window::Expired));
    }

    #[test]
    fn runs_trim_keeps_latest() {
        let mut runs: Vec<RunRecord> = (0..KEEP_RUNS + 5)
            .map(|i| RunRecord {
                run_id: (i as i64).to_string(),
                trigger: "cron".into(),
                started_at: i as i64,
                finished_at: None,
                status: "ok".into(),
                session_id: None,
                error: None,
            })
            .collect();
        trim_runs(&mut runs);
        assert_eq!(runs.len(), KEEP_RUNS);
        assert_eq!(runs[0].run_id, "5");
        assert_eq!(runs.last().unwrap().run_id, (KEEP_RUNS + 4).to_string());
    }

    #[test]
    fn jobs_json_roundtrip() {
        let job = ScheduledJob {
            id: Uuid::now_v7().to_string(),
            name: "每日站会摘要".into(),
            prompt: "总结昨天的进展".into(),
            cron: "0 30 9 * * *".into(),
            workspace: Some("D:\\work".into()),
            model: None,
            thinking_level: Some("medium".into()),
            images: Some(vec![JobImage { data: "aGk=".into(), mime_type: "image/png".into() }]),
            permission_mode: default_permission_mode(),
            starts_at: None,
            ends_at: Some(1798761600000),
            enabled: true,
            created_at: 1,
            updated_at: 2,
            last_run: None,
            runs: vec![],
        };
        let bytes = serde_json::to_vec(&vec![job.clone()]).unwrap();
        let back: Vec<ScheduledJob> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, job.name);
        assert_eq!(back[0].permission_mode, "full");
        assert_eq!(back[0].ends_at, job.ends_at);
    }
}
