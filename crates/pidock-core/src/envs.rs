//! 设置中心「常规 · 环境变量」的统一出口。
//!
//! `~/.pi/agent/pidock/settings.json` 的 `env` 字段（字符串键值表），supervisor
//! 拉起 pi-host 时逐项注入子进程；在代理派生变量（[`crate::proxy`]）之后写入，
//! 同名的用户显式配置覆盖代理派生值。代理仍负责 HTTP(S)_PROXY/NO_PROXY 等，
//! 这里的 env 面向其余变量（如 API 网关地址、语言区域、扩展所需凭据）。
//! 修改后需重启应用生效；非法条目（手编文件兜底）跳过不注入。

use serde_json::Value;

fn pidock_settings_path() -> Option<std::path::PathBuf> {
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(
        std::path::Path::new(&base)
            .join(".pi")
            .join("agent")
            .join("pidock")
            .join("settings.json"),
    )
}

/// 读取并解析 env 表。配置缺失/文件损坏时返回空表（保持历史行为）。
pub fn env_overrides() -> Vec<(String, String)> {
    let text = pidock_settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok());
    parse_env_setting(text.as_deref())
}

/// 合法变量名：字母/下划线开头，其余为字母/数字/下划线（与 host 写入校验一致）
fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 解析 settings.json 文本中的 `env` 表：仅保留合法键名与字符串值，
/// 其余条目（含非字符串值）打日志跳过，避免单个坏条目拖垮整个注入。
fn parse_env_setting(text: Option<&str>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(text) = text else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return out;
    };
    let Some(table) = v.get("env").and_then(Value::as_object) else {
        return out;
    };
    for (key, value) in table {
        if !valid_env_key(key) {
            eprintln!("[envs] 非法变量名，跳过：{key}");
            continue;
        }
        match value.as_str() {
            Some(value) => out.push((key.clone(), value.to_string())),
            None => eprintln!("[envs] 非字符串值，跳过：{key}"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_key_validation() {
        assert!(valid_env_key("HTTP_TIMEOUT"));
        assert!(valid_env_key("_PRIVATE"));
        assert!(valid_env_key("a9"));
        assert!(!valid_env_key("9A"));
        assert!(!valid_env_key("bad-key"));
        assert!(!valid_env_key("空"));
        assert!(!valid_env_key(""));
    }

    #[test]
    fn parse_env_setting_keeps_valid_and_skips_bad() {
        let text = r#"{
            "proxy": { "mode": "direct" },
            "env": {
                "API_GATEWAY": "https://gw.corp.example",
                "LC_ALL": "zh_CN.UTF-8",
                "bad-key": "x",
                "EMPTY_OK": "",
                "NOT_STR": 42
            }
        }"#;
        let out = parse_env_setting(Some(text));
        // serde_json Map 按键排序，输出顺序确定
        assert_eq!(
            out,
            vec![
                ("API_GATEWAY".to_string(), "https://gw.corp.example".to_string()),
                ("EMPTY_OK".to_string(), String::new()),
                ("LC_ALL".to_string(), "zh_CN.UTF-8".to_string()),
            ]
        );
    }

    #[test]
    fn parse_env_setting_missing_or_broken_file_means_empty() {
        assert!(parse_env_setting(None).is_empty());
        assert!(parse_env_setting(Some("not json")).is_empty());
        assert!(parse_env_setting(Some(r#"{"proxy":{}}"#)).is_empty());
        assert!(parse_env_setting(Some(r#"{"env": []}"#)).is_empty());
    }
}
