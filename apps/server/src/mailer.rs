//! SMTP 邮件发送：登录邮箱验证码等（lettre + rustls，配置 [email] 节）。
//! 只配 host 即视为可用；from 留空回落 smtp_user。

use crate::config::EmailCfg;

/// SMTP 是否已配置（host 非空）。login_code 开关另见 otp 模块。
pub fn smtp_ready(cfg: &EmailCfg) -> bool {
    !cfg.smtp_host.trim().is_empty()
}

/// 登录邮箱验证码总开关：显式开关 + SMTP 就绪，二者缺一即关闭
pub fn login_code_enabled(cfg: &EmailCfg) -> bool {
    cfg.login_code && smtp_ready(cfg)
}

/// 注册邮箱验证码总开关：语义同登录码
pub fn register_code_enabled(cfg: &EmailCfg) -> bool {
    cfg.register_code && smtp_ready(cfg)
}

fn transport(cfg: &EmailCfg) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, String> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::AsyncSmtpTransport;

    let host = cfg.smtp_host.trim();
    // 异步 transport 的 relay/starttls_relay 返回 builder，需再 .build()
    let mut b = match cfg.smtp_tls.trim() {
        "starttls" => AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(host),
        // 明文只该出现在内网调试；生产请用 tls / starttls
        "none" => Ok(AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(host)),
        _ => AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(host),
    }
    .map_err(|e| format!("SMTP 连接配置无效（{host}）: {e}"))?;
    if cfg.smtp_port > 0 {
        b = b.port(cfg.smtp_port);
    }
    if !cfg.smtp_user.trim().is_empty() {
        b = b.credentials(Credentials::new(
            cfg.smtp_user.trim().to_string(),
            cfg.smtp_pass.clone(),
        ));
    }
    Ok(b.build())
}

/// 发送登录验证码邮件（纯文本中文）；ttl_min 用于正文里的有效期提示
pub async fn send_login_code(cfg: &EmailCfg, to: &str, code: &str, ttl_min: i64) -> Result<(), String> {
    use lettre::{AsyncTransport, Message};

    let from_addr = if cfg.from.trim().is_empty() {
        cfg.smtp_user.trim()
    } else {
        cfg.from.trim()
    };
    if from_addr.is_empty() {
        return Err("未配置发件地址（email.from 或 email.smtp_user）".into());
    }
    let msg = Message::builder()
        .from(from_addr.parse().map_err(|e| format!("发件地址无效: {e}"))?)
        .to(to.parse().map_err(|e| format!("收件地址无效: {e}"))?)
        .subject("PiDock 登录验证码")
        .body(format!(
            "你的 PiDock 登录验证码是：{code}\r\n\r\n验证码 {ttl_min} 分钟内有效，请勿泄露给他人。\r\n如非本人操作，请忽略本邮件并尽快修改密码。"
        ))
        .map_err(|e| format!("邮件构建失败: {e}"))?;
    transport(cfg)?
        .send(msg)
        .await
        .map_err(|e| format!("邮件发送失败: {e}"))?;
    Ok(())
}
