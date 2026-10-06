//! 系统托盘：图标角标、通知、关闭到托盘、开机自启。
//!
//! 全部由 Rust watcher 消费 host 事件广播驱动（agent_state_changed /
//! session_meta / automation.run_finished），不需要前端推送：
//! - watcher 维护「当前等待确认的会话集合」→ 角标图标与 tooltip；
//! - 等待/自动化结束在窗口隐藏或失焦时发系统通知（title 从 session_meta 缓存）。
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use pidock_protocol::{ephemeral, event, Envelope};
use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{image::Image, AppHandle, Manager, Wry};
use tokio::sync::broadcast;

/// 桌面端本地偏好（%APPDATA%/app.pidock.desktop/desktop.json），与 host 的 settings.json 无关
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    /// 关窗行为："hide" = 隐藏到托盘（默认），"exit" = 直接退出
    pub close_action: String,
    /// 系统通知总开关（窗口隐藏/失焦时才发）
    pub notifications: bool,
    /// 开机自动启动（Run 项，带 --tray 参数静默启动到托盘）
    pub autostart: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        DesktopConfig {
            close_action: "hide".into(),
            notifications: true,
            autostart: false,
        }
    }
}

pub struct TrayState {
    config: Mutex<DesktopConfig>,
    /// 会话标题缓存（session_meta 事件，通知文案用）
    titles: Mutex<HashMap<String, String>>,
    /// 当前处于等待确认/提问状态的会话；同时承担等待通知去重（状态离开 waiting 时移除）
    waiting: Mutex<HashSet<String>>,
    hint_shown: AtomicBool,
    tray: Mutex<Option<TrayIcon>>,
    icon_normal: Image<'static>,
    icon_attention: Image<'static>,
}

fn config_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    Some(dir.join("desktop.json"))
}

fn load_config(app: &AppHandle) -> DesktopConfig {
    let Some(path) = config_path(app) else {
        return DesktopConfig::default();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => DesktopConfig::default(),
    }
}

fn save_config(app: &AppHandle, cfg: &DesktopConfig) {
    if let Some(path) = config_path(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(cfg) {
            let _ = std::fs::write(path, text);
        }
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<TrayState>();
    let cfg = state.config.lock().unwrap().clone();

    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let notify = CheckMenuItem::with_id(
        app,
        "toggle-notify",
        "系统通知（窗口隐藏时提醒）",
        true,
        cfg.notifications,
        None::<&str>,
    )?;
    let close_hide = CheckMenuItem::with_id(
        app,
        "close-hide",
        "关闭窗口时隐藏到托盘",
        true,
        cfg.close_action == "hide",
        None::<&str>,
    )?;
    let autostart = CheckMenuItem::with_id(
        app,
        "toggle-autostart",
        "开机自动启动",
        true,
        cfg.autostart,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "退出 PiDock", true, None::<&str>)?;
    MenuBuilder::new(app)
        .item(&show)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&notify)
        .item(&close_hide)
        .item(&autostart)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit)
        .build()
}

/// 重建并替换托盘菜单（开关勾选态烤在菜单项里，配置变化后需整体重建）
fn refresh_menu(app: &AppHandle) {
    match build_menu(app) {
        Ok(menu) => {
            let state = app.state::<TrayState>();
            let tray_guard = state.tray.lock().unwrap();
            if let Some(tray) = tray_guard.as_ref() {
                let _ = tray.set_menu(Some(menu));
            }
        }
        Err(e) => eprintln!("[tray] 重建托盘菜单失败: {e}"),
    }
}

/// 按等待集合刷新角标图标与 tooltip
fn update_attention(app: &AppHandle) {
    let state = app.state::<TrayState>();
    let n = state.waiting.lock().unwrap().len();
    // guard 显式落局部变量：尾位置 if-let 的临时值会活到 state 之后（E0597）
    let tray_guard = state.tray.lock().unwrap();
    if let Some(tray) = tray_guard.as_ref() {
        let _ = tray.set_icon(Some(if n > 0 {
            state.icon_attention.clone()
        } else {
            state.icon_normal.clone()
        }));
        let _ = tray.set_tooltip(Some(if n > 0 {
            format!("PiDock\n{n} 个会话等待确认")
        } else {
            "PiDock".to_string()
        }));
    }
}

fn apply_autostart(app: &AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let launch = app.autolaunch();
    let r = if enable { launch.enable() } else { launch.disable() };
    if let Err(e) = r {
        eprintln!("[tray] 设置开机自启失败: {e}");
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 关窗拦截：返回 true 表示已隐藏、应阻止默认关闭。退出路径不在此处理，
/// 交给 RunEvent::Exit 的 Supervisor 平滑关闭。
pub fn handle_close_request(app: &AppHandle) -> bool {
    let state = app.state::<TrayState>();
    let hide = state.config.lock().unwrap().close_action == "hide";
    if !hide {
        return false;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
        if !state.hint_shown.swap(true, Ordering::SeqCst) {
            maybe_notify(
                app,
                "PiDock 已最小化到托盘",
                "会话仍在后台运行，点击托盘图标可重新打开",
            );
        }
    }
    true
}

/// 窗口隐藏或失焦才真正弹出系统通知
fn maybe_notify(app: &AppHandle, title: &str, body: &str) {
    {
        let state = app.state::<TrayState>();
        if !state.config.lock().unwrap().notifications {
            return;
        }
    }
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(true);
        let focused = win.is_focused().unwrap_or(true);
        if visible && focused {
            return;
        }
    }
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let config = load_config(app);
    let icon_normal = Image::from_bytes(include_bytes!("../icons/tray.png"))?.to_owned();
    let icon_attention = Image::from_bytes(include_bytes!("../icons/tray-attention.png"))?.to_owned();
    let initial_icon = icon_normal.clone();
    app.manage(TrayState {
        config: Mutex::new(config),
        titles: Mutex::new(HashMap::new()),
        waiting: Mutex::new(HashSet::new()),
        hint_shown: AtomicBool::new(false),
        tray: Mutex::new(None),
        icon_normal,
        icon_attention,
    });

    // 菜单必须挂在 builder 上：build_menu 返回 Menu，set_menu 要等 tray 存在后才能用
    let menu = build_menu(app)?;
    let tray = TrayIconBuilder::with_id("pidock-tray")
        .icon(initial_icon)
        .tooltip("PiDock")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            match event.id().0.as_str() {
                "show" => show_main(app),
                "quit" => app.exit(0),
                "toggle-notify" => {
                    let cfg = {
                        let state = app.state::<TrayState>();
                        let mut c = state.config.lock().unwrap();
                        c.notifications = !c.notifications;
                        c.clone()
                    };
                    save_config(app, &cfg);
                    refresh_menu(app);
                }
                "close-hide" => {
                    let cfg = {
                        let state = app.state::<TrayState>();
                        let mut c = state.config.lock().unwrap();
                        c.close_action =
                            if c.close_action == "hide" { "exit" } else { "hide" }.into();
                        c.clone()
                    };
                    save_config(app, &cfg);
                    refresh_menu(app);
                }
                "toggle-autostart" => {
                    let (cfg, enable) = {
                        let state = app.state::<TrayState>();
                        let mut c = state.config.lock().unwrap();
                        c.autostart = !c.autostart;
                        (c.clone(), c.autostart)
                    };
                    apply_autostart(app, enable);
                    save_config(app, &cfg);
                    refresh_menu(app);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let visible = win.is_visible().unwrap_or(false);
                    let focused = win.is_focused().unwrap_or(false);
                    if visible && focused {
                        let _ = win.hide();
                    } else {
                        show_main(app);
                    }
                }
            }
        })
        .build(app)?;
    *app.state::<TrayState>().tray.lock().unwrap() = Some(tray);
    Ok(())
}

/// 直接消费 host 事件广播的第三路：等待确认/提问 → 角标 + 系统通知，
/// 自动化任务结束 → 系统通知。独立于前端，窗口隐藏时 webview 定时器
/// 被节流也不影响关键提醒。
pub fn spawn_watcher(app: &AppHandle) {
    // broadcast::Sender::subscribe 不会失败（容量固定），直接拿接收端
    let mut rx = app.state::<broadcast::Sender<Envelope>>().subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let env = match rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            };
            if env.kind == ephemeral::AGENT_STATE_CHANGED {
                let st = env.payload.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let state = app.state::<TrayState>();
                let changed = {
                    let mut waiting = state.waiting.lock().unwrap();
                    if st == "waiting_approval" || st == "waiting_ask" {
                        waiting.insert(env.session_id.clone())
                    } else {
                        waiting.remove(&env.session_id)
                    }
                };
                drop(state);
                if !changed {
                    continue;
                }
                update_attention(&app);
                if st == "waiting_approval" || st == "waiting_ask" {
                    // 先落局部变量：块尾表达式里的 MutexGuard 临时值会活到 state 之后（E0597）
                    let title = {
                        let state = app.state::<TrayState>();
                        let cached = state.titles.lock().unwrap().get(&env.session_id).cloned();
                        cached.unwrap_or_else(|| {
                            format!("会话 {}", &env.session_id[..env.session_id.len().min(8)])
                        })
                    };
                    let what =
                        if st == "waiting_approval" { "等待你的确认" } else { "等待你的回答" };
                    maybe_notify(&app, "PiDock · 需要你处理", &format!("会话「{title}」{what}"));
                }
            } else if env.kind == event::SESSION_META {
                // host 生成/更新会话标题后推送，缓存给通知文案
                if let Some(name) = env.payload.get("name").and_then(|v| v.as_str()) {
                    if !name.is_empty() {
                        let state = app.state::<TrayState>();
                        state
                            .titles
                            .lock()
                            .unwrap()
                            .insert(env.session_id.clone(), name.to_string());
                    }
                }
            } else if env.kind == "automation.run_finished" {
                let status = env.payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let job_id = env.payload.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = crate::scheduler::SchedulerManager::job_name_of(&app, job_id).await;
                let name = name.as_deref().unwrap_or("定时任务");
                if status == "ok" {
                    maybe_notify(&app, "自动化任务完成", &format!("「{name}」运行成功"));
                } else if status == "failed" {
                    let err = env
                        .payload
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("运行失败");
                    maybe_notify(&app, "自动化任务失败", &format!("「{name}」{err}"));
                }
            }
        }
    });
}

#[tauri::command]
pub fn desktop_config_get(app: AppHandle) -> DesktopConfig {
    app.state::<TrayState>().config.lock().unwrap().clone()
}

#[tauri::command]
pub fn desktop_config_set(app: AppHandle, config: DesktopConfig) -> Result<(), String> {
    let prev_autostart = app.state::<TrayState>().config.lock().unwrap().autostart;
    if prev_autostart != config.autostart {
        apply_autostart(&app, config.autostart);
    }
    save_config(&app, &config);
    *app.state::<TrayState>().config.lock().unwrap() = config;
    refresh_menu(&app);
    Ok(())
}

/// 进程退出前显式移除托盘图标（Windows 上强退后死图标要悬停才消失）
pub fn remove_tray(app: &AppHandle) {
    if let Some(tray) = app.state::<TrayState>().tray.lock().unwrap().take() {
        drop(tray);
    }
}
