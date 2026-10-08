//! LDAP authentication：服务账号 bind → 按 user_filter 搜索用户条目 →
//! 用条目 DN + 登录密码做 simple bind 验证。配置存 auth_config（_id="ldap"），
//! 由管理端动态修改，登录时现读现用。

use std::time::Duration;

use ldap3::{LdapConnAsync, LdapConnSettings, LdapError, Scope, SearchEntry};
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// LDAP 登录配置（auth_config 集合 _id="ldap"）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapCfg {
    #[serde(default)]
    pub enabled: bool,
    /// ldap://host:389 或 ldaps://host:636
    #[serde(default)]
    pub server_url: String,
    /// 服务账号 DN（空 = 匿名搜索）
    #[serde(default)]
    pub bind_dn: String,
    #[serde(default)]
    pub bind_password: String,
    #[serde(default)]
    pub base_dn: String,
    /// 用户搜索过滤器，{username} 替换为登录输入
    #[serde(default)]
    pub user_filter: String,
    /// ldap:// 地址是否升级 StartTLS
    #[serde(default)]
    pub starttls: bool,
    /// 邮箱属性名
    #[serde(default)]
    pub email_attr: String,
    /// 姓名属性名
    #[serde(default)]
    pub name_attr: String,
    /// 首次登录成功自动建号（JIT provisioning）
    #[serde(default = "default_true")]
    pub allow_register: bool,
}

impl Default for LdapCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: String::new(),
            bind_dn: String::new(),
            bind_password: String::new(),
            base_dn: String::new(),
            user_filter: "(uid={username})".into(),
            starttls: false,
            email_attr: "mail".into(),
            name_attr: "cn".into(),
            allow_register: true,
        }
    }
}

impl LdapCfg {
    /// 管理端保存前的校验（enabled 时才要求完整）
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if !(self.server_url.trim().starts_with("ldap://")
            || self.server_url.trim().starts_with("ldaps://"))
        {
            return Err("LDAP 服务器地址必须以 ldap:// 或 ldaps:// 开头".into());
        }
        if self.base_dn.trim().is_empty() {
            return Err("搜索 Base DN 不能为空".into());
        }
        if !self.user_filter.contains("{username}") {
            return Err("用户过滤器必须包含 {username} 占位符".into());
        }
        Ok(())
    }

    fn settings(&self) -> LdapConnSettings {
        LdapConnSettings::new()
            .set_starttls(self.starttls)
            .set_conn_timeout(Duration::from_secs(5))
    }
}

/// {username} 占位符替换（其余内容原样交给 LDAP 服务器解析）
pub fn render_filter(filter: &str, username: &str) -> String {
    filter.replace("{username}", username)
}

/// 认证成功后从条目取出的用户信息
pub struct LdapUser {
    pub email: String,
    pub name: String,
}

/// 对外错误分类：凭据问题统一 401，服务故障 502，不向客户端泄露细节
pub enum LdapAuthError {
    Credentials,
    Unavailable(String),
}

/// 完整登录流程。返回的 email 已小写化（作为本地账号唯一键）。
pub async fn authenticate(
    cfg: &LdapCfg,
    username: &str,
    password: &str,
) -> Result<LdapUser, LdapAuthError> {
    if username.trim().is_empty() || password.is_empty() {
        return Err(LdapAuthError::Credentials);
    }
    let (conn, mut ldap) = LdapConnAsync::with_settings(cfg.settings(), cfg.server_url.trim())
        .await
        .map_err(unavail("LDAP 连接失败"))?;
    ldap3::drive!(conn);
    let r = auth_on(&mut ldap, cfg, username, password).await;
    let _ = ldap.unbind().await;
    r
}

fn unavail(what: &'static str) -> impl Fn(LdapError) -> LdapAuthError {
    move |e| LdapAuthError::Unavailable(format!("{what}: {e}"))
}

async fn auth_on(
    ldap: &mut ldap3::Ldap,
    cfg: &LdapCfg,
    username: &str,
    password: &str,
) -> Result<LdapUser, LdapAuthError> {
    if !cfg.bind_dn.trim().is_empty() {
        let res = ldap
            .simple_bind(cfg.bind_dn.trim(), &cfg.bind_password)
            .await
            .map_err(unavail("服务账号 bind 失败"))?;
        if res.rc != 0 {
            return Err(LdapAuthError::Unavailable(format!(
                "服务账号 bind 被拒（rc={}）",
                res.rc
            )));
        }
    }
    let filter = render_filter(cfg.user_filter.trim(), username.trim());
    let attrs = [
        cfg.email_attr.trim().to_string(),
        cfg.name_attr.trim().to_string(),
    ];
    let (rs, _) = ldap
        .search(cfg.base_dn.trim(), Scope::Subtree, &filter, &attrs)
        .await
        .map_err(unavail("用户搜索失败"))?
        .success()
        .map_err(unavail("用户搜索失败"))?;
    let found = rs
        .into_iter()
        .next()
        .ok_or(LdapAuthError::Credentials)?;
    let entry = SearchEntry::construct(found);

    // 用条目 DN + 登录密码验证（覆盖服务账号 bind，即 LDAP 协议的重新认证）
    let res = ldap
        .simple_bind(&entry.dn, password)
        .await
        .map_err(unavail("用户验证失败"))?;
    if res.rc == 49 {
        return Err(LdapAuthError::Credentials);
    }
    if res.rc != 0 {
        return Err(LdapAuthError::Unavailable(format!(
            "用户验证失败（rc={}）",
            res.rc
        )));
    }

    let email = entry
        .attrs
        .get(cfg.email_attr.trim())
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_else(|| format!("{}@ldap", username.trim()));
    let name = entry
        .attrs
        .get(cfg.name_attr.trim())
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_default();
    Ok(LdapUser {
        email: email.to_lowercase(),
        name,
    })
}

/// 管理端「测试连接」：连服务、搜用户、（可选）试绑定，返回诊断文案。
pub async fn test(cfg: &LdapCfg, username: &str, password: &str) -> Result<String, String> {
    let (conn, mut ldap) = LdapConnAsync::with_settings(cfg.settings(), cfg.server_url.trim())
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    ldap3::drive!(conn);
    let r = test_on(&mut ldap, cfg, username, password).await;
    let _ = ldap.unbind().await;
    r
}

async fn test_on(
    ldap: &mut ldap3::Ldap,
    cfg: &LdapCfg,
    username: &str,
    password: &str,
) -> Result<String, String> {
    if !cfg.bind_dn.trim().is_empty() {
        let res = ldap
            .simple_bind(cfg.bind_dn.trim(), &cfg.bind_password)
            .await
            .map_err(|e| format!("服务账号 bind 失败: {e}"))?;
        if res.rc != 0 {
            return Err(format!("服务账号 bind 被拒（rc={}）", res.rc));
        }
    }
    let filter = if username.trim().is_empty() {
        "(objectClass=*)".to_string()
    } else {
        render_filter(cfg.user_filter.trim(), username.trim())
    };
    let attrs = [
        cfg.email_attr.trim().to_string(),
        cfg.name_attr.trim().to_string(),
    ];
    let (rs, _) = ldap
        .search(cfg.base_dn.trim(), Scope::Subtree, &filter, &attrs)
        .await
        .map_err(|e| format!("搜索失败: {e}"))?
        .success()
        .map_err(|e| format!("搜索失败: {e}"))?;
    if rs.is_empty() {
        return Ok("连接与搜索正常，但没有匹配到任何用户条目".into());
    }
    let entry = SearchEntry::construct(rs[0].clone());
    let mut msg = format!(
        "连接与搜索正常，匹配到 {} 个条目（第一个: {}）",
        rs.len(),
        entry.dn
    );
    if !username.trim().is_empty() && !password.is_empty() {
        let res = ldap
            .simple_bind(&entry.dn, password)
            .await
            .map_err(|e| format!("；用户绑定失败: {e}"))?;
        if res.rc == 0 {
            msg.push_str("；用户密码验证通过 ✓");
        } else {
            return Err(format!("{msg}；用户密码验证被拒（rc={}）", res.rc));
        }
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_placeholder_replaced() {
        assert_eq!(
            render_filter("(uid={username})", "bob"),
            "(uid=bob)"
        );
        assert_eq!(
            render_filter("(&(objectClass=person)(mail={username}))", "a@b.c"),
            "(&(objectClass=person)(mail=a@b.c))"
        );
    }

    #[test]
    fn validate_requires_placeholder_and_url() {
        let mut cfg = LdapCfg::default();
        assert!(cfg.validate().is_ok(), "未启用不校验");
        cfg.enabled = true;
        assert!(cfg.validate().is_err(), "缺 server_url");
        cfg.server_url = "ftp://x".into();
        assert!(cfg.validate().is_err());
        cfg.server_url = "ldaps://ldap.example.com:636".into();
        assert!(cfg.validate().is_err(), "缺 base_dn");
        cfg.base_dn = "dc=example,dc=com".into();
        cfg.user_filter = "(objectClass=*)".into();
        assert!(cfg.validate().is_err(), "过滤器缺占位符应被拒");
        cfg.user_filter = "(mail={username})".into();
        assert!(cfg.validate().is_ok());
    }
}
