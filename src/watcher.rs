//! 文件监听模块
//!
//! 监听数据库文件变化，自动刷新数据库连接。
//! 使用 `notify` 库实现跨平台文件系统事件监听。

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::RustMinidbError;
use crate::storage::engine::SharedEngine;
use crate::storage::redb_engine::RedbEngine;

/// 文件变更事件
#[derive(Debug, Clone)]
pub enum FileChangeEvent {
    /// 数据库文件被修改
    DatabaseModified {
        path: String,
        db_name: String,
    },
    /// 新数据库文件被创建
    DatabaseCreated {
        path: String,
        db_name: String,
    },
    /// 数据库文件被删除
    DatabaseDeleted {
        path: String,
        db_name: String,
    },
}

/// 文件变更回调
pub type ChangeCallback = Arc<dyn Fn(FileChangeEvent) + Send + Sync>;

/// 文件监听器
pub struct FileWatcher {
    watcher: Option<RecommendedWatcher>,
    handle: Option<thread::JoinHandle<()>>,
    running: Arc<Mutex<bool>>,
}

impl FileWatcher {
    /// 创建一个新的文件监听器
    pub fn new() -> Self {
        Self {
            watcher: None,
            handle: None,
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// 开始监听指定目录下的 .db 文件变化
    ///
    /// `callback` 在文件发生变化时被调用。
    pub fn watch<P: AsRef<Path>>(
        &mut self,
        dir: P,
        callback: ChangeCallback,
    ) -> crate::error::Result<()> {
        let dir_path = dir.as_ref().to_path_buf();
        if !dir_path.exists() {
            return Err(RustMinidbError::Config(format!(
                "Directory does not exist: {}",
                dir_path.display()
            )));
        }

        let (tx, rx) = mpsc::channel::<crate::error::Result<Event>>();
        let running = self.running.clone();

        // 构造通知事件发送器
        let event_tx = tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                match res {
                    Ok(event) => {
                        let _ = event_tx.send(Ok(event));
                    }
                    Err(e) => {
                        let _ = event_tx.send(Err(RustMinidbError::Config(
                            format!("File watch error: {}", e),
                        )));
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| {
            RustMinidbError::Config(format!("Failed to create file watcher: {}", e))
        })?;

        watcher
            .watch(&dir_path, RecursiveMode::NonRecursive)
            .map_err(|e| {
                RustMinidbError::Config(format!(
                    "Failed to watch directory {}: {}",
                    dir_path.display(),
                    e
                ))
            })?;

        *running.lock().unwrap() = true;
        let running_clone = running.clone();
        let cb = callback.clone();

        let handle = thread::spawn(move || {
            let debounce_map: Arc<Mutex<HashMap<String, std::time::Instant>>> =
                Arc::new(Mutex::new(HashMap::new()));

            loop {
                if !*running_clone.lock().unwrap() {
                    break;
                }

                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Ok(event)) => {
                        // 只处理 .db 文件相关事件
                        let db_events: Vec<FileChangeEvent> = event
                            .paths
                            .iter()
                            .filter(|p| {
                                p.extension()
                                    .map(|e| e == "db")
                                    .unwrap_or(false)
                            })
                            .filter_map(|path| {
                                let path_str = path.to_string_lossy().to_string();
                                let file_name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();

                                // 防抖：同一文件在 2 秒内不重复触发
                                {
                                    let mut debounce = debounce_map.lock().unwrap();
                                    if let Some(last) = debounce.get(&path_str) {
                                        if last.elapsed() < Duration::from_secs(2) {
                                            return None;
                                        }
                                    }
                                    debounce.insert(path_str.clone(), std::time::Instant::now());
                                }

                                match &event.kind {
                                    EventKind::Modify(_) => Some(FileChangeEvent::DatabaseModified {
                                        path: path_str,
                                        db_name: file_name,
                                    }),
                                    EventKind::Create(_) => Some(FileChangeEvent::DatabaseCreated {
                                        path: path_str,
                                        db_name: file_name,
                                    }),
                                    EventKind::Remove(_) => Some(FileChangeEvent::DatabaseDeleted {
                                        path: path_str,
                                        db_name: file_name,
                                    }),
                                    _ => None,
                                }
                            })
                            .collect();

                        for evt in db_events {
                            cb(evt);
                        }
                    }
                    Ok(Err(_e)) => {
                        // 忽略单个错误，继续监听
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // 正常超时，继续循环
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        });

        self.watcher = Some(watcher);
        self.handle = Some(handle);

        Ok(())
    }

    /// 停止监听
    pub fn stop(&mut self) {
        *self.running.lock().unwrap() = false;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.watcher = None;
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 自动重载数据库引擎的辅助函数
///
/// 当数据库文件变化时，自动重新打开发动机。
pub fn reload_engine(path: &str) -> crate::error::Result<SharedEngine> {
    let engine = RedbEngine::open(path)?;
    Ok(Arc::new(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    #[test]
    fn test_watcher_create_and_drop() {
        let dir = TempDir::new().unwrap();
        let mut watcher = FileWatcher::new();
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        let cb: ChangeCallback = Arc::new(move |_evt| {
            f.store(true, Ordering::SeqCst);
        });

        let result = watcher.watch(dir.path(), cb);
        assert!(result.is_ok());
        watcher.stop();
    }
}