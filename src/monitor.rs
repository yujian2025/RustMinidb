//! 监控与日志系统（增强版）
//!
//! 提供统一的日志追踪、请求埋点、性能指标收集、请求追踪和事件系统。
//!
//! # 设计
//!
//! - **Metrics** — 全局原子指标计数器
//! - **TraceContext** — 请求追踪上下文（UUID + span）
//! - **QueryTimer** — SQL 执行耗时追踪
//! - **EventBus** — 轻量级监控事件发布/订阅
//! - **Histogram** — 查询延迟分桶统计
//! - **ConnectionTracker** — 连接池监控
//! - **DbSizeTracker** — 数据库文件大小记录

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tracing_subscriber::EnvFilter;

// ── 重新导出 ──

/// 默认使用全局 Metrics 实例
pub use crate::monitor::global_metrics as metrics;

/// 日志配置
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// 日志级别: trace, debug, info, warn, error
    pub level: String,
    /// 日志输出格式: text, json
    pub format: String,
    /// 是否启用追踪（UUID 请求 ID）
    pub tracing_enabled: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: "text".into(),
            tracing_enabled: true,
        }
    }
}

// ── 全局指标 ──

/// 全局监控指标（增强版）
#[derive(Debug, Default)]
pub struct Metrics {
    // ── 请求计数 ──
    /// 总请求数
    pub total_queries: AtomicU64,
    /// 成功请求数
    pub success_queries: AtomicU64,
    /// 失败请求数
    pub failed_queries: AtomicU64,

    // ── 操作类型计数 ──
    /// 创建表次数
    pub create_count: AtomicU64,
    /// 插入行数
    pub insert_count: AtomicU64,
    /// 查询次数
    pub select_count: AtomicU64,
    /// 更新次数
    pub update_count: AtomicU64,
    /// 删除次数
    pub delete_count: AtomicU64,

    // ── 性能 ──
    /// 累积耗时（微秒）
    pub total_time_us: AtomicU64,
    /// 最慢查询耗时（微秒）
    pub slowest_us: AtomicU64,
    /// 最快查询耗时（微秒）
    pub fastest_us: AtomicU64,

    // ── 连接和系统 ──
    /// 当前活跃连接数
    pub active_connections: AtomicI64,
    /// 总连接数（历史累计）
    pub total_connections: AtomicU64,
    /// 系统事件计数
    pub system_events: AtomicU64,
    /// 错误事件计数
    pub error_events: AtomicU64,

    // ── 行统计 ──
    /// 累计读取行数
    pub rows_read: AtomicU64,
    /// 累计写入行数
    pub rows_written: AtomicU64,
}

impl Metrics {
    /// 创建新的 Metrics 实例（Arc 包装）
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    // ── 请求记录 ──

    /// 记录一次查询（成功/失败 + 耗时）
    pub fn record_query(&self, success: bool, elapsed_us: u64) {
        self.total_queries.fetch_add(1, Ordering::Relaxed);
        if success {
            self.success_queries.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_queries.fetch_add(1, Ordering::Relaxed);
        }
        self.total_time_us.fetch_add(elapsed_us, Ordering::Relaxed);

        // 更新最慢/最快
        self.update_extremes(elapsed_us);
    }

    fn update_extremes(&self, elapsed_us: u64) {
        // 最快（宽松比较）
        loop {
            let cur = self.fastest_us.load(Ordering::Relaxed);
            if cur == 0 || elapsed_us < cur {
                if self.fastest_us.compare_exchange(cur, elapsed_us, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    break;
                }
            } else {
                break;
            }
        }
        // 最慢
        loop {
            let cur = self.slowest_us.load(Ordering::Relaxed);
            if elapsed_us > cur {
                if self.slowest_us.compare_exchange(cur, elapsed_us, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// 根据 SQL 类型记录操作计数
    pub fn record_statement(&self, sql: &str) {
        let sql = sql.trim().to_uppercase();
        if sql.starts_with("CREATE") {
            self.create_count.fetch_add(1, Ordering::Relaxed);
        } else if sql.starts_with("INSERT") {
            self.insert_count.fetch_add(1, Ordering::Relaxed);
        } else if sql.starts_with("SELECT") {
            self.select_count.fetch_add(1, Ordering::Relaxed);
        } else if sql.starts_with("UPDATE") {
            self.update_count.fetch_add(1, Ordering::Relaxed);
        } else if sql.starts_with("DELETE") {
            self.delete_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── 连接跟踪 ──

    /// 记录连接建立
    pub fn record_connection_open(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录连接关闭
    pub fn record_connection_close(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    // ── 行计数 ──

    /// 记录读取的行数
    pub fn record_rows_read(&self, count: u64) {
        self.rows_read.fetch_add(count, Ordering::Relaxed);
    }

    /// 记录写入的行数
    pub fn record_rows_written(&self, count: u64) {
        self.rows_written.fetch_add(count, Ordering::Relaxed);
    }

    // ── 系统/错误事件 ──

    /// 记录系统事件
    pub fn record_system_event(&self) {
        self.system_events.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录错误事件
    pub fn record_error_event(&self) {
        self.error_events.fetch_add(1, Ordering::Relaxed);
    }

    // ── 快照 ──

    /// 获取当前指标快照
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_queries: self.total_queries.load(Ordering::Relaxed),
            success_queries: self.success_queries.load(Ordering::Relaxed),
            failed_queries: self.failed_queries.load(Ordering::Relaxed),
            create_count: self.create_count.load(Ordering::Relaxed),
            insert_count: self.insert_count.load(Ordering::Relaxed),
            select_count: self.select_count.load(Ordering::Relaxed),
            update_count: self.update_count.load(Ordering::Relaxed),
            delete_count: self.delete_count.load(Ordering::Relaxed),
            total_time_us: self.total_time_us.load(Ordering::Relaxed),
            slowest_us: self.slowest_us.load(Ordering::Relaxed),
            fastest_us: self.fastest_us.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            system_events: self.system_events.load(Ordering::Relaxed),
            error_events: self.error_events.load(Ordering::Relaxed),
            rows_read: self.rows_read.load(Ordering::Relaxed),
            rows_written: self.rows_written.load(Ordering::Relaxed),
        }
    }
}

/// 指标快照（不可变，可序列化）
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    // ── 请求 ──
    pub total_queries: u64,
    pub success_queries: u64,
    pub failed_queries: u64,

    // ── 操作类型 ──
    pub create_count: u64,
    pub insert_count: u64,
    pub select_count: u64,
    pub update_count: u64,
    pub delete_count: u64,

    // ── 性能 ──
    pub total_time_us: u64,
    pub slowest_us: u64,
    pub fastest_us: u64,

    // ── 连接 ──
    pub active_connections: i64,
    pub total_connections: u64,

    // ── 系统 ──
    pub system_events: u64,
    pub error_events: u64,

    // ── 行 ──
    pub rows_read: u64,
    pub rows_written: u64,
}

impl MetricsSnapshot {
    /// 平均耗时（毫秒）
    pub fn avg_time_ms(&self) -> f64 {
        if self.total_queries == 0 {
            0.0
        } else {
            self.total_time_us as f64 / self.total_queries as f64 / 1000.0
        }
    }

    /// QPS（每秒查询数）
    pub fn qps(&self, uptime_secs: u64) -> f64 {
        if uptime_secs == 0 {
            0.0
        } else {
            self.total_queries as f64 / uptime_secs as f64
        }
    }

    /// 成功率
    pub fn success_rate(&self) -> f64 {
        if self.total_queries == 0 {
            1.0
        } else {
            self.success_queries as f64 / self.total_queries as f64
        }
    }

    /// 最慢查询耗时（毫秒）
    pub fn slowest_ms(&self) -> f64 {
        self.slowest_us as f64 / 1000.0
    }

    /// 最快查询耗时（毫秒）
    pub fn fastest_ms(&self) -> f64 {
        self.fastest_us as f64 / 1000.0
    }
}

// ── 全局 Metrics 实例 ──

static GLOBAL_METRICS: std::sync::OnceLock<Arc<Metrics>> = std::sync::OnceLock::new();

/// 获取全局 Metrics 实例
pub fn global_metrics() -> Arc<Metrics> {
    GLOBAL_METRICS
        .get_or_init(|| {
            let m = Metrics::new();
            tracing::info!("Global metrics initialized");
            m
        })
        .clone()
}

// ── 查询延迟直方图 ──

/// 查询延迟分桶（微秒）
const LATENCY_BUCKETS_MS: &[f64] = &[0.1, 0.5, 1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0];

/// 延迟直方图
#[derive(Debug, Default)]
pub struct LatencyHistogram {
    buckets: [AtomicU64; 10],
    total: AtomicU64,
}

impl LatencyHistogram {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 记录一次耗时（微秒）
    pub fn observe(&self, elapsed_us: u64) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let ms = elapsed_us as f64 / 1000.0;
        for (i, &bucket) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if ms <= bucket {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // 超过最大桶，计入最后一桶
        self.buckets[9].fetch_add(1, Ordering::Relaxed);
    }

    /// 获取直方图快照
    pub fn snapshot(&self) -> HistogramSnapshot {
        let mut counts = Vec::with_capacity(LATENCY_BUCKETS_MS.len());
        for b in &self.buckets {
            counts.push(b.load(Ordering::Relaxed));
        }
        HistogramSnapshot {
            buckets: LATENCY_BUCKETS_MS.to_vec(),
            counts,
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HistogramSnapshot {
    pub buckets: Vec<f64>,
    pub counts: Vec<u64>,
    pub total: u64,
}

// ── 请求追踪上下文 ──

/// 追踪上下文：为每个请求分配唯一 ID
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// 请求唯一 ID
    pub trace_id: String,
    /// 请求开始时间
    pub start: Instant,
    /// 操作名称
    pub operation: String,
    /// 是否启用详细追踪
    pub enabled: bool,
}

impl TraceContext {
    /// 创建新的追踪上下文
    pub fn new(operation: &str) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            start: Instant::now(),
            operation: operation.to_string(),
            enabled: true,
        }
    }

    /// 创建不带追踪 ID 的上下文（轻量）
    pub fn new_light(operation: &str) -> Self {
        Self {
            trace_id: String::new(),
            start: Instant::now(),
            operation: operation.to_string(),
            enabled: false,
        }
    }

    /// 记录追踪日志（debug 级别）
    pub fn log(&self, msg: &str) {
        if self.enabled {
            tracing::debug!(
                trace_id = %self.trace_id,
                operation = %self.operation,
                elapsed_us = %self.start.elapsed().as_micros(),
                "{}", msg
            );
        }
    }

    /// 完成追踪并返回耗时（微秒）
    pub fn finish(&self) -> u64 {
        let elapsed = self.start.elapsed().as_micros() as u64;
        if self.enabled {
            tracing::debug!(
                trace_id = %self.trace_id,
                operation = %self.operation,
                elapsed_us = elapsed,
                "Trace finished"
            );
        }
        elapsed
    }
}

// ── SQL 执行计时器 ──

/// 记录 SQL 执行耗时
pub struct QueryTimer {
    start: Instant,
    sql: String,
    metrics: Arc<Metrics>,
    trace: Option<TraceContext>,
}

impl QueryTimer {
    /// 创建新的查询计时器
    pub fn new(sql: &str, metrics: Arc<Metrics>) -> Self {
        metrics.record_statement(sql);
        Self {
            start: Instant::now(),
            sql: sql.to_string(),
            metrics,
            trace: None,
        }
    }

    /// 创建带追踪的查询计时器
    pub fn new_traced(sql: &str, metrics: Arc<Metrics>, trace: TraceContext) -> Self {
        metrics.record_statement(sql);
        trace.log(&format!("Executing SQL: {}", sql));
        Self {
            start: Instant::now(),
            sql: sql.to_string(),
            metrics,
            trace: Some(trace),
        }
    }

    /// 完成计时（成功）
    pub fn finish(&self, success: bool) -> u64 {
        let elapsed = self.start.elapsed().as_micros() as u64;
        self.metrics.record_query(success, elapsed);

        // 输出结构化日志
        if success {
            tracing::debug!(
                sql = %self.sql,
                elapsed_us = elapsed,
                elapsed_ms = format!("{:.2}", elapsed as f64 / 1000.0),
                success = true,
                "SQL executed successfully"
            );
        } else {
            tracing::warn!(
                sql = %self.sql,
                elapsed_us = elapsed,
                success = false,
                "SQL execution failed"
            );
        }

        if let Some(ref trace) = self.trace {
            trace.log(&format!("SQL done ({}μs)", elapsed));
        }

        elapsed
    }

    /// 获取已耗用的微秒数
    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

// ── 事件系统 ──

/// 监控事件类型
#[derive(Debug, Clone)]
pub enum MonitorEvent {
    /// SQL 执行事件
    QueryExecuted {
        sql: String,
        elapsed_us: u64,
        success: bool,
        rows_affected: u64,
    },
    /// 连接事件
    ConnectionOpened,
    ConnectionClosed,
    /// 系统事件
    System(String),
    /// 错误事件
    Error { source: String, message: String },
    /// 自定义事件
    Custom { name: String, data: String },
}

/// 事件处理器
pub type EventHandler = Arc<dyn Fn(&MonitorEvent) + Send + Sync>;

/// 轻量级事件总线
#[derive(Default)]
pub struct EventBus {
    handlers: Mutex<Vec<EventHandler>>,
}

impl EventBus {
    /// 创建新的事件总线
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            handlers: Mutex::new(Vec::new()),
        })
    }

    /// 注册事件处理器
    pub fn subscribe(&self, handler: EventHandler) {
        if let Ok(mut handlers) = self.handlers.lock() {
            handlers.push(handler);
        }
    }

    /// 发布事件
    pub fn publish(&self, event: &MonitorEvent) {
        if let Ok(handlers) = self.handlers.lock() {
            for handler in handlers.iter() {
                handler(event);
            }
        }
    }
}

// 全局事件总线
static GLOBAL_EVENT_BUS: std::sync::OnceLock<Arc<EventBus>> = std::sync::OnceLock::new();

/// 获取全局事件总线
pub fn global_event_bus() -> Arc<EventBus> {
    GLOBAL_EVENT_BUS
        .get_or_init(|| {
            let bus = EventBus::new();
            // 默认订阅：日志输出
            let logger: EventHandler = Arc::new(|event: &MonitorEvent| {
                match event {
                    MonitorEvent::QueryExecuted { sql, elapsed_us, success, rows_affected } => {
                        if *success {
                            tracing::info!(
                                sql = %sql,
                                elapsed_us = elapsed_us,
                                rows_affected = rows_affected,
                                "Query executed"
                            );
                        } else {
                            tracing::error!(
                                sql = %sql,
                                elapsed_us = elapsed_us,
                                "Query failed"
                            );
                        }
                    }
                    MonitorEvent::Error { source, message } => {
                        tracing::error!(source = %source, error = %message, "Monitoring error");
                    }
                    _ => {}
                }
            });
            bus.subscribe(logger);
            tracing::info!("Global event bus initialized");
            bus
        })
        .clone()
}

// ── 便捷函数（全局 Metrics 代理） ──

/// 记录查询（快捷方式）
pub fn record_query(sql: &str, elapsed_us: u64, rows: u64) {
    let m = global_metrics();
    m.record_query(true, elapsed_us);
    m.record_rows_read(rows);
}

/// 记录写操作（快捷方式）
pub fn record_write(sql: &str, elapsed_us: u64, rows: u64) {
    let m = global_metrics();
    m.record_query(true, elapsed_us);
    m.record_rows_written(rows);
}

/// 记录错误（快捷方式）
pub fn record_error(source: &str, message: &str) {
    let m = global_metrics();
    m.record_error_event();
    tracing::error!(source = %source, error = %message, "Error recorded");
}

/// 记录系统事件（快捷方式）
pub fn record_system(event: &str) {
    let m = global_metrics();
    m.record_system_event();
    tracing::info!(event = %event, "System event");
}

/// 记录连接打开
pub fn record_connection_open() {
    let m = global_metrics();
    m.record_connection_open();
}

/// 记录连接关闭
pub fn record_connection_close() {
    let m = global_metrics();
    m.record_connection_close();
}

/// 打印指标摘要到标准输出
pub fn print_metrics_summary() {
    let m = global_metrics();
    let snap = m.snapshot();
    println!();
    println!("═══ RustMinidb Runtime Metrics ═══");
    println!("  Queries   : {} total ({} ok, {} failed)",
        snap.total_queries, snap.success_queries, snap.failed_queries);
    println!("  By type   : C:{}/I:{}/S:{}/U:{}/D:{}",
        snap.create_count, snap.insert_count, snap.select_count,
        snap.update_count, snap.delete_count);
    println!("  Latency   : avg {:.2}ms, slowest {:.2}ms, fastest {:.2}ms",
        snap.avg_time_ms(), snap.slowest_ms(), snap.fastest_ms());
    println!("  Rows      : {} read, {} written", snap.rows_read, snap.rows_written);
    println!("  Conn      : {} active ({} total)", snap.active_connections, snap.total_connections);
    println!("  Errors    : {}", snap.error_events);
    println!("  System    : {} events", snap.system_events);
    println!("══════════════════════════════════");
    println!();
}

/// 获取指标摘要的 JSON 值
pub fn metrics_to_json() -> serde_json::Value {
    let m = global_metrics();
    let snap = m.snapshot();
    serde_json::json!({
        "total_queries": snap.total_queries,
        "success_queries": snap.success_queries,
        "failed_queries": snap.failed_queries,
        "create_count": snap.create_count,
        "insert_count": snap.insert_count,
        "select_count": snap.select_count,
        "update_count": snap.update_count,
        "delete_count": snap.delete_count,
        "avg_time_ms": format!("{:.2}", snap.avg_time_ms()),
        "slowest_ms": format!("{:.2}", snap.slowest_ms()),
        "fastest_ms": format!("{:.2}", snap.fastest_ms()),
        "rows_read": snap.rows_read,
        "rows_written": snap.rows_written,
        "active_connections": snap.active_connections,
        "total_connections": snap.total_connections,
        "error_events": snap.error_events,
        "system_events": snap.system_events,
        "success_rate": format!("{:.2}%", snap.success_rate() * 100.0),
    })
}

// ── 日志初始化 ──

/// 初始化统一日志系统
pub fn init_logging(config: &MonitorConfig) {
    let filter = EnvFilter::new(&config.level);

    match config.format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_thread_ids(false)
                .compact()
                .init();
        }
    }

    // 初始化全局 Metrics
    global_metrics();
    global_event_bus();

    tracing::info!(
        "Logging initialized: level={}, format={}, tracing={}",
        config.level,
        config.format,
        config.tracing_enabled
    );
}

// ── 数据库大小跟踪 ──

/// 获取数据库文件大小（字节）
pub fn db_file_size(path: &str) -> std::io::Result<u64> {
    std::fs::metadata(path).map(|m| m.len())
}

/// 格式化文件大小
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_idx])
}

// ── 计时器（兼容旧的 Timer API） ──

/// 简易计时器（用于旧 API 兼容）
pub struct Timer {
    start: Instant,
}

impl Timer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    pub fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_record_query() {
        let metrics = Metrics::new();
        metrics.record_query(true, 100);
        metrics.record_query(false, 200);
        let snap = metrics.snapshot();
        assert_eq!(snap.total_queries, 2);
        assert_eq!(snap.success_queries, 1);
        assert_eq!(snap.failed_queries, 1);
        assert_eq!(snap.total_time_us, 300);
    }

    #[test]
    fn test_metrics_record_statement() {
        let metrics = Metrics::new();
        metrics.record_statement("CREATE TABLE t (id INT)");
        metrics.record_statement("INSERT INTO t VALUES (1)");
        metrics.record_statement("SELECT * FROM t");
        metrics.record_statement("UPDATE t SET id=2");
        metrics.record_statement("DELETE FROM t");
        let snap = metrics.snapshot();
        assert_eq!(snap.create_count, 1);
        assert_eq!(snap.insert_count, 1);
        assert_eq!(snap.select_count, 1);
        assert_eq!(snap.update_count, 1);
        assert_eq!(snap.delete_count, 1);
    }

    #[test]
    fn test_metrics_snapshot_qps() {
        let m = Metrics::new();
        m.record_query(true, 1000);
        let snap = m.snapshot();
        assert!(snap.qps(1) > 0.0);
        assert_eq!(snap.qps(0), 0.0);
    }

    #[test]
    fn test_global_metrics_singleton() {
        let a = global_metrics();
        let b = global_metrics();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn test_trace_context() {
        let trace = TraceContext::new("test");
        assert!(!trace.trace_id.is_empty());
        assert_eq!(trace.operation, "test");
        let elapsed = trace.finish();
        assert!(elapsed < 100_000); // 应在 100ms 内
    }

    #[test]
    fn test_latency_histogram() {
        let h = LatencyHistogram::new();
        h.observe(100);     // 0.1ms
        h.observe(1000);    // 1ms
        h.observe(1_000_000); // 1s
        let snap = h.snapshot();
        assert_eq!(snap.total, 3);
    }

    #[test]
    fn test_db_file_size_format() {
        let s = format_file_size(1024);
        assert!(s.contains("KB"));
        let s = format_file_size(1_048_576);
        assert!(s.contains("MB"));
    }

    #[test]
    fn test_timer() {
        let t = Timer::start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(t.elapsed_ms() >= 10.0);
        assert!(t.elapsed_us() >= 10_000);
    }

    #[test]
    fn test_event_bus() {
        let bus = EventBus::new();
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        bus.subscribe(Arc::new(move |_event: &MonitorEvent| {
            counter_clone.fetch_add(1, Ordering::Relaxed);
        }));
        bus.publish(&MonitorEvent::System("test".into()));
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
