mod host;
mod scheduler;
mod sync;
mod tray;

use host::Supervisor;
use tauri::{Manager, WindowEvent};
use tokio::sync::{broadcast, mpsc};

fn default_sync_cfg_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(base)
        .join("pidock")
        .join("sync.json")
}

/// 打开系统原生目录选择器（Windows: PowerShell WinForms FolderBrowserDialog）。
/// 用户取消时返回 None。
#[tauri::command]
async fn pick_folder() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<Option<String>, String> {
        const SCRIPT: &str = "\
Add-Type -AssemblyName System.Windows.Forms; \
$owner = New-Object System.Windows.Forms.Form; \
$owner.TopMost = $true; \
$d = New-Object System.Windows.Forms.FolderBrowserDialog; \
$d.Description = '选择项目文件夹'; \
if ($d.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $d.SelectedPath }";
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", SCRIPT])
            .output()
            .map_err(|e| format!("failed to launch folder picker: {e}"))?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if text.is_empty() { None } else { Some(text) })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 打开系统原生文件选择器（单选）。kind 决定过滤器预设：
/// "image" = 头像图片；缺省 = 技能压缩包/插件源文件。取消返回 None。
#[tauri::command]
async fn pick_file(kind: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        let (filter, title) = match kind.as_deref() {
            Some("image") => (
                "图片 (*.png;*.jpg;*.jpeg;*.webp;*.gif)|*.png;*.jpg;*.jpeg;*.webp;*.gif|所有文件 (*.*)|*.*",
                "选择头像图片",
            ),
            _ => (
                "技能包/插件 (*.zip;*.tgz;*.tar.gz;*.gz;*.ts;*.js)|*.zip;*.tgz;*.tar.gz;*.gz;*.ts;*.js|所有文件 (*.*)|*.*",
                "选择要安装的技能包或插件",
            ),
        };
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $owner = New-Object System.Windows.Forms.Form; $owner.TopMost = $true; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Filter = '{}'; $d.Title = '{}'; if ($d.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {{ Write-Output $d.FileName }}",
            filter, title
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", &script])
            .output()
            .map_err(|e| format!("failed to launch file picker: {e}"))?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if text.is_empty() { None } else { Some(text) })
    })
    .await
    .map_err(|e| e.to_string())?
}

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

pub fn run() {
    let supervisor = Supervisor::default();
    // host events fan out to the sync-agent through this channel
    let (event_tx, event_rx) = broadcast::channel(2048);
    let (sync_tx, sync_rx) = mpsc::channel::<sync::SyncControl>(8);

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
        .manage(supervisor)
        .manage(event_tx.clone())
        .manage(sync::SyncManager::new(default_sync_cfg_path(), sync_tx))
        .manage(scheduler::SchedulerManager::new(
            scheduler::SchedulerManager::default_jobs_path(),
        ))
        .invoke_handler(tauri::generate_handler![
            host::host_request,
            scheduler::automation_request,
            sync::sync_configure,
            sync::sync_status,
            sync::sync_disable,
            tray::desktop_config_get,
            tray::desktop_config_set,
            pick_folder,
            pick_file,
            reveal_path
        ])
        .setup(move |app| {
            tray::init(app.handle())?;
            tray::spawn_watcher(app.handle());
            sync::spawn(app.handle().clone(), event_rx, sync_rx);
            scheduler::SchedulerManager::spawn_event_watcher(app.handle().clone());
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
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mgr = handle.state::<scheduler::SchedulerManager>();
                if let Err(e) = mgr.ensure_started(&handle).await {
                    eprintln!("[scheduler] 启动失败: {e}");
                }
            });
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
                if let Some(state) = app_handle.try_state::<Supervisor>() {
                    state.shutdown();
                }
            }
            _ => {}
        });
    // The webview teardown can hang on Windows; the state cleanup has already
    // run in the Exit handler, so force the process to exit.
    std::process::exit(0);
}
