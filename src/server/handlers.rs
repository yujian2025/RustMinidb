//! API 请求处理器

use std::time::Instant;

use axum::{
    extract::{Json, Path, State},
};
use serde::{Deserialize, Serialize};

use super::error::AppState;
use crate::error::RustMinidbError;
use crate::sql::executor::ExecuteResult;
use crate::sql::parser::SqlParser;

// ── 请求/响应类型 ──

/// SQL 查询请求
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    5000
}

/// 统一 API 响应
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<QueryResultData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResultData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub rows_affected: usize,
    pub time_ms: f64,
}

#[derive(Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

/// 导入请求
#[derive(Deserialize)]
pub struct ImportRequest {
    pub table: String,
    pub data: Vec<serde_json::Value>,
}

impl ApiResponse {
    pub fn success(data: QueryResultData) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(code: &str, message: &str) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(ApiError {
                code: code.to_string(),
                message: message.to_string(),
            }),
        }
    }
}

/// 健康检查
pub async fn health_check(
    State(state): State<AppState>,
) -> Json<ApiResponse> {
    let uptime = state.uptime_secs();
    let db = state.db();
    let current_db = db.current_db.clone();

    // 获取表数量
    let table_count = match db.db_instance.engine.list_tables() {
        Ok(tables) => tables.len(),
        Err(_) => 0,
    };

    Json(ApiResponse::success(QueryResultData {
        columns: vec![
            "status".into(),
            "version".into(),
            "uptime_secs".into(),
            "tables".into(),
            "database".into(),
        ],
        rows: vec![vec![
            serde_json::Value::String("ok".into()),
            serde_json::Value::String(crate::version().into()),
            serde_json::Value::Number(uptime.into()),
            serde_json::Value::Number(table_count.into()),
            serde_json::Value::String(current_db),
        ]],
        rows_affected: 1,
        time_ms: 0.0,
    }))
}

/// 列出所有表
pub async fn list_tables(
    State(state): State<AppState>,
) -> Json<ApiResponse> {
    let db = state.db();
    match db.db_instance.engine.list_tables() {
        Ok(tables) => {
            let json_tables: Vec<serde_json::Value> = tables
                .iter()
                .map(|t| serde_json::Value::String(t.clone()))
                .collect();

            Json(ApiResponse::success(QueryResultData {
                columns: vec!["table_name".into()],
                rows: json_tables
                    .into_iter()
                    .map(|t| vec![t])
                    .collect(),
                rows_affected: 0,
                time_ms: 0.0,
            }))
        }
        Err(e) => Json(ApiResponse::error("INTERNAL_ERROR", &e.to_string())),
    }
}

/// 获取表结构
pub async fn get_schema(
    State(state): State<AppState>,
    Path(table): Path<String>,
) -> Json<ApiResponse> {
    let db = state.db();
    match db.db_instance.engine.get_schema(&table) {
        Ok(Some(schema)) => {
            let mut rows = Vec::new();
            let _table_comment = schema.comment.clone().unwrap_or_default();
            for col in &schema.columns {
                rows.push(vec![
                    serde_json::Value::String(col.name.clone()),
                    serde_json::Value::String(col.col_type.to_string()),
                    serde_json::Value::Bool(col.nullable),
                    serde_json::Value::Bool(col.is_primary_key),
                    serde_json::Value::String(col.comment.clone().unwrap_or_default()),
                ]);
            }
            let rows_len = rows.len();

            Json(ApiResponse::success(QueryResultData {
                columns: vec![
                    "column_name".into(),
                    "type".into(),
                    "nullable".into(),
                    "primary_key".into(),
                    "comment".into(),
                ],
                rows,
                rows_affected: rows_len,
                time_ms: 0.0,
            }))
        }
        Ok(None) => Json(ApiResponse::error("TABLE_NOT_FOUND", &format!("表 '{}' 不存在", table))),
        Err(e) => Json(ApiResponse::error("INTERNAL_ERROR", &e.to_string())),
    }
}

/// 主查询处理器
pub async fn execute_query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Json<ApiResponse> {
    let start = Instant::now();

    // 1. 解析 SQL
    let stmt = match SqlParser::parse(&req.sql) {
        Ok(stmt) => stmt,
        Err(e) => {
            return Json(ApiResponse::error("PARSE_ERROR", &e.to_string()));
        }
    };

    // 2. 执行
    let db = state.db();
    let result = match db.db_instance.executor.execute(&stmt) {
        Ok(r) => r,
        Err(e) => {
            // 将错误映射到合适的错误码
            let code = error_code(&e);
            return Json(ApiResponse::error(code, &e.to_string()));
        }
    };

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    // 3. 格式化响应
    match result {
        ExecuteResult::QueryResult {
            columns,
            rows,
            rows_affected,
        } => {
            let json_rows: Vec<Vec<serde_json::Value>> = rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| v.to_json_value()).collect())
                .collect();

            Json(ApiResponse::success(QueryResultData {
                columns,
                rows: json_rows,
                rows_affected,
                time_ms: elapsed,
            }))
        }
        ExecuteResult::WriteResult {
            rows_affected,
            last_insert_id,
        } => {
            let mut data = QueryResultData {
                columns: vec![],
                rows: vec![],
                rows_affected,
                time_ms: elapsed,
            };
            if let Some(id) = last_insert_id {
                data.columns = vec!["last_insert_id".to_string()];
                data.rows = vec![vec![serde_json::Value::Number(id.into())]];
            }
            Json(ApiResponse::success(data))
        }
    }
}

/// 数据导入
pub async fn import_data(
    State(state): State<AppState>,
    Json(req): Json<ImportRequest>,
) -> Json<ApiResponse> {
    let start = Instant::now();

    // 1. 获取表 schema
    let db = state.db();
    let schema = match db.db_instance.engine.get_schema(&req.table) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Json(ApiResponse::error(
                "TABLE_NOT_FOUND",
                &format!("表 '{}' 不存在", req.table),
            ))
        }
        Err(e) => return Json(ApiResponse::error("INTERNAL_ERROR", &e.to_string())),
    };

    // 2. 解析每一行
    let mut imported = 0;
    for item in &req.data {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };

        let mut values = Vec::new();
        for col in &schema.columns {
            match obj.get(&col.name) {
                Some(val) => {
                    let db_val = json_to_value(val, &col.col_type);
                    values.push(db_val);
                }
                None if col.nullable => values.push(crate::sql::types::Value::Null),
                None => {
                    return Json(ApiResponse::error(
                        "VALIDATION_ERROR",
                        &format!("缺少列 '{}' 的值", col.name),
                    ))
                }
            }
        }

        let row = crate::sql::types::Row { values };

        // 验证行
        if let Err(e) = schema.validate_row(&row.values) {
            return Json(ApiResponse::error("VALIDATION_ERROR", &e));
        }

        // 插入
        let db = state.db();
        if let Err(e) = db.db_instance.engine.insert_row(&req.table, row) {
            return Json(ApiResponse::error("INSERT_ERROR", &e.to_string()));
        }
        imported += 1;
    }

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    Json(ApiResponse::success(QueryResultData {
        columns: vec![],
        rows: vec![],
        rows_affected: imported,
        time_ms: elapsed,
    }))
}

// ── 辅助函数 ──

fn error_code(err: &RustMinidbError) -> &str {
    match err {
        RustMinidbError::Parse(_) => "PARSE_ERROR",
        RustMinidbError::Exec(e) => match e {
            crate::error::ExecError::TableNotFound(_) => "TABLE_NOT_FOUND",
            crate::error::ExecError::ColumnNotFound(_) => "COLUMN_NOT_FOUND",
            crate::error::ExecError::TypeMismatch(_) => "TYPE_MISMATCH",
            crate::error::ExecError::ConstraintViolation(_) => "CONSTRAINT_VIOLATION",
            crate::error::ExecError::Validation(_) => "VALIDATION_ERROR",
            _ => "EXEC_ERROR",
        },
        RustMinidbError::Engine(e) => match e {
            crate::error::EngineError::TableAlreadyExists(_) => "TABLE_ALREADY_EXISTS",
            crate::error::EngineError::PrimaryKeyConflict(_) => "PRIMARY_KEY_CONFLICT",
            crate::error::EngineError::TableNotFound(_) => "TABLE_NOT_FOUND",
            _ => "ENGINE_ERROR",
        },
        _ => "INTERNAL_ERROR",
    }
}

fn json_to_value(val: &serde_json::Value, col_type: &crate::sql::types::ColumnType) -> crate::sql::types::Value {
    match (col_type, val) {
        (crate::sql::types::ColumnType::Integer, serde_json::Value::Number(n)) => {
            crate::sql::types::Value::Integer(n.as_i64().unwrap_or(0))
        }
        (crate::sql::types::ColumnType::Float, serde_json::Value::Number(n)) => {
            crate::sql::types::Value::Float(n.as_f64().unwrap_or(0.0))
        }
        (crate::sql::types::ColumnType::Boolean, serde_json::Value::Bool(b)) => {
            crate::sql::types::Value::Boolean(*b)
        }
        (crate::sql::types::ColumnType::Text, serde_json::Value::String(s)) => {
            crate::sql::types::Value::Text(s.clone())
        }
        (crate::sql::types::ColumnType::Timestamp, serde_json::Value::String(s)) => {
            // 尝试解析 ISO 8601
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return crate::sql::types::Value::Timestamp(dt.timestamp_micros());
            }
            crate::sql::types::Value::Text(s.clone())
        }
        (_, serde_json::Value::Null) => crate::sql::types::Value::Null,
        (_, serde_json::Value::Number(n)) => {
            // 尝试数值转换
            if let Some(i) = n.as_i64() {
                crate::sql::types::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                crate::sql::types::Value::Float(f)
            } else {
                crate::sql::types::Value::Text(n.to_string())
            }
        }
        (_, other) => crate::sql::types::Value::Text(other.to_string()),
    }
}

/// 切换/创建数据库请求
#[derive(Deserialize)]
pub struct SwitchDbRequest {
    pub name: String,
    #[serde(default)]
    pub create: bool,
}

/// 列出所有数据库
pub async fn list_databases(
    State(state): State<AppState>,
) -> Json<ApiResponse> {
    let db = state.db();
    let current = db.current_db.clone();
    drop(db);
    match state.list_databases() {
        Ok(databases) => {
            let rows: Vec<Vec<serde_json::Value>> = databases.iter().map(|d| {
                vec![
                    serde_json::Value::String(d.clone()),
                    serde_json::Value::Bool(*d == current),
                ]
            }).collect();
            Json(ApiResponse::success(QueryResultData {
                columns: vec!["name".into(), "active".into()],
                rows,
                rows_affected: databases.len(),
                time_ms: 0.0,
            }))
        }
        Err(e) => Json(ApiResponse::error("INTERNAL_ERROR", &e.to_string())),
    }
}

/// 切换数据库
pub async fn switch_database(
    State(state): State<AppState>,
    Json(req): Json<SwitchDbRequest>,
) -> Json<ApiResponse> {
    if req.create {
        let exists = {
            let dbs = state.list_databases().unwrap_or_default();
            let db_name = if req.name.ends_with(".db") { req.name.clone() } else { format!("{}.db", req.name) };
            dbs.contains(&db_name)
        };
        if !exists {
            if let Err(e) = state.create_database(&req.name) {
                return Json(ApiResponse::error("CREATE_ERROR", &e.to_string()));
            }
        }
    }
    match state.switch_db(&req.name) {
        Ok(()) => Json(ApiResponse::success(QueryResultData {
            columns: vec!["database".into()],
            rows: vec![vec![serde_json::Value::String(req.name)]],
            rows_affected: 1,
            time_ms: 0.0,
        })),
        Err(e) => Json(ApiResponse::error("SWITCH_ERROR", &e.to_string())),
    }
}

/// 创建数据库
pub async fn create_database(
    State(state): State<AppState>,
    Json(req): Json<SwitchDbRequest>,
) -> Json<ApiResponse> {
    match state.create_database(&req.name) {
        Ok(()) => {
            let _ = state.switch_db(&req.name);
            Json(ApiResponse::success(QueryResultData {
                columns: vec!["database".into()],
                rows: vec![vec![serde_json::Value::String(req.name)]],
                rows_affected: 1,
                time_ms: 0.0,
            }))
        }
        Err(e) => Json(ApiResponse::error("CREATE_ERROR", &e.to_string())),
    }
}

/// 删除数据库
pub async fn delete_database(
    State(state): State<AppState>,
    Json(req): Json<SwitchDbRequest>,
) -> Json<ApiResponse> {
    let db = state.db();
    if db.current_db == req.name {
        return Json(ApiResponse::error("DELETE_ERROR", "不能删除当前正在使用的数据库"));
    }
    let db_path = format!("{}\\{}.db", db.db_dir.trim_end_matches('\\'), req.name);
    drop(db);
    match std::fs::remove_file(&db_path) {
        Ok(()) => Json(ApiResponse::success(QueryResultData {
            columns: vec![], rows: vec![], rows_affected: 1, time_ms: 0.0,
        })),
        Err(e) => Json(ApiResponse::error("DELETE_ERROR", &e.to_string())),
    }
}

// ═══════════════════════════════════════
// 备注管理 API
// ═══════════════════════════════════════

/// 设置备注请求
#[derive(Deserialize)]
pub struct CommentRequest {
    pub table: String,
    pub column: Option<String>,
    pub comment: String,
}

/// 设置备注（表或列）
pub async fn set_comment(
    State(state): State<AppState>,
    Json(req): Json<CommentRequest>,
) -> Json<ApiResponse> {
    let db = state.db();
    let mut schema = match db.db_instance.engine.get_schema(&req.table) {
        Ok(Some(s)) => s,
        Ok(None) => return Json(ApiResponse::error("TABLE_NOT_FOUND", &format!("表 '{}' 不存在", req.table))),
        Err(e) => return Json(ApiResponse::error("INTERNAL_ERROR", &e.to_string())),
    };
    drop(db);
    
    if let Some(col_name) = &req.column {
        if let Some(col) = schema.columns.iter_mut().find(|c| &c.name == col_name) {
            col.comment = if req.comment.is_empty() { None } else { Some(req.comment.clone()) };
        } else {
            return Json(ApiResponse::error("COLUMN_NOT_FOUND", &format!("列 '{}' 不存在", col_name)));
        }
    } else {
        schema.comment = if req.comment.is_empty() { None } else { Some(req.comment.clone()) };
    }
    
    let db = state.db();
    match db.db_instance.engine.update_schema(&schema) {
        Ok(()) => Json(ApiResponse::success(QueryResultData {
            columns: vec!["table".into(), "column".into(), "comment".into()],
            rows: vec![vec![
                serde_json::Value::String(req.table),
                serde_json::Value::String(req.column.unwrap_or_default()),
                serde_json::Value::String(req.comment),
            ]],
            rows_affected: 1, time_ms: 0.0,
        })),
        Err(e) => Json(ApiResponse::error("SAVE_ERROR", &e.to_string())),
    }
}

// ═══════════════════════════════════════
// 导出 & 监控 API
// ═══════════════════════════════════════

/// 导出数据库为标准 SQL
pub async fn export_database(
    State(state): State<AppState>,
) -> Json<ApiResponse> {
    let db = state.db();
    let engine = db.db_instance.engine.clone();
    drop(db);
    match crate::migration::export_database_to_string(engine) {
        Ok(sql) => {
            let rows = vec![vec![serde_json::Value::String(sql)]];
            Json(ApiResponse::success(QueryResultData {
                columns: vec!["sql".into()],
                rows,
                rows_affected: 1,
                time_ms: 0.0,
            }))
        }
        Err(e) => Json(ApiResponse::error("EXPORT_ERROR", &e.to_string())),
    }
}

/// 获取服务器性能指标（增强版）
pub async fn get_metrics(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let uptime = state.uptime_secs();
    let tables_count = state.db().db_instance.engine.list_tables().unwrap_or_default().len();

    // 使用增强的 monitor 指标
    let monitor_metrics = crate::monitor::metrics_to_json();

    // 合并系统指标
    let info = serde_json::json!({
        "success": true,
        "data": {
            "uptime_secs": uptime,
            "version": crate::version(),
            "database": {
                "current": state.db().current_db,
                "tables": tables_count,
            },
            "server": {
                "status": "ok",
                "version": crate::version(),
                "uptime_secs": uptime,
            },
            "metrics": monitor_metrics,
        }
    });
    Json(info)
}
