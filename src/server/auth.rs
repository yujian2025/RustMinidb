//! API 认证中间件
//!
//! 提供 Bearer Token 认证保护，防止未授权访问。
//! 公开端点（/、/v1/health）不受此限制。

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use crate::server::error::APP_TOKEN;

/// 可公开访问的路径前缀列表（不校验 Token）
const PUBLIC_PATHS: &[&str] = &["/", "/v1/health"];

/// 认证守卫中间件
///
/// 如果服务未配置 `api_token`，则直接放行所有请求。
/// 如果配置了 `api_token`，则校验 `Authorization: Bearer <token>` 请求头。
pub async fn guard(req: Request, next: Next) -> Result<Response, StatusCode> {
    // 1. 获取全局 Token
    let expected_token = APP_TOKEN.get().map(|s| s.as_str());

    // 2. 如果未配置 Token，直接放行（兼容开发模式）
    let Some(expected_token) = expected_token else {
        return Ok(next.run(req).await);
    };

    // 3. 公开路径直接放行
    let path = req.uri().path().to_string();
    if PUBLIC_PATHS.contains(&path.as_str()) {
        return Ok(next.run(req).await);
    }

    // 4. 校验 Authorization 请求头
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {}", expected_token);
    if auth_header == expected {
        return Ok(next.run(req).await);
    }

    // 5. 认证失败 → 401
    Err(StatusCode::UNAUTHORIZED)
}
