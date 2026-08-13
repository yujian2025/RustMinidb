//! RustMinidb 性能基准测试
//!
//! 提供基本的性能基线：
//! - INSERT 写入吞吐量
//! - 主键点查延迟
//! - 全表扫描延迟
//!
//! 运行: cargo bench

use std::sync::Arc;
use std::time::Instant;

use rustminidb::error::Result;
use rustminidb::sql::executor::{ExecuteResult, Executor};
use rustminidb::sql::parser::SqlParser;
use rustminidb::sql::types::Value;
use rustminidb::storage::redb_engine::RedbEngine;
use tempfile::TempDir;

fn setup_executor() -> (TempDir, Executor) {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(RedbEngine::open(dir.path().join("bench.db")).unwrap());
    let executor = Executor::new(engine);
    (dir, executor)
}

fn print_separator(title: &str) {
    println!("\n{}", "=".repeat(60));
    println!("  {}", title);
    println!("{}", "=".repeat(60));
}

fn print_result(name: &str, elapsed_ns: u128, count: usize) {
    let total_ms = elapsed_ns as f64 / 1_000_000.0;
    let avg_us = elapsed_ns as f64 / count as f64 / 1_000.0;
    let ops = if total_ms > 0.0 {
        (count as f64 / total_ms * 1000.0) as u64
    } else {
        0
    };
    println!(
        "  {:<30} {:>8} ops/sec  {:>8.2} µs/op  {:>8.2} ms total",
        name, ops, avg_us, total_ms
    );
}

/// 批量 INSERT 基准测试
fn bench_insert(count: usize) -> Result<()> {
    let (_dir, executor) = setup_executor();

    // 建表
    executor
        .execute(&SqlParser::parse(
            "CREATE TABLE bench (id INT PRIMARY KEY, name TEXT, value FLOAT)",
        )?)
        .unwrap();

    let start = Instant::now();
    for i in 0..count {
        let sql = format!("INSERT INTO bench VALUES ({}, 'test_data_{}', {}.5)", i, i, i);
        executor
            .execute(&SqlParser::parse(&sql)?)
            .unwrap();
    }
    let elapsed = start.elapsed().as_nanos();

    print_result(
        &format!("INSERT {} rows", count),
        elapsed,
        count,
    );
    Ok(())
}

/// 主键点查基准测试
fn bench_point_lookup(count: usize) -> Result<()> {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse(
            "CREATE TABLE bench (id INT PRIMARY KEY, name TEXT, value FLOAT)",
        )?)
        .unwrap();

    // 准备数据
    for i in 0..count {
        let sql = format!("INSERT INTO bench VALUES ({}, 'data_{}', {}.0)", i, i, i);
        executor.execute(&SqlParser::parse(&sql)?).unwrap();
    }

    let start = Instant::now();
    for i in 0..count {
        let sql = format!("SELECT * FROM bench WHERE id = {}", i);
        let result = executor.execute(&SqlParser::parse(&sql)?)?;
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("Expected QueryResult"),
        }
    }
    let elapsed = start.elapsed().as_nanos();

    print_result(
        &format!("PointLookup {} rows", count),
        elapsed,
        count,
    );
    Ok(())
}

/// 全表扫描基准测试
fn bench_full_scan(count: usize) -> Result<()> {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse(
            "CREATE TABLE bench (id INT PRIMARY KEY, name TEXT, value FLOAT)",
        )?)
        .unwrap();

    // 准备数据
    for i in 0..count {
        let sql = format!("INSERT INTO bench VALUES ({}, 'data_{}', {}.0)", i, i, i);
        executor.execute(&SqlParser::parse(&sql)?).unwrap();
    }

    let start = Instant::now();
    let result = executor.execute(&SqlParser::parse("SELECT * FROM bench")?)?;
    let elapsed = start.elapsed().as_nanos();

    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), count);
        }
        _ => panic!("Expected QueryResult"),
    }

    print_result(
        &format!("FullScan {} rows", count),
        elapsed,
        count,
    );
    Ok(())
}

/// 聚合查询基准测试
fn bench_aggregate(count: usize) -> Result<()> {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse(
            "CREATE TABLE bench (id INT PRIMARY KEY, cat TEXT, val INT)",
        )?)
        .unwrap();

    // 准备数据
    for i in 0..count {
        let cat = if i % 2 == 0 { "a" } else { "b" };
        let sql = format!("INSERT INTO bench VALUES ({}, '{}', {})", i, cat, i);
        executor.execute(&SqlParser::parse(&sql)?).unwrap();
    }

    let start = Instant::now();

    // COUNT(*)
    let r1 = executor.execute(&SqlParser::parse("SELECT COUNT(*) FROM bench")?)?;
    // AVG(val)
    let _r2 = executor.execute(&SqlParser::parse("SELECT AVG(val) FROM bench")?)?;
    // GROUP BY
    let _r3 = executor.execute(&SqlParser::parse("SELECT cat, COUNT(*) FROM bench GROUP BY cat")?)?;

    let elapsed = start.elapsed().as_nanos();

    match r1 {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows[0][0], Value::Integer(count as i64));
        }
        _ => panic!("Expected QueryResult"),
    }

    print_result(
        &format!("Aggregate {} rows (COUNT+AVG+GROUPBY)", count),
        elapsed,
        3,
    );
    Ok(())
}

fn main() {
    println!("RustMinidb 性能基准测试");
    println!("版本: {}", rustminidb::version());
    println!("日期: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));

    // 小规模测试
    print_separator("小规模测试 (100 rows)");
    bench_insert(100).unwrap();
    bench_point_lookup(100).unwrap();
    bench_full_scan(100).unwrap();
    bench_aggregate(100).unwrap();

    // 中规模测试
    print_separator("中规模测试 (1000 rows)");
    bench_insert(1000).unwrap();
    bench_point_lookup(1000).unwrap();
    bench_full_scan(1000).unwrap();
    bench_aggregate(1000).unwrap();

    // 大规模测试
    print_separator("大规模测试 (10000 rows)");
    bench_insert(10000).unwrap();
    bench_point_lookup(10000).unwrap();
    bench_full_scan(10000).unwrap();
    bench_aggregate(10000).unwrap();

    println!("\n{}", "=".repeat(60));
    println!("  基准测试完成");
    println!("{}", "=".repeat(60));
}
