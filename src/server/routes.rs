//! API 路由定义

use axum::{
    middleware,
    response::Html,
    routing::{get, post},
    Router,
};
use tower_http::cors::CorsLayer;

use super::auth;
use super::error::AppState;
use super::handlers;

/// 管理页面 HTML（编译时嵌入）
const ADMIN_HTML: &str = include_str!("admin.html");

/// 服务管理页面
async fn admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

/// 构建所有 API 路由
pub fn build_routes(state: AppState) -> Router {
    // ── 公开路由（无需认证） ──
    let public = Router::new()
        .route("/", get(admin_page))
        .route("/v1/health", get(handlers::health_check));

    // ── 受保护路由（需要 Bearer Token 认证） ──
    let protected = Router::new()
        .route("/v1/query", post(handlers::execute_query))
        .route("/v1/tables", get(handlers::list_tables))
        .route("/v1/schema/{table}", get(handlers::get_schema))
        .route("/v1/import", post(handlers::import_data))
        .route("/v1/databases", get(handlers::list_databases))
        .route("/v1/databases/switch", post(handlers::switch_database))
        .route("/v1/databases/create", post(handlers::create_database))
        .route("/v1/databases/delete", post(handlers::delete_database))
        .route("/v1/comment", post(handlers::set_comment))
        .route("/v1/export", get(handlers::export_database))
        .route("/v1/metrics", get(handlers::get_metrics))
        .layer(middleware::from_fn(auth::guard));

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
