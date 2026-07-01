//! RustMinidb 命令行入口
//!
//! 支持子命令：
//! - serve: 启动 HTTP 服务器
//! - shell: 交互式 SQL 控制台
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
            max_connections: _,
            api_token,
        } => cmd_serve(host, port, db, api_token),
        Commands::Shell { db } => cmd_shell(db),
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
fn cmd_serve(host: String, port: u16, db: String, api_token: Option<String>) -> Result<()> {
    use rustminidb::banner;
    use rustminidb::server::build_routes;
    use rustminidb::server::error::AppState;
    use tracing::info;
    use rustminidb::monitor;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let addr = format!("{}:{}", host, port);
        let db_path = std::path::Path::new(&db);
        let db_dir = if let Some(parent) = db_path.parent() {
            let p = parent.to_string_lossy().to_string();
            if p.is_empty() { std::env::current_dir().unwrap_or_default().to_string_lossy().to_string() } else { p }
        } else {
            std::env::current_dir().unwrap_or_default().to_string_lossy().to_string()
        };
        let db_name = db_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "data.db".to_string());

        // 打印服务器信息面板
        banner::print_server_info(&host, port, &db_name);
        banner::print_auth_status(api_token.is_some());

        let state = AppState::new(&db_dir, &db_name, api_token)?;
        let app = build_routes(state);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        info!("RustMinidb v{} server starting on {}", rustminidb::version(), addr);
        info!("Database directory: {}", db_dir);
        info!("Current database: {}", db_name);
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
fn cmd_serve(_host: String, _port: u16, _db: String, _api_token: Option<String>) -> Result<()> {
    eprintln!("错误: 'serve' 命令需要 'server' feature (默认已启用)");
    eprintln!("请使用默认 feature 重新编译: cargo build");
    std::process::exit(1);
}

/// 交互式 SQL Shell
fn cmd_shell(db: String) -> Result<()> {
    use rustminidb::monitor;

    println!("RustMinidb Shell v{}", rustminidb::version());
    println!("Enter SQL statements or '.help' for help");
    println!();

    let engine = Arc::new(RedbEngine::open(&db)?);
    let executor = Executor::new(engine.clone());

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
                    match engine.list_tables() {
                        Ok(tables) => {
                            if tables.is_empty() {
                                println!("No tables found");
                            } else {
                                println!("Tables:");
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
                    println!("  .tables      List all tables");
                    println!("  .schema      Show table schemas");
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
                ".export" => {
                    // Shell 内快速导出
                    let engine_ref = engine.clone() as rustminidb::storage::engine::SharedEngine;
                    match rustminidb::migration::export_database_to_string(engine_ref) {
                        Ok(sql) => {
                            println!("═══ Database Export ═══");
                            println!("{}", sql);
                        }
                        Err(e) => {
                            println!("Export error: {}", e);
                        }
                    }
                }
                ".schema" => {
                    match engine.list_tables() {
                        Ok(tables) => {
                            for t in tables {
                                if let Ok(Some(schema)) = engine.get_schema(&t) {
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
            Ok(stmt) => match executor.execute(&stmt) {
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

                            if !columns.is_empty() {
                                // 表格式输出
                                let mut col_widths: Vec<usize> = columns
                                    .iter()
                                    .map(|c| c.len())
                                    .collect();

                                for row in &rows {
                                    for (i, val) in row.iter().enumerate() {
                                        if i < col_widths.len() {
                                            let val_str = format!("{}", val_display(val));
                                            col_widths[i] = col_widths[i].max(val_str.len());
                                        }
                                    }
                                }

                                // 分隔线
                                print_separator(&col_widths);
                                // 表头
                                print_row(&columns, &col_widths);
                                print_separator(&col_widths);
                                // 数据行
                                for row in &rows {
                                    let strs: Vec<String> = row
                                        .iter()
                                        .map(|v| val_display(v))
                                        .collect();
                                    print_row(&strs, &col_widths);
                                }
                                print_separator(&col_widths);

                                println!(
                                    "{} row(s) in set ({:.2}ms)",
                                    rows_affected, elapsed
                                );
                            } else {
                                println!("Query set ({:.2}ms)", elapsed);
                            }
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
                    if columns.is_empty() {
                        println!("Query set ({:.2}ms)", elapsed);
                    } else {
                        let mut col_widths: Vec<usize> =
                            columns.iter().map(|c| c.len()).collect();
                        for row in &rows {
                            for (i, val) in row.iter().enumerate() {
                                if i < col_widths.len() {
                                    let vs = format!("{}", val_display(val));
                                    col_widths[i] = col_widths[i].max(vs.len());
                                }
                            }
                        }
                        print_separator(&col_widths);
                        print_row(&columns, &col_widths);
                        print_separator(&col_widths);
                        for row in &rows {
                            let strs: Vec<String> =
                                row.iter().map(|v| val_display(v)).collect();
                            print_row(&strs, &col_widths);
                        }
                        print_separator(&col_widths);
                        println!(
                            "{} row(s) in set ({:.2}ms)",
                            rows_affected, elapsed
                        );
                    }
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
    _include_data: bool,
    _include_create: bool,
) -> Result<()> {
    use std::io::Write;

    let engine = Arc::new(RedbEngine::open(&db)?) as rustminidb::storage::engine::SharedEngine;

    let sql = if let Some(table_name) = table {
        let exporter = rustminidb::migration::Exporter::new(engine);
        exporter.export_table_to_string(&table_name)?
    } else {
        rustminidb::migration::export_database_to_string(engine)?
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
