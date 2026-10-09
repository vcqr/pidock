mod tray;

use std::sync::Arc;

use pidock_core::sync::SyncConfigureBody;
use pidock_core::{CoreCtx, CorePaths};
use serde_json::Value;
use tauri::{Emitter, Manager, WindowEvent};
use tokio::sync::{broadcast, mpsc};

/// 在系统文件管理器中打开目录（Windows: explorer，macOS: Finder，Linux: xdg-open）。
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open explorer: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open finder: {e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file manager: {e}"))?;
    }
    Ok(())
}

// 以下命令都是 pidock-core 的薄转换层：核心能力（supervisor / 调度器 / 云同步）
// 与 UI 壳解耦，无头 webhost 壳走同一套实现（CoreCtx::handle）。

#[tauri::command]
async fn host_request(
    state: tauri::State<'_, Arc<CoreCtx>>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    state.supervisor.request(method, params).await
}

#[tauri::command]
async fn automation_request(
    state: tauri::State<'_, Arc<CoreCtx>>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    pidock_core::scheduler::dispatch_automation(&state.scheduler, &method, params).await
}

#[tauri::command]
async fn sync_configure(
    state: tauri::State<'_, Arc<CoreCtx>>,
    body: SyncConfigureBody,
) -> Result<Value, String> {
    state.sync.configure(body).await
}

#[tauri::command]
async fn sync_disable(state: tauri::State<'_, Arc<CoreCtx>>) -> Result<Value, String> {
    state.sync.disable().await
}

#[tauri::command]
async fn sync_status(state: tauri::State<'_, Arc<CoreCtx>>) -> Result<Value, String> {
    state.sync.status().await
}

// git 分支信息与切换（项目 chip 的分支选择器）：CoreCtx::handle 的薄转换层

#[tauri::command]
async fn git_info(state: tauri::State<'_, Arc<CoreCtx>>, cwd: String) -> Result<Value, String> {
    state.handle("git_info", serde_json::json!({ "cwd": cwd })).await
}

#[tauri::command]
async fn git_branches(state: tauri::State<'_, Arc<CoreCtx>>, cwd: String) -> Result<Value, String> {
    state.handle("git_branches", serde_json::json!({ "cwd": cwd })).await
}

#[tauri::command]
async fn git_checkout(
    state: tauri::State<'_, Arc<CoreCtx>>,
    cwd: String,
    branch: String,
    create: Option<bool>,
) -> Result<Value, String> {
    state
        .handle(
            "git_checkout",
            serde_json::json!({ "cwd": cwd, "branch": branch, "create": create }),
        )
        .await
}

#[tauri::command]
async fn fs_list(state: tauri::State<'_, Arc<CoreCtx>>, path: Option<String>) -> Result<Value, String> {
    state.handle("fs_list", serde_json::json!({ "path": path })).await
}

pub fn run() {
    let (sync_tx, sync_rx) = mpsc::channel::<pidock_core::sync::SyncControl>(8);
    let ctx = CoreCtx::new(CorePaths::default_data(), sync_tx);

    tauri::Builder::default()
        // 单实例必须最先注册：二次启动（含托盘通知点击唤起的进程）改为聚焦已有窗口
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .manage(ctx.clone())
        .invoke_handler(tauri::generate_handler![
            host_request,
            automation_request,
            sync_configure,
            sync_status,
            sync_disable,
            git_info,
            git_branches,
            git_checkout,
            fs_list,
            tray::desktop_config_get,
            tray::desktop_config_set,
            reveal_path
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // desktop.json 落在 Tauri app_config_dir（沿用历史路径，其余文件走默认布局）
            if let Ok(dir) = handle.path().app_config_dir() {
                ctx.set_desktop_cfg_path(dir.join("desktop.json"));
            }
            tray::init(&handle)?;
            tray::spawn_watcher(&handle);
            // 事件泵：core 事件广播 -> webview。webview 尚未加载时先订阅，
            // 不会丢 setup 之后的首批事件（原实现为 supervisor 直发，现统一走广播）。
            let mut pump_rx = ctx.events.subscribe();
            let pump_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match pump_rx.recv().await {
                        Ok(env) => {
                            let _ = pump_handle.emit("pidock:event", &env);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            // core 后台循环：云同步 agent + 调度器（watcher / 启动）
            let start_ctx = ctx.clone();
            tauri::async_runtime::spawn(async move {
                start_ctx.start(sync_rx).await;
            });
            // 开机自启以 --tray 启动：窗口显示后立刻收进托盘
            if std::env::args().any(|a| a == "--tray") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.hide();
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && tray::handle_close_request(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            tauri::RunEvent::Exit => {
                tray::remove_tray(app_handle);
                let ctx = app_handle.state::<Arc<CoreCtx>>();
                ctx.supervisor.shutdown();
            }
            _ => {}
        });
    // The webview teardown can hang on Windows; the state cleanup has already
    // run in the Exit handler, so force the process to exit.
    std::process::exit(0);
}
