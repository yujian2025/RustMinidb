//! RustMinidb - 轻量级嵌入式关系型数据库
//!
//! RustMinidb 是一个使用 Rust 编写的轻量级嵌入式数据库，
//! 原生支持 REST API，适合物联网、边缘计算和嵌入式场景。
//!
//! # 嵌入式用法
//!
//! ```rust,no_run
//! use rustminidb::Database;
//!
//! let db = Database::open("data.db").unwrap();
//! db.execute("CREATE TABLE sensors (id INT PRIMARY KEY, value FLOAT)").unwrap();
//! db.execute("INSERT INTO sensors VALUES (1, 25.6)").unwrap();
//! let rows = db.query("SELECT * FROM sensors").unwrap();
//! for row in rows {
//!     println!("{:?}", row);
//! }
//! ```

pub mod banner;
pub mod cli;
pub mod config;
pub mod error;
pub mod migration;
pub mod monitor;
#[cfg(feature = "server")]
pub mod server;
pub mod sql;
pub mod storage;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::error::Result;
use crate::sql::executor::{ExecuteResult, Executor};
use crate::sql::parser::SqlParser;
use crate::sql::types::Value;
use crate::storage::engine::SharedEngine;
use crate::storage::redb_engine::RedbEngine;

/// RustMinidb 数据库主入口
///
/// 提供嵌入式 API，可直接在 Rust 程序中使用。
pub struct Database {
    engine: SharedEngine,
    executor: Executor,
    config: Config,
}

impl Database {
    /// 打开或创建数据库文件
    ///
    /// ```rust,no_run
    /// # use rustminidb::Database;
    /// let db = Database::open("data.db").unwrap();
    /// ```
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let engine = RedbEngine::open(path)?;
        let shared = Arc::new(engine);
        let executor = Executor::new(shared.clone());
        Ok(Self {
            engine: shared,
            executor,
            config: Config::default(),
        })
    }

    /// 带配置的打开
    pub fn open_with_config<P: AsRef<Path>>(path: P, config: Config) -> Result<Self> {
        let engine = RedbEngine::open(path)?;
        let shared = Arc::new(engine);
        let executor = Executor::new(shared.clone());
        Ok(Self {
            engine: shared,
            executor,
            config,
        })
    }

    /// 执行 SQL 语句（返回结构化结果）
    ///
    /// ```rust,no_run
    /// # use rustminidb::Database;
    /// let db = Database::open("data.db").unwrap();
    /// let result = db.execute("CREATE TABLE test (id INT PRIMARY KEY, val TEXT)").unwrap();
    /// ```
    pub fn execute(&self, sql: &str) -> Result<ExecuteResult> {
        let stmt = SqlParser::parse(sql)?;
        self.executor.execute(&stmt)
    }

    /// 查询并返回行列表（方便使用）
    ///
    /// ```rust,no_run
    /// # use rustminidb::Database;
    /// let db = Database::open("data.db").unwrap();
    /// let rows = db.query("SELECT * FROM test").unwrap();
    /// for row in rows {
    ///     println!("{:?}", row);
    /// }
    /// ```
    pub fn query(&self, sql: &str) -> Result<Vec<HashMap<String, Value>>> {
        let result = self.execute(sql)?;
        match result {
            ExecuteResult::QueryResult { columns, rows, .. } => {
                let maps: Vec<HashMap<String, Value>> = rows
                    .into_iter()
                    .map(|row| columns.iter().cloned().zip(row.into_iter()).collect())
                    .collect();
                Ok(maps)
            }
            _ => Ok(vec![]),
        }
    }

    /// 获取执行器引用
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    /// 获取存储引擎引用
    pub fn engine(&self) -> &SharedEngine {
        &self.engine
    }

    /// 获取配置
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 启动 HTTP 服务器（需要 server feature）
    #[cfg(feature = "server")]
    pub async fn serve<A>(self, addr: A) -> Result<()>
    where
        A: tokio::net::ToSocketAddrs + Send + 'static,
    {
        self.serve_with_token(addr, None).await
    }

    /// 启动 HTTP 服务器并指定 API 访问令牌
    #[cfg(feature = "server")]
    pub async fn serve_with_token<A>(self, addr: A, api_token: Option<String>) -> Result<()>
    where
        A: tokio::net::ToSocketAddrs + Send + 'static,
    {
        use crate::server::build_routes;
        use crate::server::error::AppState;
        use tracing::info;

        let state = AppState::new(".", "data.db", api_token)?;
        let app = build_routes(state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(
            "RustMinidb server starting on {}",
            listener.local_addr()?
        );
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// 获取版本号
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::types::ColumnType;
    use tempfile::TempDir;

    #[test]
    fn test_database_open() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).unwrap();
        assert_eq!(db.config().server.port, 8080);
    }

    #[test]
    fn test_database_create_and_insert() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).unwrap();

        db.execute("CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)")
            .unwrap();
        db.execute("INSERT INTO users VALUES (1, 'Alice', 30)")
            .unwrap();

        let rows = db.query("SELECT * FROM users").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("name").unwrap(),
            &Value::Text("Alice".into())
        );
    }

    #[test]
    fn test_database_full_crud() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).unwrap();

        // Create
        db.execute("CREATE TABLE items (id INT PRIMARY KEY, label TEXT, price FLOAT)")
            .unwrap();

        // Insert multiple
        db.execute("INSERT INTO items VALUES (1, 'item1', 10.5)")
            .unwrap();
        db.execute("INSERT INTO items VALUES (2, 'item2', 20.0)")
            .unwrap();

        // Select all
        let rows = db.query("SELECT * FROM items").unwrap();
        assert_eq!(rows.len(), 2);

        // Update
        db.execute("UPDATE items SET price = 15.0 WHERE id = 1")
            .unwrap();

        // Verify update
        let rows = db.query("SELECT price FROM items WHERE id = 1").unwrap();
        assert_eq!(rows[0].get("price").unwrap(), &Value::Float(15.0));

        // Delete
        db.execute("DELETE FROM items WHERE id = 2").unwrap();
        let rows = db.query("SELECT * FROM items").unwrap();
        assert_eq!(rows.len(), 1);

        // Drop
        db.execute("DROP TABLE items").unwrap();
        assert!(db.execute("SELECT * FROM items").is_err());
    }

    #[test]
    fn test_error_on_missing_table() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).unwrap();
        let result = db.execute("SELECT * FROM nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_error_on_duplicate_pk() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("test.db")).unwrap();
        db.execute("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)")
            .unwrap();
        db.execute("INSERT INTO t VALUES (1, 'a')").unwrap();
        let result = db.execute("INSERT INTO t VALUES (1, 'b')");
        assert!(result.is_err());
    }
}
