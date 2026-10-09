//! 服务端文件系统读取：目录浏览（文件夹/文件选择器的数据源）。
//!
//! 命令面：`fs_list { path? }` → `{ path, parent, entries: [{ name, path, dir }] }`
//! （path 缺省为家目录；entries 含文件与目录，目录排前）。在 [`CoreCtx::handle`]
//! 分发，桌面与 webhost 壳共用——前端「打开文件夹」与文件选择弹层靠它
//! 浏览本机（webhost 即节点机）目录。

use std::path::PathBuf;

use serde_json::{json, Value};

pub async fn list(args: Value) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        // path 缺省 = 家目录（与 CorePaths::default_data 同一套环境变量约定）
        let requested = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let path = match requested {
            Some(p) => p,
            None => PathBuf::from(
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into()),
            ),
        };
        // canonicalize 顺带做存在性检查；不存在 / 无权限时报错给前端展示
        let path = path
            .canonicalize()
            .map_err(|e| format!("目录不可访问: {e}"))?;
        let mut entries: Vec<Value> = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| format!("读取目录失败: {e}"))? {
            let entry = entry.map_err(|e| format!("读取目录失败: {e}"))?;
            // metadata 跟随符号链接：链接到目录的也算目录
            let is_dir = std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false);
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "path": entry.path().to_string_lossy(),
                "dir": is_dir,
            }));
        }
        // 目录在前，同组内按名称排序（不分大小写）
        entries.sort_by(|a, b| {
            let da = a["dir"].as_bool().unwrap_or(false);
            let db = b["dir"].as_bool().unwrap_or(false);
            db.cmp(&da).then_with(|| {
                a["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
            })
        });
        let parent = path.parent().map(|p| p.to_string_lossy().to_string());
        // 常用目录（桌面/下载/图片/文档）：存在才返回，供选择弹层做快捷入口。
        // 家目录下点开头的系统目录很多，没有快捷入口时真实文件会被挤到列表底部
        let home = PathBuf::from(
            std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".into()),
        );
        let special = |name: &str| -> Value {
            let p = home.join(name);
            match p.canonicalize() {
                Ok(c) if c.is_dir() => json!(c.to_string_lossy()),
                _ => Value::Null,
            }
        };
        Ok(json!({
            "path": path.to_string_lossy(),
            "parent": parent,
            "entries": entries,
            "specials": {
                "desktop": special("Desktop"),
                "downloads": special("Downloads"),
                "pictures": special("Pictures"),
                "documents": special("Documents"),
            },
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}
