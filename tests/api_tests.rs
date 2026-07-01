//! REST API 集成测试
//!
//! 使用 axum 的 ServiceExt 进行 HTTP 级别的测试。

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

use rustminidb::server::build_routes;
use rustminidb::server::error::AppState;
use rustminidb::sql::executor::Executor;
use rustminidb::storage::redb_engine::RedbEngine;

/// 创建测试应用
fn setup_test_app() -> (TempDir, Router) {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(RedbEngine::open(dir.path().join("test.db")).unwrap());
    let executor = Executor::new(engine.clone());
    let state = AppState::from_engine(engine, Arc::new(executor), None);
    let app = build_routes(state);
    (dir, app)
}

#[tokio::test]
async fn test_health_endpoint() {
    let (_dir, app) = setup_test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_create_table_and_query() {
    let (_dir, app) = setup_test_app();

    // 创建表
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/query")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "sql": "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // 插入数据
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/query")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "sql": "INSERT INTO users VALUES (1, 'Alice', 30)"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // 查询数据
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/query")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "sql": "SELECT * FROM users"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["columns"], json!(["id", "name", "age"]));
    assert_eq!(body["data"]["rows_affected"], 1);
}

#[tokio::test]
async fn test_list_tables() {
    let (_dir, app) = setup_test_app();

    // 先创建一张表
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/query")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "sql": "CREATE TABLE test_table (id INT PRIMARY KEY, val TEXT)"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await;

    // 获取表列表
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/tables")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_parse_error_response() {
    let (_dir, app) = setup_test_app();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/query")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "sql": "SLECT * FROME users"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"]["code"].as_str().unwrap().contains("PARSE"));
}
