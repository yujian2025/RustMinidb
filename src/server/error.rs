//! REST API 错误处理与 AppState
//!
//! 支持多数据库同时打开、文件监听自动重载。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::RustMinidbError;
use crate::sql::executor::Executor;
use crate::storage::engine::SharedEngine;
use crate::storage::redb_engine::RedbEngine;
use crate::watcher;

/// 全局 API Token（由 AppState 初始化时设置，供认证中间件使用）
pub static APP_TOKEN: OnceLock<String> = OnceLock::new();

/// 数据库引擎 + 执行器（可替换）
#[derive(Clone)]
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

    /// 重新加载数据库引擎（文件变化时调用）
    pub fn reload(&mut self, path: &str) -> Result<(), RustMinidbError> {
        let engine = RedbEngine::open(path)?;
        let shared: SharedEngine = Arc::new(engine);
        self.engine = shared.clone();
        self.executor = Arc::new(Executor::new(shared));
        Ok(())
    }
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<RwLock<AppStateInner>>,
}

pub struct AppStateInner {
    /// 当前活跃的数据库名称
    pub current_db: String,
    /// 数据库文件所在目录
    pub db_dir: String,
    /// 当前数据库实例
    pub db_instance: DbInstance,
    /// 所有已打开的数据库（多数据库支持）
    pub databases: HashMap<String, DbInstance>,
    /// 服务器启动时间
    pub start_time: std::time::Instant,
    /// API 访问令牌（None 表示不启用认证）
    pub api_token: Option<String>,
}

impl AppState {
    /// 通过单个文件路径创建新实例
    pub fn new(db_dir: &str, db_name: &str, api_token: Option<String>) -> Result<Self, RustMinidbError> {
        let db_path = Self::db_path(db_dir, db_name);
        let db_instance = DbInstance::from_path(&db_path)?;
        let mut databases = HashMap::new();
        databases.insert(db_name.to_string(), db_instance.clone());

        // 初始化全局 Token（仅首次设置有效）
        if let Some(ref token) = api_token {
            let _ = APP_TOKEN.set(token.clone());
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                current_db: db_name.to_string(),
                db_dir: db_dir.to_string(),
                db_instance,
                databases,
                start_time: std::time::Instant::now(),
                api_token,
            })),
        })
    }

    /// 通过多个数据库文件创建（多数据库同时打开）
    pub fn new_multi(
        db_dir: &str,
        db_names: &[String],
        api_token: Option<String>,
    ) -> Result<Self, RustMinidbError>
    {
        if db_names.is_empty() {
            return Err(RustMinidbError::Config(
                "At least one database file must be specified".into(),
            ));
        }

        let mut databases = HashMap::new();
        let mut db_instance = None;

        for name in db_names {
            let db_path = Self::db_path(db_dir, name);
            let instance = DbInstance::from_path(&db_path)?;
            if db_instance.is_none() {
                db_instance = Some(instance.clone());
            }
            databases.insert(name.clone(), instance);
        }

        let current_db = db_names[0].clone();
        let db_instance = db_instance.unwrap();

        // 初始化全局 Token（仅首次设置有效）
        if let Some(ref token) = api_token {
            let _ = APP_TOKEN.set(token.clone());
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                current_db,
                db_dir: db_dir.to_string(),
                db_instance,
                databases,
                start_time: std::time::Instant::now(),
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
        let mut databases = HashMap::new();
        databases.insert("data.db".to_string(), db_instance.clone());

        // 初始化全局 Token（仅首次设置有效）
        if let Some(ref token) = api_token {
            let _ = APP_TOKEN.set(token.clone());
        }
        Self {
            inner: Arc::new(RwLock::new(AppStateInner {
                current_db: "data.db".to_string(),
                db_dir: ".".to_string(),
                db_instance,
                databases,
                start_time: std::time::Instant::now(),
                api_token,
            })),
        }
    }

    /// 切换数据库
    pub fn switch_db(&self, db_name: &str) -> Result<(), RustMinidbError> {
        let mut inner = self.inner.write().expect("AppState lock poisoned");

        // 如果已经打开，直接切换
        if inner.databases.contains_key(db_name) {
            let instance = inner.databases.get(db_name).unwrap().clone();
            inner.current_db = db_name.to_string();
            inner.db_instance = instance;
            return Ok(());
        }

        // 否则尝试打开新数据库
        let db_path = Self::db_path(&inner.db_dir, db_name);
        let db_instance = DbInstance::from_path(&db_path)?;
        inner.current_db = db_name.to_string();
        inner.db_instance = db_instance.clone();
        inner.databases.insert(db_name.to_string(), db_instance);
        Ok(())
    }

    /// 获取当前数据库实例
    pub fn db(&self) -> std::sync::RwLockReadGuard<'_, AppStateInner> {
        self.inner.read().expect("AppState lock poisoned")
    }

    /// 获取可写锁
    pub fn db_mut(&self) -> std::sync::RwLockWriteGuard<'_, AppStateInner> {
        self.inner.write().expect("AppState lock poisoned")
    }

    /// 重新加载指定数据库（文件变化时自动调用）
    pub fn reload_database(&self, db_name: &str) -> Result<(), RustMinidbError> {
        let mut inner = self.inner.write().expect("AppState lock poisoned");
        let db_path = Self::db_path(&inner.db_dir, db_name);
        let is_current = inner.current_db == db_name;

        if inner.databases.contains_key(db_name) {
            // 在块内执行 reload，完成后释放可变借用
            let updated_instance = {
                let instance = inner.databases.get_mut(db_name).unwrap();
                instance.reload(&db_path)?;
                instance.clone()
            };
            if is_current {
                inner.db_instance = updated_instance;
            }
            tracing::info!("Database auto-reloaded: {} (file changed)", db_name);
        } else {
            // 未打开的数据库，自动打开
            let instance = DbInstance::from_path(&db_path)?;
            inner.databases.insert(db_name.to_string(), instance.clone());
            if is_current {
                inner.db_instance = instance;
            }
            tracing::info!("Database auto-opened: {} (file created)", db_name);
        }
        Ok(())
    }

    /// 获取所有已打开的数据库名称列表
    pub fn loaded_databases(&self) -> Vec<String> {
        let inner = self.inner.read().expect("AppState lock poisoned");
        inner.databases.keys().cloned().collect()
    }

    /// 获取当前数据库名称
    pub fn current_db_name(&self) -> String {
        let inner = self.inner.read().expect("AppState lock poisoned");
        inner.current_db.clone()
    }

    /// 可用数据库列表（目录中所有 .db 文件 + 已打开的数据库）
    pub fn list_databases(&self) -> Result<Vec<String>, RustMinidbError> {
        let inner = self.inner.read().expect("AppState lock poisoned");
        let dir = Path::new(&inner.db_dir);
        let mut databases: Vec<String> = Vec::new();

        // 添加已打开的数据库
        for name in inner.databases.keys() {
            if !databases.contains(name) {
                databases.push(name.clone());
            }
        }

        // 扫描目录中的 .db 文件
        if dir.exists() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.path().extension().map_or(false, |e| e == "db") {
                    if let Some(name) = entry.file_name().to_str() {
                        let name = name.to_string();
                        if !databases.contains(&name) {
                            databases.push(name);
                        }
                    }
                }
            }
        }
        databases.sort();
        Ok(databases)
    }

    /// 创建新数据库
    pub fn create_database(&self, db_name: &str) -> Result<(), RustMinidbError> {
        let inner = self.inner.read().expect("AppState lock poisoned");
        let db_path = Self::db_path(&inner.db_dir, db_name);
        // 创建空数据库文件
        let _ = RedbEngine::open(&db_path)?;
        Ok(())
    }

    /// 删除数据库（从状态中移除）
    pub fn remove_database(&self, db_name: &str) {
        let mut inner = self.inner.write().expect("AppState lock poisoned");
        inner.databases.remove(db_name);
        // 如果删除的是当前数据库，切换到第一个可用数据库
        if inner.current_db == db_name {
            // 先克隆第一个可用的 key 和 instance，避免借用冲突
            let first_entry: Option<(String, DbInstance)> = {
                let mut iter = inner.databases.iter();
                iter.next().map(|(k, v)| (k.clone(), v.clone()))
            };
            if let Some((name, instance)) = first_entry {
                inner.current_db = name;
                inner.db_instance = instance;
            }
        }
    }

    fn db_path(db_dir: &str, db_name: &str) -> String {
        let name = if db_name.ends_with(".db") {
            db_name.to_string()
        } else {
            format!("{}.db", db_name)
        };
        Path::new(db_dir).join(&name).to_string_lossy().to_string()
    }

    /// 获取服务器运行时长（秒）
    pub fn uptime_secs(&self) -> u64 {
        self.inner.read().expect("AppState lock poisoned").start_time.elapsed().as_secs()
    }

    /// 启动文件监听自动重载
    pub fn start_file_watcher(&self) -> Result<watcher::FileWatcher, RustMinidbError> {
        let state = self.clone();
        let mut file_watcher = watcher::FileWatcher::new();

        let cb: watcher::ChangeCallback = Arc::new(move |event| {
            let db_name = match &event {
                watcher::FileChangeEvent::DatabaseModified { db_name, .. } => db_name.clone(),
                watcher::FileChangeEvent::DatabaseCreated { db_name, .. } => db_name.clone(),
                watcher::FileChangeEvent::DatabaseDeleted { db_name, .. } => db_name.clone(),
            };

            match &event {
                watcher::FileChangeEvent::DatabaseModified { .. } => {
                    let _ = state.reload_database(&db_name);
                }
                watcher::FileChangeEvent::DatabaseCreated { .. } => {
                    let _ = state.reload_database(&db_name);
                }
                watcher::FileChangeEvent::DatabaseDeleted { .. } => {
                    state.remove_database(&db_name);
                    tracing::info!("Database removed (file deleted): {}", db_name);
                }
            }
        });

        let dir = {
            let inner = self.inner.read().expect("AppState lock poisoned");
            inner.db_dir.clone()
        };

        file_watcher.watch(&dir, cb).map_err(|e| {
            RustMinidbError::Config(format!("Failed to start file watcher: {}", e))
        })?;

        tracing::info!("File watcher started for directory: {}", dir);
        Ok(file_watcher)
    }
}