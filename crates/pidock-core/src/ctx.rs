//! CoreCtx：supervisor / scheduler / sync / 事件广播打包成一个可托管的整体。
//!
//! 壳层（Tauri 桌面、无头 webhost）构造一份 [`Arc<CoreCtx>`]，然后：
//! - 订阅 [`CoreCtx::events`] 分发事件（桌面泵到 webview，webhost 推给浏览器）；
//! - 调 [`CoreCtx::start`] 拉起后台循环（sync loop、调度器 watcher + 启动）；
//! - 命令统一走 [`CoreCtx::handle`]，与前端 `ipc(cmd, args)` 一一对应。

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::config::DesktopConfig;
use crate::scheduler::SchedulerManager;
use crate::supervisor::Supervisor;
use crate::sync::{SyncConfigureBody, SyncControl, SyncManager};

/// 壳层各自的持久化文件路径（桌面用 APPDATA/app_config，webhost 用数据目录）
pub struct CorePaths {
    pub jobs: PathBuf,
    pub sync_cfg: PathBuf,
    pub desktop_cfg: PathBuf,
}

impl CorePaths {
    /// 桌面壳与 webhost 的缺省布局：`<APPDATA|HOME>/pidock/{jobs.json, sync.json}`。
    /// desktop.json 由桌面壳在 setup 里覆盖为 app_config_dir（保持历史路径）。
    pub fn default_data() -> Self {
        let base = std::env::var("APPDATA")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".into());
        let dir = PathBuf::from(base).join("pidock");
        CorePaths {
            jobs: dir.join("jobs.json"),
            sync_cfg: dir.join("sync.json"),
            desktop_cfg: dir.join("desktop.json"),
        }
    }
}

pub struct CoreCtx {
    pub supervisor: Arc<Supervisor>,
    pub scheduler: Arc<SchedulerManager>,
    pub sync: Arc<SyncManager>,
    /// host / 调度器事件广播；壳层按需 subscribe
    pub events: broadcast::Sender<pidock_protocol::Envelope>,
    desktop_cfg_path: RwLock<PathBuf>,
}

impl CoreCtx {
    pub fn new(paths: CorePaths, sync_control: mpsc::Sender<SyncControl>) -> Arc<Self> {
        // 容量与桌面时代一致（2048）：慢订阅者最多回退一秒量级的流式事件
        let (events, _) = broadcast::channel(2048);
        let supervisor = Arc::new(Supervisor::new(events.clone()));
        let scheduler = Arc::new(SchedulerManager::new(
            paths.jobs,
            supervisor.clone(),
            events.clone(),
        ));
        let sync = Arc::new(SyncManager::new(paths.sync_cfg, sync_control));
        Arc::new(CoreCtx {
            supervisor,
            scheduler,
            sync,
            events,
            desktop_cfg_path: RwLock::new(paths.desktop_cfg),
        })
    }

    /// 桌面壳在 setup 里把 desktop.json 指到 Tauri app_config_dir
    pub fn set_desktop_cfg_path(&self, path: PathBuf) {
        *self.desktop_cfg_path.write().unwrap() = path;
    }

    fn desktop_cfg_path(&self) -> PathBuf {
        self.desktop_cfg_path.read().unwrap().clone()
    }

    /// 拉起后台循环：sync loop、调度器事件 watcher、调度器本体。
    /// 必须在 tokio 运行时上下文里调用（桌面壳经 tauri::async_runtime::spawn 包一层）。
    pub async fn start(self: &Arc<Self>, sync_control_rx: mpsc::Receiver<SyncControl>) {
        // sync agent：订阅事件广播 + 等控制命令
        let event_rx = self.events.subscribe();
        tokio::spawn(crate::sync::run_sync_loop(
            self.supervisor.clone(),
            self.scheduler.clone(),
            self.sync.clone(),
            event_rx,
            sync_control_rx,
        ));
        // 调度器：事件 watcher（完成追踪）+ 本体启动
        self.scheduler.spawn_event_watcher();
        let sched = self.scheduler.clone();
        tokio::spawn(async move {
            if let Err(e) = sched.ensure_started().await {
                eprintln!("[scheduler] 启动失败: {e}");
            }
        });
    }

    /// 统一命令面：cmd 与前端 `ipc(cmd, args)` 的命令名一一对应（沿用了
    /// Tauri 命令名与参数形状），host_request / automation_request / sync_*
    /// 与桌面命令行为一致；pick_* / reveal_path 在无头壳层没有意义，明确报错。
    pub async fn handle(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value, String> {
        match cmd {
            "host_request" => {
                let method = args
                    .get("method")
                    .and_then(|v| v.as_str())
                    .ok_or("host_request 缺少 method")?
                    .to_string();
                let params = args.get("params").cloned().unwrap_or(json!({}));
                self.supervisor.request(method, params).await
            }
            "automation_request" => {
                let method = args
                    .get("method")
                    .and_then(|v| v.as_str())
                    .ok_or("automation_request 缺少 method")?
                    .to_string();
                let params = args.get("params").cloned().unwrap_or(json!({}));
                crate::scheduler::dispatch_automation(&self.scheduler, &method, params).await
            }
            "sync_configure" => {
                let body: SyncConfigureBody =
                    serde_json::from_value(args.get("body").cloned().unwrap_or(Value::Null))
                        .map_err(|e| format!("sync_configure 参数无效: {e}"))?;
                self.sync.configure(body).await
            }
            "sync_disable" => self.sync.disable().await,
            "sync_status" => self.sync.status().await,
            "desktop_config_get" => {
                serde_json::to_value(DesktopConfig::load(&self.desktop_cfg_path()))
                    .map_err(|e| e.to_string())
            }
            "desktop_config_set" => {
                let cfg: DesktopConfig =
                    serde_json::from_value(args.get("config").cloned().unwrap_or(Value::Null))
                        .map_err(|e| format!("desktop_config_set 参数无效: {e}"))?;
                cfg.save(&self.desktop_cfg_path());
                Ok(json!({"ok": true}))
            }
            "pick_folder" | "pick_file" => Err("Web 模式不支持系统文件对话框".into()),
            "reveal_path" => Err("Web 模式不支持打开本机文件管理器".into()),
            other => Err(format!("unknown command \"{other}\"")),
        }
    }
}
