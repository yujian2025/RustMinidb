//! RustMinidb 监控与埋点系统
//!
//! 统一收集：查询次数、执行时间、错误次数、慢查询

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 监控指标
#[derive(Default)]
pub struct Metrics {
    // 查询统计
    pub total_queries: AtomicU64,
    pub successful_queries: AtomicU64,
    pub failed_queries: AtomicU64,
    pub total_query_time_ns: AtomicU64,
    pub slow_queries: AtomicU64,

    // 行统计
    pub rows_inserted: AtomicU64,
    pub rows_deleted: AtomicU64,
    pub rows_returned: AtomicU64,

    // 连接
    pub tables_created: AtomicU64,
    pub tables_dropped: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Metrics::default())
    }

    /// 记录一次查询执行
    pub fn record_query(&self, success: bool, duration: std::time::Duration, rows: usize) {
        self.total_queries.fetch_add(1, Ordering::Relaxed);
        if success {
            self.successful_queries.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_queries.fetch_add(1, Ordering::Relaxed);
        }
        self.total_query_time_ns
            .fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
        self.rows_returned.fetch_add(rows as u64, Ordering::Relaxed);

        // 慢查询阈值：500ms
        if duration.as_millis() > 500 {
            self.slow_queries.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 获取指标快照
    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total_queries.load(Ordering::Relaxed);
        MetricsSnapshot {
            total_queries: total,
            successful_queries: self.successful_queries.load(Ordering::Relaxed),
            failed_queries: self.failed_queries.load(Ordering::Relaxed),
            avg_query_time_ms: if total > 0 {
                (self.total_query_time_ns.load(Ordering::Relaxed) / total) as f64 / 1_000_000.0
            } else {
                0.0
            },
            slow_queries: self.slow_queries.load(Ordering::Relaxed),
            rows_inserted: self.rows_inserted.load(Ordering::Relaxed),
            rows_deleted: self.rows_deleted.load(Ordering::Relaxed),
            rows_returned: self.rows_returned.load(Ordering::Relaxed),
            tables_created: self.tables_created.load(Ordering::Relaxed),
            tables_dropped: self.tables_dropped.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    pub total_queries: u64,
    pub successful_queries: u64,
    pub failed_queries: u64,
    pub avg_query_time_ms: f64,
    pub slow_queries: u64,
    pub rows_inserted: u64,
    pub rows_deleted: u64,
    pub rows_returned: u64,
    pub tables_created: u64,
    pub tables_dropped: u64,
}

/// 启动 Banner
pub fn print_banner(version: &str, port: u16, db_dir: &str, db_name: &str) {
    let banner = format!(
        r#"
  ╔══════════════════════════════════════════════════╗
  ║                                                  ║
  ║     ____                  _   _  __ _     _      ║
  ║    |  _ \ _   _ _ __ ___ | |_(_)/ _(_) __| |     ║
  ║    | |_) | | | | '_ ` _ \| __| | |_| |/ _` |     ║
  ║    |  _ <| |_| | | | | | | |_| |  _| | (_| |     ║
  ║    |_| \_\\__,_|_| |_| |_|\__|_|_| |_|\__,_|     ║
  ║                                                  ║
  ║    v{:<8}  Lightweight Embedded Database       ║
  ║    Port: {:<6}  Database: {:<16} ║
  ║    Dir:  {:<35}║
  ║                                                  ║
  ║    🌐 Web UI  : http://localhost:{:<4}/          ║
  ║    📡 API     : http://localhost:{:<4}/v1/query  ║
  ║    📁 Storage : redb (ACID, single-file)         ║
  ║    📜 License : BSL-1.1                          ║
  ║                                                  ║
  ╚══════════════════════════════════════════════════╝
"#,
        version, port, db_name, db_dir, port, port
    );
    println!("{}", banner);
}
