//! REST API 错误处理与 AppState

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::RustMinidbError;
use crate::sql::executor::Executor;
use crate::server::metrics::Metrics;
use crate::storage::engine::SharedEngine;
use crate::storage::redb_engine::RedbEngine;

/// 全局 API Token（由 AppState 初始化时设置，供认证中间件使用）
pub static APP_TOKEN: OnceLock<String> = OnceLock::new();

/// 数据库引擎 + 执行器（可替换）
pub struct DbInstance {
    pub engine: SharedEngine,
    pub executor: Arc<Executor>,
}

impl DbInstance {
    pub fn from_path(path: &str) -> Result<Self, RustMinidbError> {
        let engine = RedbEngine::open(path)?;
        let shared: SharedEngine = Arc::new(engine);
        let executor = Arc::new(Executor::new(shared.clone()));
        Ok(Self {
            engine: shared,
            executor,
        })
    }
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<RwLock<AppStateInner>>,
}

pub struct AppStateInner {
    pub current_db: String,
    pub db_dir: String,
    pub db_instance: DbInstance,
    pub start_time: std::time::Instant,
    pub metrics: Arc<Metrics>,
    /// API 访问令牌（None 表示不启用认证）
    pub api_token: Option<String>,
}

impl AppState {
    /// 通过文件路径创建新实例
    pub fn new(db_dir: &str, db_name: &str, api_token: Option<String>) -> Result<Self, RustMinidbError> {
        let db_path = Self::db_path(db_dir, db_name);
        let db_instance = DbInstance::from_path(&db_path)?;
        // 初始化全局 Token（仅首次设置有效）
        if let Some(ref token) = api_token {
            let _ = APP_TOKEN.set(token.clone());
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                current_db: db_name.to_string(),
                db_dir: db_dir.to_string(),
                db_instance,
                start_time: std::time::Instant::now(),
                metrics: Metrics::new(),
                api_token,
            })),
        })
    }

    /// 通过已有的引擎和执行器创建（用于测试和嵌入式场景）
    pub fn from_engine(engine: SharedEngine, executor: Arc<Executor>, api_token: Option<String>) -> Self {
        let db_instance = DbInstance {
            engine,
            executor,
        };
        // 初始化全局 Token（仅首次设置有效）
        if let Some(ref token) = api_token {
            let _ = APP_TOKEN.set(token.clone());
        }
        Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                current_db: "data.db".to_string(),
                db_dir: ".".to_string(),
                db_instance,
                start_time: std::time::Instant::now(),
                metrics: Metrics::new(),
                api_token,
            })),
        }
    }

    /// 切换数据库
    pub fn switch_db(&self, db_name: &str) -> Result<(), RustMinidbError> {
        let mut inner = self.inner.write().unwrap();
        let db_path = Self::db_path(&inner.db_dir, db_name);
        let db_instance = DbInstance::from_path(&db_path)?;
        inner.current_db = db_name.to_string();
        inner.db_instance = db_instance;
        Ok(())
    }

    /// 获取当前数据库实例
    pub fn db(&self) -> std::sync::RwLockReadGuard<'_, AppStateInner> {
        self.inner.read().unwrap()
    }

    /// 可用数据库列表
    pub fn list_databases(&self) -> Result<Vec<String>, RustMinidbError> {
        let inner = self.inner.read().unwrap();
        let dir = Path::new(&inner.db_dir);
        let mut databases = Vec::new();
        if dir.exists() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.path().extension().map_or(false, |e| e == "db") {
                    if let Some(name) = entry.file_name().to_str() {
                        databases.push(name.to_string());
                    }
                }
            }
        }
        databases.sort();
        Ok(databases)
    }

    /// 创建新数据库
    pub fn create_database(&self, db_name: &str) -> Result<(), RustMinidbError> {
        let inner = self.inner.read().unwrap();
        let db_path = Self::db_path(&inner.db_dir, db_name);
        // 创建空数据库文件
        let _ = RedbEngine::open(&db_path)?;
        Ok(())
    }

    fn db_path(db_dir: &str, db_name: &str) -> String {
        let name = if db_name.ends_with(".db") {
            db_name.to_string()
        } else {
            format!("{}.db", db_name)
        };
        format!("{}\\{}", db_dir.trim_end_matches('\\'), name)
    }

    /// 获取服务器运行时长（秒）
    pub fn uptime_secs(&self) -> u64 {
        self.inner.read().unwrap().start_time.elapsed().as_secs()
    }
}
