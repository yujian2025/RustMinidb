//! REST API 服务器模块
//!
//! 基于 axum 框架，提供 REST API 接口。

pub mod auth;
pub mod error;
pub mod handlers;
pub mod routes;

pub use error::AppState;
pub use routes::build_routes;
