//! RustMinidb 快速入门示例
//!
//! 展示嵌入式 API 的完整 CRUD 操作。
//! 运行: cargo run --example quickstart

use rustminidb::Database;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== RustMinidb Quickstart ===");
    println!();

    // 1. 打开（或创建）数据库
    let db = Database::open("quickstart.db")?;
    println!("✅ Database opened: quickstart.db");

    // 2. 创建表
    db.execute(
        "CREATE TABLE sensors (
            id INT PRIMARY KEY,
            name TEXT,
            value FLOAT,
            active BOOLEAN,
            ts TIMESTAMP
        )",
    )?;
    println!("✅ Table created: sensors");

    // 3. 插入数据
    db.execute("INSERT INTO sensors VALUES (1, 'temperature', 25.6, true, '2026-06-30T10:00:00Z')")?;
    db.execute("INSERT INTO sensors VALUES (2, 'humidity', 60.5, true, '2026-06-30T10:00:01Z')")?;
    db.execute("INSERT INTO sensors VALUES (3, 'pressure', 1013.25, true, '2026-06-30T10:00:02Z')")?;
    db.execute("INSERT INTO sensors VALUES (4, 'light', 450.0, false, '2026-06-30T10:00:03Z')")?;
    println!("✅ Inserted 4 sensor records");

    // 4. 查询所有数据
    println!();
    println!("--- All Sensors ---");
    let rows = db.query("SELECT * FROM sensors")?;
    for row in &rows {
        println!("  {:?}", row);
    }

    // 5. 带条件的查询
    println!();
    println!("--- Active Sensors (value > 30) ---");
    let rows = db.query("SELECT name, value FROM sensors WHERE active = true AND value > 30")?;
    for row in &rows {
        println!("  {:?}", row);
    }

    // 6. ORDER BY + LIMIT
    println!();
    println!("--- Top 2 by Value (DESC) ---");
    let rows = db.query("SELECT name, value FROM sensors ORDER BY value DESC LIMIT 2")?;
    for row in &rows {
        println!("  {:?}", row);
    }

    // 7. 更新数据
    db.execute("UPDATE sensors SET value = 26.8 WHERE id = 1")?;
    println!();
    println!("✅ Updated sensor 1 value to 26.8");

    // 8. 删除数据
    db.execute("DELETE FROM sensors WHERE id = 4")?;
    println!("✅ Deleted sensor 4");

    // 9. 验证
    println!();
    println!("--- Final State ---");
    let rows = db.query("SELECT * FROM sensors ORDER BY id")?;
    for row in &rows {
        println!("  {:?}", row);
    }
    println!("Total rows: {}", rows.len());

    // 10. 清理
    db.execute("DROP TABLE sensors")?;
    println!();
    println!("✅ Cleanup completed");

    // 删除示例数据库文件
    std::fs::remove_file("quickstart.db").ok();
    println!("✅ Cleaned up quickstart.db");

    println!();
    println!("=== Quickstart Complete! ===");

    Ok(())
}
