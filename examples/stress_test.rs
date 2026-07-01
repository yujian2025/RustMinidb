//! RustMinidb 压力测试工具
//!
//! 创建500张表（250张100列，250张10列）
//! 150张表插入10000+条随机数据
//! 记录性能指标，可反复使用
//!
//! 用法: cargo run --example stress_test [url]

use std::time::Instant;

const SERVER: &str = "http://localhost:8080";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::args().nth(1).unwrap_or_else(|| SERVER.to_string());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    println!("╔══════════════════════════════════════════╗");
    println!("║     RustMinidb 压力测试                   ║");
    println!("║     服务器: {:<30}║", base);
    println!("╚══════════════════════════════════════════╝");
    println!();

    let total_start = Instant::now();

    // ═══════════════════════════════════════
    // 阶段1: 清理旧数据
    // ═══════════════════════════════════════
    println!("[阶段0] 清理旧测试数据...");
    for i in 1..=500 {
        query(&client, &base, &format!("DROP TABLE IF EXISTS stress_{}", i)).await.ok();
        if i % 100 == 0 { println!("  清理进度: {}/500", i); }
    }
    println!("✅ 清理完成\n");

    // ═══════════════════════════════════════
    // 阶段1: 创建 500 张表
    // ═══════════════════════════════════════
    println!("═══ 阶段1: 创建 500 张表 ═══");
    let phase1_start = Instant::now();

    for i in 1..=500 {
        let sql = if i <= 250 {
            // 250张100列大表
            let cols: Vec<String> = (1..100).map(|j| format!("col_{} INT", j)).collect();
            format!("CREATE TABLE stress_{} (id INT PRIMARY KEY, {})", i, cols.join(", "))
        } else {
            // 250张10列小表
            let cols: Vec<String> = (1..10).map(|j| format!("col_{} TEXT", j)).collect();
            format!("CREATE TABLE stress_{} (id INT PRIMARY KEY, {})", i, cols.join(", "))
        };

        match query(&client, &base, &sql).await {
            Ok(true) => {},
            Ok(false) => { eprintln!("  建表失败: stress_{}", i); }
            Err(e) => { eprintln!("  建表错误: stress_{}: {}", i, e); }
        }

        if i % 50 == 0 { println!("  建表进度: {}/500 ({})", i, i*100/500); }
    }
    let phase1_elapsed = phase1_start.elapsed();
    println!("✅ 阶段1完成: 500张表已创建 (耗时: {:?})\n", phase1_elapsed);

    // ═══════════════════════════════════════
    // 阶段2: 插入数据（150张表各10000+条）
    // ═══════════════════════════════════════
    println!("═══ 阶段2: 插入随机数据 ═══");
    let phase2_start = Instant::now();
    let mut total_rows = 0usize;
    let mut total_batches = 0usize;

    for table_id in 1..=150 {
        let row_count = 10000 + (table_id as u64 * 137) % 5000; // 10000~15000
        let batch_size = 200;
        let batches = (row_count as usize + batch_size - 1) / batch_size;
        let is_wide = table_id <= 250; // 前250张是宽表

        for batch_idx in 0..batches {
            let start_row = batch_idx * batch_size + 1;
            let end_row = std::cmp::min(start_row + batch_size - 1, row_count as usize);

            let mut sql = if is_wide {
                let cols: Vec<String> = (1..100).map(|j| format!("col_{}", j)).collect();
                format!("INSERT INTO stress_{} (id, {}) VALUES ", table_id, cols.join(", "))
            } else {
                let cols: Vec<String> = (1..10).map(|j| format!("col_{}", j)).collect();
                format!("INSERT INTO stress_{} (id, {}) VALUES ", table_id, cols.join(", "))
            };

            let mut first = true;
            for row_id in start_row..=end_row {
                if !first { sql.push_str(","); }
                first = false;
                sql.push_str(&format!("({}", row_id));
                if is_wide {
                    for _ in 1..100 { sql.push_str(&format!(",{}", fastrand::i32(0..10000))); }
                } else {
                    for _ in 1..10 { sql.push_str(&format!(",'val_{}'", fastrand::i32(0..9999))); }
                }
                sql.push_str(")");
            }

            match query(&client, &base, &sql).await {
                Ok(true) => {
                    total_rows += end_row - start_row + 1;
                    total_batches += 1;
                }
                Ok(false) => { eprintln!("  插入失败: stress_{} batch {}", table_id, batch_idx); }
                Err(e) => { eprintln!("  插入错误: stress_{}: {}", table_id, e); }
            }
        }

        if table_id % 15 == 0 {
            let pct = table_id * 100 / 150;
            println!("  插入进度: {}/150 ({}%) - {} rows so far", table_id, pct, total_rows);
        }
    }
    let phase2_elapsed = phase2_start.elapsed();
    println!("✅ 阶段2完成: {} 行数据插入 (耗时: {:?})\n", total_rows, phase2_elapsed);

    // ═══════════════════════════════════════
    // 阶段3: 查询性能测试
    // ═══════════════════════════════════════
    println!("═══ 阶段3: 查询性能测试 ═══");
    let phase3_start = Instant::now();
    let mut query_ok = 0u32;
    let mut query_fail = 0u32;
    let mut query_times = Vec::new();

    for test_idx in 0..100 {
        let table_id = fastrand::u32(1..=150);
        let pk = fastrand::u32(1..=10000);
        let sql = format!("SELECT * FROM stress_{} WHERE id = {}", table_id, pk);

        let qstart = Instant::now();
        match query(&client, &base, &sql).await {
            Ok(true) => { query_ok += 1; query_times.push(qstart.elapsed()); }
            _ => { query_fail += 1; }
        }

        if test_idx % 20 == 19 {
            println!("  查询进度: {}/100", test_idx + 1);
        }
    }
    let phase3_elapsed = phase3_start.elapsed();

    // 计算查询延迟统计
    query_times.sort();
    let p50 = query_times.get(query_times.len() / 2).map(|d| d.as_micros()).unwrap_or(0);
    let p90 = query_times.get(query_times.len() * 9 / 10).map(|d| d.as_micros()).unwrap_or(0);
    let p99 = query_times.get(query_times.len() * 99 / 100).map(|d| d.as_micros()).unwrap_or(0);
    let avg: u128 = if !query_times.is_empty() {
        query_times.iter().map(|d| d.as_micros()).sum::<u128>() / query_times.len() as u128
    } else { 0 };

    println!("✅ 阶段3完成\n");

    // ═══════════════════════════════════════
    // 报告
    // ═══════════════════════════════════════
    let total_elapsed = total_start.elapsed();
    let total_secs = total_elapsed.as_secs_f64();

    let report = format!(
        r#"
╔══════════════════════════════════════════╗
║         RustMinidb 压力测试报告            ║
╠══════════════════════════════════════════╣
║  创建的表:          500                   ║
║  大表(100列):       250                   ║
║  小表(10列):        250                   ║
║  含数据表:          150                   ║
║  总数据行数:        {}                    ║
║  总耗时:            {:.1} 秒              ║
╠══════════════════════════════════════════╣
║  建表耗时:          {:?}                  ║
║  插入耗时:          {:?}                  ║
║  插入批次:          {}                    ║
║  插入速率:          {:.0} 行/秒           ║
╠══════════════════════════════════════════╣
║  查询测试:          100 次                ║
║  查询成功:          {}                    ║
║  查询失败:          {}                    ║
║  平均延迟:          {} μs                ║
║  P50 延迟:          {} μs                ║
║  P90 延迟:          {} μs                ║
║  P99 延迟:          {} μs                ║
╠══════════════════════════════════════════╣
║  服务器:            {:<30}║
║  测试时间:          {}          ║
╚══════════════════════════════════════════╝
"#,
        total_rows,
        total_secs,
        phase1_elapsed,
        phase2_elapsed,
        total_batches,
        total_rows as f64 / total_secs.max(0.001),
        query_ok,
        query_fail,
        avg,
        p50,
        p90,
        p99,
        base,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
    );

    println!("{}", report);

    // 保存报告
    std::fs::write("stress_test_report.txt", &report).ok();
    println!("📝 报告已保存: stress_test_report.txt\n");

    // 输出服务器最终状态
    println!("═══ 服务器最终状态 ═══");
    if let Ok(body) = client.get(&format!("{}/v1/health", base)).send().await {
        println!("{}", body.text().await.unwrap_or_default());
    }
    println!("\n✅ 压力测试全部完成!");

    Ok(())
}

/// 执行 SQL 并判断是否成功
async fn query(client: &reqwest::Client, base: &str, sql: &str) -> Result<bool, reqwest::Error> {
    let body = serde_json::json!({ "sql": sql });
    let resp = client
        .post(&format!("{}/v1/query", base))
        .json(&body)
        .send()
        .await?;
    let result: serde_json::Value = resp.json().await?;
    Ok(result["success"].as_bool().unwrap_or(false))
}
