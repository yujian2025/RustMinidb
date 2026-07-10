//! RustMinidb 命令行入口
//!
//! 支持子命令：
//! - serve: 启动 HTTP 服务器（支持多数据库、文件监听）
//! - shell: 交互式 SQL 控制台（支持多数据库切换、文件监听）
//! - exec: 执行单条 SQL
//! - init: 初始化数据库
//! - export: 导出数据库为 SQL
//! - version: 显示版本信息

use clap::Parser;

use rustminidb::cli::commands::{Cli, Commands};
use rustminidb::error::Result;
use rustminidb::sql::executor::ExecuteResult;
use rustminidb::sql::executor::Executor;
use rustminidb::sql::parser::SqlParser;
use rustminidb::storage::engine::StorageEngine;
use rustminidb::storage::redb_engine::RedbEngine;
use std::sync::Arc;

fn init_logging() {
    // 使用 MonitorConfig 初始化统一的日志系统
    let config = rustminidb::monitor::MonitorConfig::default();
    rustminidb::monitor::init_logging(&config);
}

fn main() -> Result<()> {
    // 记录启动时间
    rustminidb::banner::record_start_time();

    // 打印增强版启动横幅
    rustminidb::banner::print_banner();

    // 初始化统一日志系统
    init_logging();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            host,
            port,
            db,
            db_dir,
            max_connections: _,
            watch,
            api_token,
        } => cmd_serve(host, port, db, db_dir, watch, api_token),
        Commands::Shell { db, watch } => cmd_shell(db, watch),
        Commands::Exec { db, sql, format } => cmd_exec(db, sql, format),
        Commands::Init { db } => cmd_init(db),
        Commands::Export {
            db,
            output,
            table,
            data,
            create,
        } => cmd_export(db, output, table, data, create),
        Commands::Version => cmd_version(),
    }
}

/// 启动 HTTP 服务器
#[cfg(feature = "server")]
fn cmd_serve(
    host: String,
    port: u16,
    dbs: Vec<String>,
    db_dir: String,
    watch: bool,
    api_token: Option<String>,
) -> Result<()> {
    use rustminidb::banner;
    use rustminidb::server::build_routes;
    use rustminidb::server::error::AppState;
    use tracing::info;
    use rustminidb::monitor;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let addr = format!("{}:{}", host, port);

        // 解析数据库文件列表
        let db_names: Vec<String> = dbs.iter().map(|d| {
            let path = std::path::Path::new(d);
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| d.clone())
        }).collect();

        // 打印服务器信息面板（显示第一个数据库）
        let primary_db = db_names.first().cloned().unwrap_or_else(|| "data.db".to_string());
        banner::print_server_info(&host, port, &primary_db);
        banner::print_auth_status(api_token.is_some());

        // 多数据库信息
        if db_names.len() > 1 {
            println!("  Loaded {} databases:", db_names.len());
            for (i, name) in db_names.iter().enumerate() {
                println!("    [{:3}] {}", i + 1, name);
            }
            println!("  (use POST /v1/databases/switch to switch)");
            println!();
        }

        // 如果启用了文件监听，显示通知
        if watch {
            println!("  File watching: ENABLED (auto-reload on file changes)");
            println!();
        }

        // 使用多数据库初始化 AppState
        let state = AppState::new_multi(&db_dir, &db_names, api_token)?;

        // 启动文件监听（可选）
        let _file_watcher = if watch {
            let fw = state.start_file_watcher().ok();
            if fw.is_some() {
                info!("File watcher started for directory: {}", db_dir);
            }
            fw
        } else {
            None
        };

        let app = build_routes(state);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        info!("RustMinidb v{} server starting on {}", rustminidb::version(), addr);
        info!("Database directory: {}", db_dir);
        info!("Databases loaded: {}", db_names.join(", "));
        info!("API endpoints:");
        info!("  GET  /          - Web Admin UI");
        info!("  POST /v1/query  - Execute SQL");
        info!("  GET  /v1/health - Health check");
        info!("  GET  /v1/tables - List tables");
        info!("  GET  /v1/schema/{{table}} - Table schema");
        info!("  GET  /v1/export  - Export database as SQL");

        monitor::record_system("server.start");

        // 打印启动完成消息
        banner::print_startup_complete(Some(&addr));

        axum::serve(listener, app).await?;
        Ok(())
    })
}

/// 非 server feature 下的占位
#[cfg(not(feature = "server"))]
#[allow(unused_variables)]
fn cmd_serve(
    host: String,
    port: u16,
    dbs: Vec<String>,
    db_dir: String,
    watch: bool,
    api_token: Option<String>,
) -> Result<()> {
    eprintln!("错误: 'serve' 命令需要 'server' feature (默认已启用)");
    eprintln!("请使用默认 feature 重新编译: cargo build");
    std::process::exit(1);
}

/// 交互式 SQL Shell（支持多数据库）
fn cmd_shell(dbs: Vec<String>, watch: bool) -> Result<()> {
    use rustminidb::monitor;

    let version = rustminidb::version();
    println!("RustMinidb Shell v{}", version);
    println!("Enter SQL statements or '.help' for help");
    if dbs.len() > 1 {
        println!("Loaded {} databases: {}", dbs.len(), dbs.join(", "));
        println!("Use '.use <db_name>' to switch between databases");
    }
    if watch {
        println!("File watching: ENABLED (auto-reload on file changes)");
    }
    println!();

    // 打开第一个数据库作为默认
    let primary_db = dbs.first().cloned().unwrap_or_else(|| "data.db".to_string());
    let engine = Arc::new(RedbEngine::open(&primary_db)?);
    let executor = Executor::new(engine.clone());

    // 多数据库支持：存储所有已打开的数据库引擎
    let mut db_map: std::collections::HashMap<String, (Arc<dyn StorageEngine>, Executor)> =
        std::collections::HashMap::new();
    db_map.insert(primary_db.clone(), (engine.clone() as Arc<dyn StorageEngine>, Executor::new(engine.clone())));

    // 打开其他数据库
    for db_name in dbs.iter().skip(1) {
        if let Ok(eng) = RedbEngine::open(db_name) {
            let shared: Arc<dyn StorageEngine> = Arc::new(eng);
            let exec = Executor::new(shared.clone());
            db_map.insert(db_name.clone(), (shared, exec));
        }
    }

    let mut current_db_name = primary_db.clone();
    let mut current_engine: Arc<dyn StorageEngine> = engine.clone();
    let mut current_executor = executor;

    // 文件监听（可选）
    let _watcher = if watch {
        let db_dir = std::path::Path::new(&primary_db)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let mut fw = rustminidb::watcher::FileWatcher::new();
        let cb: rustminidb::watcher::ChangeCallback = Arc::new(move |event| {
            // 在 shell 中文件变化通知
            match &event {
                rustminidb::watcher::FileChangeEvent::DatabaseModified { db_name, .. } => {
                    eprintln!("\n[File changed] {} — database modified externally", db_name);
                }
                _ => {}
            }
        });
        let _ = fw.watch(&db_dir, cb);
        Some(fw)
    } else {
        None
    };

    // 简易交互式 shell（不依赖 rustyline，使用标准输入）
    use std::io::{self, BufRead, Write};

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        print!("> ");
        io::stdout().flush().ok();

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,     // EOF
            Ok(_) => {}
            Err(_) => break,
        }

        let line = line.trim();

        // 处理 . 开头的命令
        if line.starts_with('.') {
            match line {
                ".exit" | ".quit" => {
                    monitor::record_system("shell.exit");
                    println!("Bye!");
                    break;
                }
                ".tables" => {
                    match current_engine.list_tables() {
                        Ok(tables) => {
                            if tables.is_empty() {
                                println!("No tables found");
                            } else {
                                println!("Tables in '{}':", current_db_name);
                                for t in tables {
                                    println!("  {}", t);
                                }
                            }
                        }
                        Err(e) => {
                            monitor::record_error("list_tables", &e.to_string());
                            println!("Error: {}", e);
                        }
                    }
                }
                ".help" => {
                    println!("RustMinidb Shell Commands:");
                    println!("  .tables      List all tables in current database");
                    println!("  .schema      Show table schemas");
                    println!("  .databases   List all loaded databases");
                    println!("  .use <name>  Switch to another database");
                    println!("  .exit        Exit shell");
                    println!("  .quit        Exit shell");
                    println!("  .help        Show this help");
                    println!("  .monitor     Show runtime metrics");
                    println!("  .export      Export database to SQL (stdout)");
                    println!();
                    println!("SQL statements end without semicolon requirement.");
                }
                ".monitor" => {
                    monitor::print_metrics_summary();
                }
                ".databases" => {
                    println!("Current database: {}", current_db_name);
                    println!("Loaded databases:");
                    for name in db_map.keys() {
                        let marker = if *name == current_db_name { "* " } else { "  " };
                        println!("  {}{}", marker, name);
                    }
                }
                _ if line.starts_with(".use ") => {
                    let target = line[5..].trim();
                    if target.is_empty() {
                        println!("Usage: .use <database_name>");
                    } else if let Some((eng, exec)) = db_map.get(target) {
                        current_db_name = target.to_string();
                        current_engine = eng.clone();
                        current_executor = exec.clone();
                        println!("Switched to database: {}", target);
                    } else {
                        // 尝试从文件打开
                        match RedbEngine::open(target) {
                            Ok(eng) => {
                                let shared: Arc<dyn StorageEngine> = Arc::new(eng);
                                let exec = Executor::new(shared.clone());
                                current_db_name = target.to_string();
                                current_engine = shared.clone();
                                current_executor = exec.clone();
                                                            db_map.insert(target.to_string(), (shared, exec));
                                println!("Opened and switched to database: {}", target);
                            }
                            Err(e) => {
                                println!("Error: cannot open database '{}': {}", target, e);
                            }
                        }
                    }
                }
                ".export" => {
                    // Shell 内快速导出
                    let engine_ref = current_engine.clone();
                    match rustminidb::migration::export_database_to_string(engine_ref) {
                        Ok(sql) => {
                            println!("═══ Database Export ({}) ═══", current_db_name);
                            println!("{}", sql);
                        }
                        Err(e) => {
                            println!("Export error: {}", e);
                        }
                    }
                }
                ".schema" => {
                    match current_engine.list_tables() {
                        Ok(tables) => {
                            for t in tables {
                                if let Ok(Some(schema)) = current_engine.get_schema(&t) {
                                    println!("CREATE TABLE {} (", schema.name);
                                    for col in &schema.columns {
                                        print!("  {} {}", col.name, col.col_type);
                                        if col.is_primary_key {
                                            print!(" PRIMARY KEY");
                                        }
                                        if col.nullable {
                                            print!(" NULL");
                                        } else if !col.is_primary_key {
                                            print!(" NOT NULL");
                                        }
                                        println!(",");
                                    }
                                    println!(");");
                                    println!();
                                }
                            }
                        }
                        Err(e) => {
                            monitor::record_error("schema", &e.to_string());
                            println!("Error: {}", e);
                        }
                    }
                }
                _ => println!("Unknown command: {}. Type .help", line),
            }
            continue;
        }

        if line.is_empty() {
            continue;
        }

        // 执行 SQL
        let start = std::time::Instant::now();
        match SqlParser::parse(line) {
            Ok(stmt) => match current_executor.execute(&stmt) {
                Ok(result) => {
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    let elapsed_us = (elapsed * 1000.0) as u64;
                    match result {
                        ExecuteResult::QueryResult {
                            columns,
                            rows,
                            rows_affected,
                        } => {
                            monitor::record_query(line, elapsed_us, rows.len() as u64);
                            print_query_result(&columns, &rows, rows_affected, elapsed);
                        }
                        ExecuteResult::WriteResult {
                            rows_affected,
                            last_insert_id,
                        } => {
                            monitor::record_write(line, elapsed_us, rows_affected as u64);

                            if let Some(id) = last_insert_id {
                                println!(
                                    "OK ({} row(s) affected, last_insert_id: {}, {:.2}ms)",
                                    rows_affected, id, elapsed
                                );
                            } else {
                                println!(
                                    "OK ({} row(s) affected, {:.2}ms)",
                                    rows_affected, elapsed
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    monitor::record_error("execute", &e.to_string());
                    println!("Error: {}", e);
                }
            },
            Err(e) => {
                monitor::record_error("parse", &e.to_string());
                println!("Parse Error: {}", e);
            }
        }
    }

    Ok(())
}

/// 执行单条 SQL
fn cmd_exec(db: String, sql: String, format: String) -> Result<()> {
    use rustminidb::monitor;

    let engine = Arc::new(RedbEngine::open(&db)?);
    let executor = Executor::new(engine.clone());

    let start = std::time::Instant::now();
    let stmt = SqlParser::parse(&sql)?;
    let result = executor.execute(&stmt)?;
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let elapsed_us = (elapsed * 1000.0) as u64;

    // 记录操作到监控
    match &result {
        ExecuteResult::QueryResult { rows, .. } => {
            monitor::record_query(&sql, elapsed_us, rows.len() as u64);
        }
        ExecuteResult::WriteResult { rows_affected, .. } => {
            monitor::record_write(&sql, elapsed_us, *rows_affected as u64);
        }
    }

    match format.as_str() {
        "json" => {
            // JSON 输出
            let json_result = match result {
                ExecuteResult::QueryResult {
                    columns,
                    rows,
                    rows_affected,
                } => {
                    let json_rows: Vec<Vec<serde_json::Value>> = rows
                        .into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|v| v.to_json_value())
                                .collect()
                        })
                        .collect();
                    serde_json::json!({
                        "success": true,
                        "data": {
                            "columns": columns,
                            "rows": json_rows,
                            "rowsAffected": rows_affected,
                            "timeMs": elapsed
                        }
                    })
                }
                ExecuteResult::WriteResult {
                    rows_affected,
                    last_insert_id,
                } => {
                    let mut data = serde_json::json!({
                        "rowsAffected": rows_affected,
                        "timeMs": elapsed
                    });
                    if let Some(id) = last_insert_id {
                        data["lastInsertId"] = serde_json::json!(id);
                    }
                    serde_json::json!({
                        "success": true,
                        "data": data
                    })
                }
            };
            println!("{}", serde_json::to_string_pretty(&json_result).unwrap());
        }
        _ => {
            // 表格式输出（默认）
            match result {
                ExecuteResult::QueryResult {
                    columns,
                    rows,
                    rows_affected,
                } => {
                    print_query_result(&columns, &rows, rows_affected, elapsed);
                }
                ExecuteResult::WriteResult {
                    rows_affected,
                    last_insert_id,
                } => {
                    if let Some(id) = last_insert_id {
                        println!("OK ({} row(s) affected, last_insert_id: {}, {:.2}ms)", rows_affected, id, elapsed);
                    } else {
                        println!("OK ({} row(s) affected, {:.2}ms)", rows_affected, elapsed);
                    }
                }
            }
        }
    }

    Ok(())
}

/// 初始化数据库
fn cmd_init(db: String) -> Result<()> {
    rustminidb::monitor::record_system(&format!("init.{}", db));
    let engine = Arc::new(RedbEngine::open(&db)?);
    println!("Database initialized: {}", db);
    println!("Storage engine: redb (ACID, single-file)");

    // 记录数据库文件大小
    if let Ok(size) = rustminidb::monitor::db_file_size(&db) {
        println!("Database size: {}", rustminidb::monitor::format_file_size(size));
    }

    drop(engine);
    Ok(())
}

/// 显示版本信息
fn cmd_version() -> Result<()> {
    println!("RustMinidb v{}", rustminidb::version());
    println!("A lightweight embedded database with native REST API");
    println!("License: BSL-1.1");
    println!("Storage: redb (single-file, ACID)");
    println!();
    println!("Build features:");
    #[cfg(feature = "server")]
    println!("  ✓ server (REST API via axum)");
    #[cfg(not(feature = "server"))]
    println!("  ✗ server (REST API not enabled)");
    println!();
    println!("Homepage: https://rustminidb.dev");
    Ok(())
}

/// 导出数据库
fn cmd_export(
    db: String,
    output: Option<String>,
    table: Option<String>,
    include_data: bool,
    include_create: bool,
) -> Result<()> {
    use std::io::Write;

    let engine = Arc::new(RedbEngine::open(&db)?) as rustminidb::storage::engine::SharedEngine;

    // 构建带用户配置的导出器
    let config = rustminidb::migration::ExportConfig {
        include_data,
        include_create,
        ..Default::default()
    };
    let exporter = rustminidb::migration::Exporter::with_config(engine, config);

    let sql = if let Some(table_name) = table {
        exporter.export_table_to_string(&table_name)?
    } else {
        exporter.export_to_string()?
    };

    match output {
        Some(path) => {
            std::fs::write(&path, &sql)?;
            println!("Export complete: {} ({} bytes)", path, sql.len());
        }
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(sql.as_bytes())?;
            handle.flush()?;
        }
    }

    Ok(())
}

// ── 辅助函数 ──

/// 表格式打印查询结果（shell 和 exec 命令共用）
fn print_query_result(columns: &[String], rows: &[Vec<rustminidb::sql::types::Value>], rows_affected: usize, elapsed_ms: f64) {
    if columns.is_empty() {
        println!("Query set ({:.2}ms)", elapsed_ms);
        return;
    }

    let mut col_widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, val) in row.iter().enumerate() {
            if i < col_widths.len() {
                let val_str = val_display(val);
                col_widths[i] = col_widths[i].max(val_str.len());
            }
        }
    }

    print_separator(&col_widths);
    print_row(columns, &col_widths);
    print_separator(&col_widths);
    for row in rows {
        let strs: Vec<String> = row.iter().map(|v| val_display(v)).collect();
        print_row(&strs, &col_widths);
    }
    print_separator(&col_widths);

    println!("{} row(s) in set ({:.2}ms)", rows_affected, elapsed_ms);
}

fn val_display(val: &rustminidb::sql::types::Value) -> String {
    match val {
        rustminidb::sql::types::Value::Integer(v) => v.to_string(),
        rustminidb::sql::types::Value::Float(v) => {
            if *v == v.trunc() {
                format!("{}.0", v)
            } else {
                format!("{}", v)
            }
        }
        rustminidb::sql::types::Value::Text(v) => v.clone(),
        rustminidb::sql::types::Value::Blob(v) => format!("<blob {} bytes>", v.len()),
        rustminidb::sql::types::Value::Boolean(v) => {
            if *v { "true".into() } else { "false".into() }
        }
        rustminidb::sql::types::Value::Timestamp(v) => {
            chrono::DateTime::from_timestamp_micros(*v)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "invalid".to_string())
        }
        rustminidb::sql::types::Value::Null => "NULL".into(),
    }
}

fn print_separator(widths: &[usize]) {
    print!("+");
    for w in widths {
        print!("{:-<width$}+", "", width = w + 2);
    }
    println!();
}

fn print_row(strs: &[String], widths: &[usize]) {
    print!("|");
    for (i, s) in strs.iter().enumerate() {
        if i < widths.len() {
            print!(" {: <width$} |", s, width = widths[i]);
        }
    }
    println!();
}