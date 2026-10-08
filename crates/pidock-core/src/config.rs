//! 桌面端本地偏好（desktop.json）：关窗行为 / 系统通知 / 开机自启。
//! 桌面壳存 Tauri app_config_dir，webhost 存数据目录；schema 与读写共用。

use serde::{Deserialize, Serialize};
use std::path::Path;

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

impl DesktopConfig {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => DesktopConfig::default(),
        }
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}
