//! 服务端文件系统读取：目录浏览（文件夹/文件选择器的数据源）。
//!
//! 命令面：`fs_list { path? }` → `{ path, parent, entries: [{ name, path, dir }], drives }`
//! （path 缺省为家目录；entries 含文件与目录，目录排前；drives 为系统磁盘与已挂载卷，
//! 供前端「此电脑」视图）。在 [`CoreCtx::handle`] 分发，桌面与 webhost 壳共用——前端
//! 「打开文件夹」与文件选择弹层靠它浏览本机（webhost 即节点机）目录。
//!
//! Windows 返回的路径一律剥掉 canonicalize 产生的 `\\?\` verbatim 前缀并把分隔符
//! 归一为 `/`（Rust/Node 在 Windows 上都接受 `/`）；需要反斜杠的原生调用（如
//! explorer /select）由调用方自行转回。

use std::path::Path;

use serde_json::{json, Value};

/// Windows 下 canonicalize 返回 `\\?\C:\...`（UNC 为 `\\?\UNC\server\share\...`），
/// 展示与下游使用都不友好：剥掉前缀，并把分隔符统一为 `/`。
#[cfg(windows)]
fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    };
    s.replace('\\', "/")
}

#[cfg(not(windows))]
fn display_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Windows 逻辑盘符（GetLogicalDrives 位掩码，即时返回，不会像逐字母探测那样
/// 在断连的网络盘上卡住）。非 Windows 平台为空，前端据此隐藏「此电脑」入口。
#[cfg(windows)]
fn drives() -> Vec<Value> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let letter = (b'A' + i as u8) as char;
            json!({ "name": format!("{letter}:"), "path": format!("{letter}:/") })
        })
        .collect()
}

#[cfg(not(windows))]
fn drives() -> Vec<Value> {
    // 各平台的「磁盘/挂载卷」：macOS 列 /Volumes（系统盘是指向 / 的那条链接）；
    // Linux 从 /proc/self/mounts 取 /media、/run/media、/mnt 下的挂载点，
    // 避免 snap/loop 等伪挂载与系统目录刷屏。列表非空时前端才显示「此电脑」。
    let mut out: Vec<Value> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if let Ok(rd) = std::fs::read_dir("/Volumes") {
            for entry in rd.flatten() {
                let Ok(canon) = entry.path().canonicalize() else {
                    continue;
                };
                if !canon.is_dir() {
                    continue;
                }
                // /Volumes/Macintosh HD 即系统盘，路径归一为 /
                let path = if canon.as_os_str() == "/" {
                    "/".to_string()
                } else {
                    display_path(&canon)
                };
                out.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "path": path,
                }));
            }
        }
        if !out.iter().any(|d| d["path"] == "/") {
            out.insert(0, json!({ "name": "/", "path": "/" }));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") {
            let mut seen: Vec<String> = Vec::new();
            for line in mounts.lines() {
                // 每行：设备 挂载点 文件系统 ...，取第 2 列挂载点
                let mut fields = line.split_whitespace();
                if fields.next().is_none() {
                    continue;
                }
                let Some(mp) = fields.next() else { continue };
                let mp = unescape_mount(mp);
                let mounted_here = mp == "/mnt"
                    || mp.starts_with("/mnt/")
                    || mp.starts_with("/media/")
                    || mp.starts_with("/run/media/");
                if !mounted_here || seen.contains(&mp) {
                    continue;
                }
                seen.push(mp.clone());
                let name = mp.rsplit_once('/').map(|(_, n)| n).filter(|n| !n.is_empty());
                out.push(json!({
                    "name": name.map(str::to_owned).unwrap_or_else(|| mp.clone()),
                    "path": display_path(Path::new(&mp)),
                }));
            }
        }
        out.insert(0, json!({ "name": "/", "path": "/" }));
    }

    out.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });
    out
}

/// /proc/self/mounts 的挂载点按字节做八进制转义（\040 = 空格），还原成 UTF-8
#[cfg(all(unix, not(target_os = "macos")))]
fn unescape_mount(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        if b == b'\\' {
            let mut v = 0u32;
            let mut ok = true;
            for _ in 0..3 {
                match it.next() {
                    Some(d @ b'0'..=b'7') => v = v * 8 + u32::from(d - b'0'),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                bytes.push(v as u8);
                continue;
            }
            bytes.push(b);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub async fn list(args: Value) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        // path 缺省 = 家目录（与 CorePaths::default_data 同一套环境变量约定）
        let requested = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Path::new);
        let path = match requested {
            Some(p) => p.to_path_buf(),
            None => Path::new(
                &std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .unwrap_or_else(|_| ".".into()),
            )
            .to_path_buf(),
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
                "path": display_path(&entry.path()),
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
        // 盘符根（C:\）的 parent 为 None，前端用 drives 做「此电脑」向上视图
        let parent = path.parent().map(|p| display_path(&p));
        // 常用目录（桌面/下载/图片/文档）：存在才返回，供选择弹层做快捷入口。
        // 家目录下点开头的系统目录很多，没有快捷入口时真实文件会被挤到列表底部
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let home = Path::new(&home);
        let special = |name: &str| -> Value {
            match home.join(name).canonicalize() {
                Ok(c) if c.is_dir() => json!(display_path(&c)),
                _ => Value::Null,
            }
        };
        Ok(json!({
            "path": display_path(&path),
            "parent": parent,
            "entries": entries,
            "drives": drives(),
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// /Volumes 下至少有指向 / 的系统盘；条目形状完整、路径可访问
    #[test]
    fn drives_includes_root_volume() {
        let ds = drives();
        assert!(ds.iter().any(|d| d["path"] == "/"), "系统盘缺失: {ds:?}");
        for d in ds {
            assert!(d["name"].is_string() && d["path"].is_string(), "条目残缺: {d:?}");
            assert!(d["path"].as_str().unwrap().starts_with('/'));
        }
    }

    /// Unix 路径原样透传（不做分隔符替换）
    #[test]
    fn display_path_keeps_unix_separators() {
        assert_eq!(display_path(Path::new("/Volumes/SSD 1T")), "/Volumes/SSD 1T");
    }
}
