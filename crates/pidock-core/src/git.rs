//! git 集成：项目目录的仓库检测、分支信息与切换。
//!
//! 走系统 `git` CLI（不引入 libgit2 重依赖），命令面：
//! - `git_info { cwd }` → `{ is_repo, branch, root, detached }`
//! - `git_branches { cwd }` → `{ current, branches: [{ name, current }] }`
//! - `git_checkout { cwd, branch, create? }` → 检出（create = 创建并检出）
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

/// 分支名防选项注入（以 `-` 开头会被 git 当 flag）；其余规则交给 git 自身校验
fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.ends_with(".lock")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 运行 git 子命令，成功返回 trim 后的 stdout，失败带 stderr
fn run(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("无法启动 git（请确认已安装）: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("git {} 失败", args.join(" "))
        } else {
            err
        })
    }
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
