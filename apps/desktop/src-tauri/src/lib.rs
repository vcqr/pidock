mod host;
mod scheduler;
mod sync;

use host::Supervisor;
use tauri::Manager;
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

/// 打开系统原生文件选择器（单选；用于导入技能压缩包/插件源文件）。取消返回 None。
#[tauri::command]
async fn pick_file() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<Option<String>, String> {
        const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; $owner = New-Object System.Windows.Forms.Form; $owner.TopMost = $true; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Filter = '技能包/插件 (*.zip;*.tgz;*.tar.gz;*.gz;*.ts;*.js)|*.zip;*.tgz;*.tar.gz;*.gz;*.ts;*.js|所有文件 (*.*)|*.*'; $d.Title = '选择要安装的技能包或插件'; if ($d.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $d.FileName }";
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", SCRIPT])
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
        .manage(supervisor)
        .manage(event_tx.clone())
        .manage(sync::SyncManager::new(default_sync_cfg_path(), sync_tx))
        .manage(scheduler::SchedulerManager::new(scheduler::SchedulerManager::default_jobs_path()))
        .invoke_handler(tauri::generate_handler![
            host::host_request,
            scheduler::automation_request,
            sync::sync_configure,
            sync::sync_status,
            sync::sync_disable,
            pick_folder,
            pick_file,
            reveal_path
        ])
        .setup(move |app| {
            sync::spawn(app.handle().clone(), event_rx, sync_rx);
            scheduler::SchedulerManager::spawn_event_watcher(app.handle().clone());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mgr = handle.state::<scheduler::SchedulerManager>();
                if let Err(e) = mgr.ensure_started(&handle).await {
                    eprintln!("[scheduler] 启动失败: {e}");
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            tauri::RunEvent::Exit => {
                if let Some(state) = app_handle.try_state::<Supervisor>() {
                    state.kill();
                }
            }
            _ => {}
        });
    // The webview teardown can hang on Windows; the state cleanup has already
    // run in the Exit handler, so force the process to exit.
    std::process::exit(0);
}
