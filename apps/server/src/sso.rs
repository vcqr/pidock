//! SSO：公开的 /auth/methods（登录页渲染用）+ OIDC authorization-code 流程。
//! 手写协议交互（reqwest + 手动表单编码），与 s3.rs 手写 SigV4 同一风格；
//! 回调成功后不直接落 cookie（本项目无 cookie），而是发一次性 exchange code
//! 由前端 POST /auth/sso/exchange 换 token 对，与 localStorage 惯例衔接。

use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::form_urlencoded;
use uuid::Uuid;

use crate::auth::{api_err, issue_pair};
use crate::state::AppState;

fn default_true() -> bool {
    true
}

/// OIDC 登录配置（auth_config 集合 _id="oidc"），管理端动态配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OidcCfg {
    #[serde(default)]
    pub enabled: bool,
    /// IdP issuer，如 https://sso.example.com/realms/pidock
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    /// 空 = "openid email profile"
    #[serde(default)]
    pub scopes: String,
    /// 登录按钮文案，空 = "SSO 登录"
    #[serde(default)]
    pub label: String,
    /// 服务/前端公开基址（空 = 从请求 Host 推断；反代后建议显式配置）
    #[serde(default)]
    pub redirect_base: String,
    /// 首次登录成功自动建号（JIT provisioning）
    #[serde(default = "default_true")]
    pub allow_register: bool,
}

impl OidcCfg {
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let issuer = self.issuer.trim();
        if !(issuer.starts_with("https://") || issuer.starts_with("http://")) {
            return Err("Issuer 必须以 https:// 或 http:// 开头".into());
        }
        if self.client_id.trim().is_empty() || self.client_secret.trim().is_empty() {
            return Err("Client ID 与 Client Secret 不能为空".into());
        }
        Ok(())
    }

    pub fn effective_scopes(&self) -> &str {
        let s = self.scopes.trim();
        if s.is_empty() {
            "openid email profile"
        } else {
            s
        }
    }

    pub fn effective_label(&self) -> &str {
        let l = self.label.trim();
        if l.is_empty() {
            "SSO 登录"
        } else {
            l
        }
    }
}

#[derive(Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

async fn discover(issuer: &str) -> Result<Discovery, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim().trim_end_matches('/')
    );
    let resp = http_client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("拉取 discovery 文档失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "discovery 文档请求失败（HTTP {}）",
            resp.status().as_u16()
        ));
    }
    resp.json::<Discovery>()
        .await
        .map_err(|e| format!("discovery 文档解析失败: {e}"))
}

/// 回调地址：redirect_base 显式配置优先；否则从请求 Host（含反代 X-Forwarded-Proto）推断
pub fn redirect_uri(cfg: &OidcCfg, headers: &HeaderMap) -> String {
    let path = "/auth/sso/oidc/callback";
    if !cfg.redirect_base.trim().is_empty() {
        return format!("{}{path}", cfg.redirect_base.trim().trim_end_matches('/'));
    }
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}{path}")
}

fn frontend_base(cfg: &OidcCfg, headers: &HeaderMap) -> String {
    if !cfg.redirect_base.trim().is_empty() {
        return cfg.redirect_base.trim().trim_end_matches('/').to_string();
    }
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

fn authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &str,
    state: &str,
) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scopes)
        .append_pair("state", state)
        .finish();
    format!("{endpoint}?{query}")
}

/// 登录中途失败：回前端登录页并带 sso_error 参数展示
fn sso_fail(base: &str, msg: &str) -> Response {
    let q = form_urlencoded::Serializer::new(String::new())
        .append_pair("sso_error", msg)
        .finish();
    Redirect::to(&format!("{}/?{}", base.trim_end_matches('/'), q)).into_response()
}

async fn redis_getdel(state: &AppState, key: &str) -> Result<Option<String>, String> {
    let mut conn = state.redis.clone();
    redis::cmd("GETDEL")
        .arg(key)
        .query_async::<Option<String>>(&mut conn)
        .await
        .map_err(|e| e.to_string())
}

// ------------------------------------------------------------ endpoints

/// 公开：登录页据此决定显示哪些登录方式
async fn methods(State(state): State<AppState>) -> Json<Value> {
    let ldap = crate::admin::load_ldap_cfg(&state).await;
    let oidc = crate::admin::load_oidc_cfg(&state).await;
    Json(json!({
        "password": true,
        "ldap": { "enabled": ldap.enabled },
        "oidc": { "enabled": oidc.enabled, "label": oidc.effective_label() },
    }))
}

/// 发起 OIDC 登录：302 到 IdP 的 authorize endpoint
async fn oidc_login(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let cfg = crate::admin::load_oidc_cfg(&state).await;
    if !cfg.enabled {
        return Err(api_err(StatusCode::NOT_FOUND, "SSO 登录未启用"));
    }
    let disc = discover(&cfg.issuer)
        .await
        .map_err(|e| api_err(StatusCode::BAD_GATEWAY, e))?;
    let st = Uuid::now_v7().simple().to_string();
    let mut conn = state.redis.clone();
    redis::cmd("SETEX")
        .arg(format!("sso:oidc:{st}"))
        .arg(600u64)
        .arg("1")
        .query_async::<()>(&mut conn)
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let ru = redirect_uri(&cfg, &headers);
    let url = authorize_url(
        &disc.authorization_endpoint,
        cfg.client_id.trim(),
        &ru,
        cfg.effective_scopes(),
        &st,
    );
    tracing::info!(redirect_uri = %ru, "oidc login initiated");
    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// IdP 回调：验 state → 换 token → userinfo → 匹配/JIT 建号 → 发一次性 exchange code
async fn oidc_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let cfg = crate::admin::load_oidc_cfg(&state).await;
    let base = frontend_base(&cfg, &headers);
    let fail = |msg: String| sso_fail(&base, &msg);

    if let Some(err) = q.error {
        let detail = q.error_description.unwrap_or_else(|| err.clone());
        tracing::warn!(error = %err, "oidc callback: IdP returned error");
        return fail(format!("SSO 登录被取消或失败：{detail}"));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return fail("回调参数缺失（code/state）".into());
    };
    match redis_getdel(&state, &format!("sso:oidc:{st}")).await {
        Ok(Some(_)) => {}
        Ok(None) => return fail("登录会话已过期或 state 校验失败，请重新发起登录".into()),
        Err(e) => return fail(format!("登录会话校验失败：{e}")),
    }

    let disc = match discover(&cfg.issuer).await {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let ru = redirect_uri(&cfg, &headers);
    let form = form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", &code)
        .append_pair("redirect_uri", &ru)
        .append_pair("client_id", cfg.client_id.trim())
        .append_pair("client_secret", cfg.client_secret.trim())
        .finish();
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let token_resp = match client
        .post(&disc.token_endpoint)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return fail(format!("换取令牌失败：{e}")),
    };
    if !token_resp.status().is_success() {
        let status = token_resp.status().as_u16();
        let body = token_resp.text().await.unwrap_or_default();
        tracing::warn!(status, body = %body, "oidc token endpoint error");
        return fail(format!("换取令牌失败（HTTP {status}），请检查 Client 配置与 redirect_uri"));
    }
    let token: Value = match token_resp.json().await {
        Ok(t) => t,
        Err(e) => return fail(format!("令牌响应解析失败：{e}")),
    };
    let access = match token.get("access_token").and_then(|v| v.as_str()) {
        Some(a) => a.to_string(),
        None => return fail("IdP 未返回 access_token".into()),
    };

    let userinfo = match client
        .get(&disc.userinfo_endpoint)
        .bearer_auth(&access)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return fail(format!("拉取用户信息失败：{e}")),
    };
    if !userinfo.status().is_success() {
        let status = userinfo.status().as_u16();
        tracing::warn!(status, "oidc userinfo error");
        return fail(format!("拉取用户信息失败（HTTP {status}）"));
    }
    let ui: Value = match userinfo.json().await {
        Ok(u) => u,
        Err(e) => return fail(format!("用户信息解析失败：{e}")),
    };
    let Some(sub) = ui.get("sub").and_then(|v| v.as_str()).map(str::to_string) else {
        return fail("SSO 用户信息缺少 sub 字段".into());
    };
    let Some(email) = ui
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.contains('@'))
    else {
        return fail("SSO 账号缺少有效邮箱，请检查 IdP 用户资料或 scopes 是否包含 email".into());
    };
    let name = ui
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // sub 加 issuer 前缀：换 IdP 或多 IdP 时互不串号
    let oidc_sub = format!("{}|{}", cfg.issuer.trim().trim_end_matches('/'), sub);
    let (user_id, role) =
        match crate::auth::external_upsert(&state, "oidc", &email, &name, Some(&oidc_sub), cfg.allow_register)
            .await
        {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
    let mut payload = match issue_pair(&state, &user_id).await {
        Ok(p) => p,
        Err(e) => return fail(format!("签发会话失败：{e}")),
    };
    payload["role"] = json!(role);
    payload["email"] = json!(email);

    let exchange_code = Uuid::now_v7().simple().to_string();
    let mut conn = state.redis.clone();
    let set = redis::cmd("SETEX")
        .arg(format!("sso:code:{exchange_code}"))
        .arg(120u64)
        .arg(payload.to_string())
        .query_async::<()>(&mut conn)
        .await;
    if let Err(e) = set {
        return fail(format!("登录会话暂存失败：{e}"));
    }
    Redirect::to(&format!("{}/?sso_code={exchange_code}", base.trim_end_matches('/')))
        .into_response()
}

#[derive(Deserialize)]
struct ExchangeBody {
    code: String,
}

/// 前端用一次性 code 换正式 token 对（code 用后即焚）
async fn sso_exchange(
    State(state): State<AppState>,
    Json(body): Json<ExchangeBody>,
) -> Result<Json<Value>, Response> {
    let code = body.code.trim();
    if code.is_empty() {
        return Err(api_err(StatusCode::BAD_REQUEST, "缺少 code"));
    }
    match redis_getdel(&state, &format!("sso:code:{code}")).await {
        Ok(Some(raw)) => {
            let payload: Value = serde_json::from_str(&raw)
                .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(Json(payload))
        }
        Ok(None) => Err(api_err(
            StatusCode::UNAUTHORIZED,
            "登录会话已过期，请重新登录",
        )),
        Err(e) => Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/methods", get(methods))
        .route("/auth/sso/oidc/login", get(oidc_login))
        .route("/auth/sso/oidc/callback", get(oidc_callback))
        .route("/auth/sso/exchange", post(sso_exchange))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(host: &str, proto: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(p) = proto {
            h.insert("x-forwarded-proto", p.parse().unwrap());
        }
        h
    }

    #[test]
    fn redirect_uri_from_host() {
        let cfg = OidcCfg::default();
        assert_eq!(
            redirect_uri(&cfg, &headers("pidock.example.com", None)),
            "http://pidock.example.com/auth/sso/oidc/callback"
        );
        assert_eq!(
            redirect_uri(&cfg, &headers("pidock.example.com", Some("https"))),
            "https://pidock.example.com/auth/sso/oidc/callback"
        );
    }

    #[test]
    fn redirect_uri_from_config_wins() {
        let cfg = OidcCfg {
            redirect_base: "https://dock.corp.cn/".into(),
            ..Default::default()
        };
        assert_eq!(
            redirect_uri(&cfg, &headers("localhost:8080", None)),
            "https://dock.corp.cn/auth/sso/oidc/callback"
        );
    }

    #[test]
    fn authorize_url_encodes_params() {
        let url = authorize_url(
            "https://idp.example.com/authorize",
            "pidock",
            "https://d.example.com/auth/sso/oidc/callback",
            "openid email profile",
            "state123",
        );
        assert!(url.starts_with("https://idp.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=pidock"));
        assert!(url.contains("state=state123"));
        assert!(!url.contains(' '), "query 必须编码，不能有裸空格");
    }

    #[test]
    fn sso_fail_redirect_encodes_message() {
        let resp = sso_fail("https://d.example.com", "登录失败 & 需重试");
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(loc.starts_with("https://d.example.com/?sso_error="));
        assert!(!loc.contains(" & "), "错误信息必须 URL 编码");
    }

    #[test]
    fn oidc_cfg_validate_and_defaults() {
        let mut cfg = OidcCfg::default();
        assert!(cfg.validate().is_ok(), "未启用不校验");
        cfg.enabled = true;
        assert!(cfg.validate().is_err());
        cfg.issuer = "https://sso.example.com/realms/x".into();
        assert!(cfg.validate().is_err(), "缺 client");
        cfg.client_id = "pidock".into();
        cfg.client_secret = "s3cret".into();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.effective_scopes(), "openid email profile");
        assert_eq!(cfg.effective_label(), "SSO 登录");
        cfg.label = "企业账号登录".into();
        assert_eq!(cfg.effective_label(), "企业账号登录");
    }
}
