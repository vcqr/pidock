//! git 集成：项目目录的仓库检测、分支信息与切换、提交历史。
//!
//! 走系统 `git` CLI（不引入 libgit2 重依赖），命令面：
//! - `git_info { cwd }` → `{ is_repo, branch, root, detached }`
//! - `git_branches { cwd }` → `{ current, branches: [{ name, current }] }`
//! - `git_checkout { cwd, branch, create? }` → 检出（create = 创建并检出）
//! - `git_log { cwd, limit?, skip? }` → `{ commits: [{ hash, short, author, time, subject, refs }] }`
//! - `git_commit_files { cwd, hash }` → `{ files: [{ path, status, added, removed }] }`
//! - `git_commit_diff { cwd, hash, path }` → `{ old_text, new_text, binary }`
//!
//! 三个命令都在 [`CoreCtx::handle`] 分发，桌面与 webhost 壳共用。

use serde_json::{json, Value};

/// 从命令参数取项目目录
fn cwd_of(args: &Value) -> Result<String, String> {
    let cwd = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .ok_or("缺少 cwd 参数")?
        .trim()
        .to_string();
    if cwd.is_empty() {
        return Err("cwd 为空".into());
    }
    Ok(cwd)
}

/// 校验提交 hash：只接受十六进制串（防选项注入；长度上限覆盖 SHA-1/SHA-256）
fn hash_of(args: &Value) -> Result<String, String> {
    let h = args
        .get("hash")
        .and_then(|v| v.as_str())
        .ok_or("缺少 hash 参数")?
        .trim()
        .to_string();
    let ok = !h.is_empty() && h.len() <= 64 && h.chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return Err(format!("无效的提交 hash: {h}"));
    }
    Ok(h)
}

/// 分支名防选项注入（以 `-` 开头会被 git 当 flag）；其余规则交给 git 自身校验
fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.ends_with(".lock")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 运行 git 子命令，成功返回原样 stdout（不 trim；内容读取需保留末尾换行），失败带 stderr
fn run_raw(cwd: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(cwd).args(args);
    // 主程序是 GUI 子系统（无控制台），console 子进程会被 Windows 分配新终端
    // 窗口（默认终端为 Windows Terminal 时弹 WT 窗口）一闪而过；CREATE_NO_WINDOW
    // 使其无窗运行（piped IO 不受影响）
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 git（请确认已安装）: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("git {} 失败", args.join(" "))
        } else {
            err
        })
    }
}

/// 运行 git 子命令，成功返回 trim 后的 stdout，失败带 stderr
fn run(cwd: &str, args: &[&str]) -> Result<String, String> {
    run_raw(cwd, args).map(|s| s.trim().to_string())
}

/// 当前分支名：`branch --show-current` 在旧版 git 的未出生分支（还没有提交）上返回空，
/// 依次回退 symbolic-ref（未出生分支）与短 sha（detached HEAD）
fn current_branch(cwd: &str) -> (Option<String>, bool) {
    if let Ok(b) = run(cwd, &["branch", "--show-current"]) {
        if !b.is_empty() {
            return (Some(b), false);
        }
    }
    if let Ok(b) = run(cwd, &["symbolic-ref", "--short", "HEAD"]) {
        if !b.is_empty() {
            return (Some(b), false);
        }
    }
    match run(cwd, &["rev-parse", "--short", "HEAD"]) {
        Ok(sha) if !sha.is_empty() => (Some(sha), true),
        _ => (None, false),
    }
}

pub async fn info(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        // 非仓库（或 git 不可用）一律按「无 git 信息」处理，不算错误
        let inside = run(&cwd, &["rev-parse", "--is-inside-work-tree"]).unwrap_or_default();
        if inside != "true" {
            return Ok(json!({ "is_repo": false }));
        }
        let root = run(&cwd, &["rev-parse", "--show-toplevel"]).ok();
        let (branch, detached) = current_branch(&cwd);
        Ok(json!({ "is_repo": true, "branch": branch, "root": root, "detached": detached }))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn branches(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let (current, _) = current_branch(&cwd);
        let out = run(&cwd, &["for-each-ref", "refs/heads", "--format=%(refname:short)"])?;
        let mut list: Vec<Value> = out
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|name| json!({ "name": name, "current": current.as_deref() == Some(name) }))
            .collect();
        // 当前分支排最前，其余按名称排序
        list.sort_by(|a, b| {
            let ca = a["current"].as_bool().unwrap_or(false);
            let cb = b["current"].as_bool().unwrap_or(false);
            cb.cmp(&ca).then_with(|| {
                a["name"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["name"].as_str().unwrap_or(""))
            })
        });
        Ok(json!({ "current": current, "branches": list }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 提交历史（HEAD 所在分支，新→旧）。未出生分支（还没有任何提交）不算错误，返回空列表；
/// limit 默认 200 上限 500，skip 供「加载更多」分页。
pub async fn log(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .clamp(1, 500) as usize;
    let skip = args.get("skip").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        // 记分隔 \x1e、字段分隔 \x1f，避免 subject 里的分隔符歧义；%D 为修饰（HEAD -> main, tag: …）
        let pretty = "%H%x1f%h%x1f%an%x1f%at%x1f%s%x1f%D%x1e";
        let out = match run(
            &cwd,
            &[
                "log",
                &format!("--max-count={limit}"),
                &format!("--skip={skip}"),
                &format!("--pretty=format:{pretty}"),
            ],
        ) {
            Ok(out) => out,
            Err(e) if e.contains("does not have any commits yet") || e.contains("bad revision") => {
                return Ok(json!({ "commits": [] }));
            }
            Err(e) => return Err(e),
        };
        let commits: Vec<Value> = out
            .split('\x1e')
            .map(str::trim)
            .filter(|rec| !rec.is_empty())
            .map(|rec| {
                let f: Vec<&str> = rec.split('\x1f').collect();
                let refs: Vec<String> = f
                    .get(5)
                    .map(|d| {
                        d.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "hash": f.first().copied().unwrap_or(""),
                    "short": f.get(1).copied().unwrap_or(""),
                    "author": f.get(2).copied().unwrap_or(""),
                    "time": f.get(3).and_then(|t| t.parse::<i64>().ok()).unwrap_or(0),
                    "subject": f.get(4).copied().unwrap_or(""),
                    "refs": refs,
                })
            })
            .collect();
        Ok(json!({ "commits": commits }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 提交的变更文件清单。name-status 取状态与路径（-z 防路径特殊字符），
/// numstat 配对 +/- 行数（二进制为 "-"，记 -1 前端不显示）。
/// --no-renames：两侧路径一一对应，重命名呈现为「旧路径 D + 新路径 A」
pub async fn commit_files(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    let hash = hash_of(&args)?;
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let st = run(
            &cwd,
            &["show", "-z", "--no-renames", "--name-status", "--pretty=format:", &hash],
        )?;
        // quotePath=false：numstat 未走 -z，关掉非 ASCII 路径的八进制转义，才能与 -z 侧原始路径配对
        let num = run(
            &cwd,
            &["-c", "core.quotePath=false", "show", "--no-renames", "--numstat", "--pretty=format:", &hash],
        )?;
        let mut stats: std::collections::HashMap<&str, (i64, i64)> = std::collections::HashMap::new();
        for line in num.lines() {
            let mut it = line.splitn(3, '\t');
            if let (Some(a), Some(r), Some(p)) = (it.next(), it.next(), it.next()) {
                stats.insert(p, (a.parse().unwrap_or(-1), r.parse().unwrap_or(-1)));
            }
        }
        // -z 记录为 status\0path\0（--no-renames 下恒单路径）；空消息产生的空 token 由过滤处理
        let mut toks = st.split('\0').filter(|t| !t.is_empty());
        let mut files: Vec<Value> = Vec::new();
        while let (Some(status), Some(path)) = (toks.next(), toks.next()) {
            let (added, removed) = stats.get(path).copied().unwrap_or((-1, -1));
            files.push(json!({ "path": path, "status": status, "added": added, "removed": removed }));
        }
        Ok(json!({ "files": files }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 提交中单个文件的新旧内容快照（喂审查面板的 oldText/newText）。
/// 新增文件旧侧为空（根提交同理），删除文件新侧为空；任一侧含 NUL 视为二进制，
/// 超 1MB 拒绝（CodeMirror 渲染大文本会卡死界面）
pub async fn commit_diff(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    let hash = hash_of(&args)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("缺少 path 参数")?
        .trim()
        .to_string();
    if path.is_empty() {
        return Err("path 为空".into());
    }
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        // <hash>~ 为父提交；父版本不存在时 git show 报错，按旧内容为空处理。
        // 用 run_raw 保留文件末尾换行（trim 会让 diff 尾行误报「无换行符」）
        let old = run_raw(&cwd, &["show", &format!("{hash}~:{path}")]).unwrap_or_default();
        let new = run_raw(&cwd, &["show", &format!("{hash}:{path}")]).unwrap_or_default();
        let binary = old.contains('\0') || new.contains('\0');
        const MAX: usize = 1_000_000;
        if old.len() > MAX || new.len() > MAX {
            return Err(format!("文件过大（超过 1MB），无法在审查面板查看: {path}"));
        }
        Ok(json!({
            "old_text": if binary { String::new() } else { old },
            "new_text": if binary { String::new() } else { new },
            "binary": binary,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn checkout(args: Value) -> Result<Value, String> {
    let cwd = cwd_of(&args)?;
    let branch = args
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("git_checkout 缺少 branch")?
        .to_string();
    let create = args.get("create").and_then(|v| v.as_bool()).unwrap_or(false);
    if !valid_branch(&branch) {
        return Err(format!("无效的分支名: {branch}"));
    }
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        if create {
            run(&cwd, &["checkout", "-b", &branch])?;
        } else {
            run(&cwd, &["checkout", &branch])?;
        }
        Ok(json!({ "ok": true, "branch": branch }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时 git 仓库：c1 建 a.txt，c2 改 a.txt + 加 b.txt（含 NUL 的 c.bin），c3 删 a.txt。
    /// 返回 (路径, [c1, c2, c3] 全 hash)
    fn setup_repo() -> (String, Vec<String>) {
        let dir = std::env::temp_dir().join(format!(
            "pidock-git-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap().to_string();
        let run = |args: &[&str]| run(&d, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        run(&["init"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "line1\nline2\n").unwrap();
        std::fs::write(dir.join("c.bin"), b"\x00\x01bin").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "c1"]);
        std::fs::write(dir.join("a.txt"), "line1\nline2 changed\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "c2"]);
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "c3"]);
        let hashes: Vec<String> = ["HEAD~2", "HEAD~1", "HEAD"]
            .iter()
            .map(|r| run(&["rev-parse", r]))
            .collect();
        (d, hashes)
    }

    #[tokio::test]
    async fn commit_files_lists_status_and_numstat() {
        let (d, hs) = setup_repo();
        // c1：a.txt 新增 2 行，c.bin 二进制（numstat 为 "-" → -1）
        let res = commit_files(json!({ "cwd": d, "hash": hs[0] })).await.unwrap();
        let files = res["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        let find = |h: &Value, p: &str| {
            h["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["path"] == p)
                .unwrap()
                .clone()
        };
        let a = find(&res, "a.txt");
        assert_eq!(a["status"], "A");
        assert_eq!(a["added"], 2);
        assert_eq!(a["removed"], 0);
        let bin = find(&res, "c.bin");
        assert_eq!(bin["status"], "A");
        assert_eq!(bin["added"], -1);
        // c2：a.txt 改 1/1 行，b.txt 新增 1 行（c.bin 未动不出现在清单里）
        let res = commit_files(json!({ "cwd": d, "hash": hs[1] })).await.unwrap();
        assert_eq!(res["files"].as_array().unwrap().len(), 2);
        let a = find(&res, "a.txt");
        assert_eq!(a["status"], "M");
        assert_eq!(a["added"], 1);
        assert_eq!(a["removed"], 1);
        let b = find(&res, "b.txt");
        assert_eq!(b["status"], "A");
        assert_eq!(b["added"], 1);
        // c3：a.txt 删除
        let res = commit_files(json!({ "cwd": d, "hash": hs[2] })).await.unwrap();
        let files = res["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["status"], "D");
        assert_eq!(files[0]["path"], "a.txt");
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn commit_diff_returns_both_snapshots() {
        let (d, hs) = setup_repo();
        // 修改文件：两侧都有内容
        let res = commit_diff(json!({ "cwd": d, "hash": hs[1], "path": "a.txt" })).await.unwrap();
        assert_eq!(res["binary"], false);
        assert_eq!(res["old_text"], "line1\nline2\n");
        assert_eq!(res["new_text"], "line1\nline2 changed\n");
        // 新增文件：旧侧为空
        let res = commit_diff(json!({ "cwd": d, "hash": hs[1], "path": "b.txt" })).await.unwrap();
        assert_eq!(res["old_text"], "");
        assert_eq!(res["new_text"], "new\n");
        // 删除文件：新侧为空
        let res = commit_diff(json!({ "cwd": d, "hash": hs[2], "path": "a.txt" })).await.unwrap();
        assert_eq!(res["old_text"], "line1\nline2 changed\n");
        assert_eq!(res["new_text"], "");
        // 二进制文件：文本清空，binary 标记
        let res = commit_diff(json!({ "cwd": d, "hash": hs[1], "path": "c.bin" })).await.unwrap();
        assert_eq!(res["binary"], true);
        assert_eq!(res["old_text"], "");
        assert_eq!(res["new_text"], "");
        std::fs::remove_dir_all(&d).ok();
    }

    #[tokio::test]
    async fn commit_commands_reject_bad_hash() {
        let (d, _) = setup_repo();
        assert!(commit_files(json!({ "cwd": d, "hash": "-ofoo" })).await.is_err());
        assert!(commit_diff(json!({ "cwd": d, "hash": "xyz;rm", "path": "a.txt" })).await.is_err());
        assert!(commit_diff(json!({ "cwd": d, "hash": "0a1b2c", "path": "" })).await.is_err());
        std::fs::remove_dir_all(&d).ok();
    }
}
