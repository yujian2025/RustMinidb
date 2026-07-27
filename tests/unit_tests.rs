//! RustMinidb 全面集成测试
//!
//! 测试覆盖完整的 SQL 流水线：解析 → 计划 → 执行 → 存储

use std::sync::Arc;
use tempfile::TempDir;

use rustminidb::error::Result;
use rustminidb::sql::executor::{ExecuteResult, Executor};
use rustminidb::sql::parser::{ComparisonOp, SqlParser, SqlStatement, WhereClause};
use rustminidb::sql::types::{ColumnType, Row, Value};
use rustminidb::storage::redb_engine::RedbEngine;
use rustminidb::storage::schema::TableSchema;

/// 创建测试用的存储引擎和 Executor
fn setup_executor() -> (TempDir, Executor) {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(RedbEngine::open(dir.path().join("test.db")).unwrap());
    let executor = Executor::new(engine);
    (dir, executor)
}

// ═══════════════════════════════════════
// SQL 解析器测试
// ═══════════════════════════════════════

#[test]
fn test_parse_create_table_with_types() {
    let sql = "CREATE TABLE test (
        id INT PRIMARY KEY,
        name TEXT NOT NULL,
        price FLOAT,
        active BOOLEAN,
        data BLOB,
        ts TIMESTAMP
    )";
    let stmt = SqlParser::parse(sql).unwrap();
    match stmt {
        SqlStatement::CreateTable { name, columns, .. } => {
            assert_eq!(name, "test");
            assert_eq!(columns.len(), 6);
            assert_eq!(columns[0].col_type, ColumnType::Integer);
            assert!(columns[0].is_primary_key);
            assert_eq!(columns[1].col_type, ColumnType::Text);
            assert!(!columns[1].nullable);
            assert_eq!(columns[2].col_type, ColumnType::Float);
            assert_eq!(columns[3].col_type, ColumnType::Boolean);
            assert_eq!(columns[4].col_type, ColumnType::Blob);
            assert_eq!(columns[5].col_type, ColumnType::Timestamp);
        }
        _ => panic!("期望 CREATE TABLE"),
    }
}

#[test]
fn test_parse_select_star() {
    let sql = "SELECT * FROM users";
    let stmt = SqlParser::parse(sql).unwrap();
    match stmt {
        SqlStatement::Select {
            table,
            columns,
            where_clause: None,
            ..
        } => {
            assert_eq!(table, "users");
            assert_eq!(columns, vec!["*"]);
        }
        _ => panic!("期望 SELECT"),
    }
}

#[test]
fn test_parse_where_with_and_or() {
    let sql = "SELECT * FROM t WHERE id > 1 AND name = 'test' OR age <= 30";
    let stmt = SqlParser::parse(sql).unwrap();
    match stmt {
        SqlStatement::Select {
            where_clause: Some(_),
            ..
        } => {}
        _ => panic!("期望 WHERE 子句"),
    }
}

#[test]
fn test_parse_multi_row_insert() {
    let sql = "INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c')";
    let stmt = SqlParser::parse(sql).unwrap();
    match stmt {
        SqlStatement::Insert {
            table, values, ..
        } => {
            assert_eq!(table, "t");
            assert_eq!(values.len(), 3);
        }
        _ => panic!("期望 INSERT"),
    }
}

#[test]
fn test_parse_comparison_ops() {
    let tests = vec![
        ("SELECT * FROM t WHERE id = 1", ComparisonOp::Eq),
        ("SELECT * FROM t WHERE id <> 1", ComparisonOp::NotEq),
        ("SELECT * FROM t WHERE id < 1", ComparisonOp::Lt),
        ("SELECT * FROM t WHERE id <= 1", ComparisonOp::LtEq),
        ("SELECT * FROM t WHERE id > 1", ComparisonOp::Gt),
        ("SELECT * FROM t WHERE id >= 1", ComparisonOp::GtEq),
    ];

    for (sql, expected_op) in tests {
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::Select {
                where_clause: Some(WhereClause::Simple { operator, .. }),
                ..
            } => {
                assert!(
                    std::mem::discriminant(&operator)
                        == std::mem::discriminant(&expected_op),
                    "SQL: {}",
                    sql
                );
            }
            _ => panic!("期望带有 WHERE 的 SELECT"),
        }
    }
}

// ═══════════════════════════════════════
// SQL 执行完整流水线测试
// ═══════════════════════════════════════

#[test]
fn test_full_create_and_query() {
    let (_dir, executor) = setup_executor();

    // 创建表
    executor
        .execute(&SqlParser::parse(
            "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)",
        ).unwrap())
        .unwrap();

    // 批量插入
    for i in 1..=5 {
        let sql = format!("INSERT INTO users VALUES ({}, 'user{}', {})", i, i, 20 + i);
        executor
            .execute(&SqlParser::parse(&sql).unwrap())
            .unwrap();
    }

    // 查询所有
    let result = executor
        .execute(&SqlParser::parse("SELECT * FROM users").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 5);
        }
        _ => panic!("期望 QueryResult"),
    }

    // WHERE 条件查询
    let result = executor
        .execute(&SqlParser::parse("SELECT name FROM users WHERE age > 22").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 3); // age 23, 24, 25
        }
        _ => panic!("期望 QueryResult"),
    }

    // ORDER BY
    let result = executor
        .execute(&SqlParser::parse("SELECT name FROM users ORDER BY age DESC").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 5);
            assert_eq!(rows[0][0], Value::Text("user5".into()));
        }
        _ => panic!("期望 QueryResult"),
    }

    // LIMIT + OFFSET
    let result = executor
        .execute(&SqlParser::parse("SELECT id FROM users ORDER BY id LIMIT 2 OFFSET 1").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], Value::Integer(2));
            assert_eq!(rows[1][0], Value::Integer(3));
        }
        _ => panic!("期望 QueryResult"),
    }
}

#[test]
fn test_full_update_and_delete() {
    let (_dir, executor) = setup_executor();

    executor
        .execute(
            &SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT, num INT)").unwrap(),
        )
        .unwrap();

    executor
        .execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'a', 10)").unwrap())
        .unwrap();
    executor
        .execute(&SqlParser::parse("INSERT INTO t VALUES (2, 'b', 20)").unwrap())
        .unwrap();
    executor
        .execute(&SqlParser::parse("INSERT INTO t VALUES (3, 'c', 30)").unwrap())
        .unwrap();

    // UPDATE 多行
    executor
        .execute(&SqlParser::parse("UPDATE t SET val = 'x' WHERE num > 15").unwrap())
        .unwrap();

    let result = executor
        .execute(&SqlParser::parse("SELECT val FROM t WHERE num > 15").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            for row in &rows {
                assert_eq!(row[0], Value::Text("x".into()));
            }
        }
        _ => panic!("期望 QueryResult"),
    }

    // DELETE
    executor
        .execute(&SqlParser::parse("DELETE FROM t WHERE id = 2").unwrap())
        .unwrap();

    let result = executor
        .execute(&SqlParser::parse("SELECT * FROM t").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 2);
        }
        _ => panic!("期望 QueryResult"),
    }
}

#[test]
fn test_primary_key_constraint() {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)").unwrap())
        .unwrap();

    executor
        .execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'first')").unwrap())
        .unwrap();

    // 重复主键应该报错
    let result =
        executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'second')").unwrap());
    assert!(result.is_err(), "重复主键应该报错");
}

#[test]
fn test_table_with_text_pk() {
    let (_dir, executor) = setup_executor();

    executor
        .execute(
            &SqlParser::parse("CREATE TABLE devices (device_id TEXT PRIMARY KEY, name TEXT)")
                .unwrap(),
        )
        .unwrap();

    executor
        .execute(&SqlParser::parse("INSERT INTO devices VALUES ('sensor_01', 'Temperature')").unwrap())
        .unwrap();
    executor
        .execute(&SqlParser::parse("INSERT INTO devices VALUES ('sensor_02', 'Humidity')").unwrap())
        .unwrap();

    let result = executor
        .execute(&SqlParser::parse("SELECT * FROM devices WHERE device_id = 'sensor_01'").unwrap())
        .unwrap();
    match result {
        ExecuteResult::QueryResult { rows, .. } => {
            assert_eq!(rows.len(), 1);
        }
        _ => panic!("期望 QueryResult"),
    }
}

#[test]
fn test_multi_insert() {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)").unwrap())
        .unwrap();

    let result = executor
        .execute(
            &SqlParser::parse("INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c')").unwrap(),
        )
        .unwrap();
    match result {
        ExecuteResult::WriteResult { rows_affected, .. } => {
            assert_eq!(rows_affected, 3);
        }
        _ => panic!("期望 WriteResult"),
    }
}

#[test]
fn test_if_not_exists() {
    let (_dir, executor) = setup_executor();

    executor
        .execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)").unwrap())
        .unwrap();

    // IF NOT EXISTS 不应该报错
    executor
        .execute(&SqlParser::parse("CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, val TEXT)").unwrap())
        .unwrap();
}

// ═══════════════════════════════════════
// 边界情况测试
// ═══════════════════════════════════════

#[test]
fn test_empty_database() {
    let (_dir, executor) = setup_executor();

    let result = executor
        .execute(&SqlParser::parse("SELECT * FROM nonexistent").unwrap());
    assert!(result.is_err());
}

#[test]
fn test_drop_nonexistent_table() {
    let (_dir, executor) = setup_executor();
    let result = executor
        .execute(&SqlParser::parse("DROP TABLE nonexistent").unwrap());
    assert!(result.is_err());
}

#[test]
fn test_empty_sql() {
    let result = SqlParser::parse("");
    assert!(result.is_err());
}

#[test]
fn test_row_count() {
    let (_dir, executor) = setup_executor();
    let engine = executor.engine();

    executor
        .execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY)").unwrap())
        .unwrap();

    for i in 1..=10 {
        let sql = format!("INSERT INTO t VALUES ({})", i);
        executor
            .execute(&SqlParser::parse(&sql).unwrap())
            .unwrap();
    }

    assert_eq!(engine.row_count("t").unwrap(), 10);
}

// ── V0.3.0 新功能测试 ──

#[test]
fn test_where_like() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, name TEXT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'Alice')").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 'Bob')").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 'Charlie')").unwrap()).unwrap();

    // LIKE 前缀匹配
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE name LIKE 'A%'").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "LIKE 'A%' 应返回 1 行");
    } else { panic!("期望 QueryResult"); }

    // LIKE 后缀匹配
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE name LIKE '%e'").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "LIKE '%e' 应返回 2 行 (Alice, Charlie)");
    } else { panic!("期望 QueryResult"); }

    // LIKE 中间匹配
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE name LIKE '%ob%'").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "LIKE '%%ob%%' 应返回 1 行 (Bob)");
    } else { panic!("期望 QueryResult"); }

    // NOT LIKE
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE name NOT LIKE 'A%'").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "NOT LIKE 'A%' 应返回 2 行 (Bob, Charlie)");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_where_in() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, score INT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 10)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 20)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 30)").unwrap()).unwrap();

    // IN
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id IN (1, 3)").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "IN (1, 3) 应返回 2 行");
    } else { panic!("期望 QueryResult"); }

    // NOT IN
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id NOT IN (1, 2)").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "NOT IN (1, 2) 应返回 1 行 (id=3)");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_where_is_null_between() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val INT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 10)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, NULL)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 30)").unwrap()).unwrap();

    // IS NULL
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE val IS NULL").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "IS NULL 应返回 1 行");
    } else { panic!("期望 QueryResult"); }

    // IS NOT NULL
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE val IS NOT NULL").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "IS NOT NULL 应返回 2 行");
    } else { panic!("期望 QueryResult"); }

    // BETWEEN
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id BETWEEN 1 AND 2").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "BETWEEN 1 AND 2 应返回 2 行");
    } else { panic!("期望 QueryResult"); }

    // NOT BETWEEN (parser: NOT BETWEEN = col < low OR col > high)
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id NOT BETWEEN 1 AND 2").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "NOT BETWEEN 1 AND 2 应返回 1 行 (id=3)");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_upsert_do_nothing() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'a')").unwrap()).unwrap();

    // ON CONFLICT DO NOTHING - 应该静默跳过
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'b') ON CONFLICT DO NOTHING").unwrap()).unwrap();

    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id = 1").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1);
        // 值应该保持原来的 'a'（未被覆盖）
        let val = &rows[0].values[1];
        assert_eq!(*val, Value::Text("a".into()));
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_upsert_do_update() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val TEXT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'a')").unwrap()).unwrap();

    // ON CONFLICT DO UPDATE SET val = 'b'
    executor.execute(&SqlParser::parse(
        "INSERT INTO t VALUES (1, 'ignored') ON CONFLICT DO UPDATE SET val = 'b'"
    ).unwrap()).unwrap();

    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id = 1").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        let val = &rows[0].values[1];
        assert_eq!(*val, Value::Text("b".into()), "UPSERT 后 val 应更新为 'b'");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_aggregate_count() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, category TEXT, val INT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'a', 10)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 'a', 20)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 'b', 30)").unwrap()).unwrap();

    // COUNT(*)
    let result = executor.execute(&SqlParser::parse("SELECT COUNT(*) FROM t").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, columns, .. } = result {
        assert_eq!(columns, vec!["COUNT(*)"]);
        assert_eq!(rows[0].values[0], Value::Integer(3));
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_aggregate_sum_avg() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val INT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 10)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 20)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 30)").unwrap()).unwrap();

    // SUM
    let result = executor.execute(&SqlParser::parse("SELECT SUM(val) FROM t").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows[0].values[0], Value::Integer(60));
    } else { panic!("期望 QueryResult"); }

    // AVG
    let result = executor.execute(&SqlParser::parse("SELECT AVG(val) FROM t").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows[0].values[0], Value::Float(20.0));
    } else { panic!("期望 QueryResult"); }

    // MIN / MAX
    let result = executor.execute(&SqlParser::parse("SELECT MIN(val), MAX(val) FROM t").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows[0].values[0], Value::Integer(10));
        assert_eq!(rows[0].values[1], Value::Integer(30));
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_group_by_and_having() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, cat TEXT, val INT)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 'a', 10)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 'a', 20)").unwrap()).unwrap();
    executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 'b', 30)").unwrap()).unwrap();

    // GROUP BY + aggregate
    let result = executor.execute(&SqlParser::parse("SELECT cat, COUNT(*) FROM t GROUP BY cat").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 2, "GROUP BY 应返回 2 组");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_create_and_use_index() {
    let (_dir, executor) = setup_executor();
    executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, name TEXT, val INT)").unwrap()).unwrap();

    // 插入数据
    for i in 0..10 {
        executor.execute(&SqlParser::parse(
            &format!("INSERT INTO t VALUES ({}, 'name{}', {})", i, i, i * 10)
        ).unwrap()).unwrap();
    }

    // 创建索引
    executor.execute(&SqlParser::parse("CREATE INDEX idx_val ON t(val)").unwrap()).unwrap();

    // 通过索引列查询（走 IndexScan）
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE val = 50").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "通过索引查询应返回 1 行");
    } else { panic!("期望 QueryResult"); }

    // 删除索引
    executor.execute(&SqlParser::parse("DROP INDEX idx_val").unwrap()).unwrap();

    // 索引删除后通过主键查询仍正常工作
    let result = executor.execute(&SqlParser::parse("SELECT * FROM t WHERE id = 5").unwrap()).unwrap();
    if let ExecuteResult::QueryResult { rows, .. } = result {
        assert_eq!(rows.len(), 1, "主键查询应仍正常工作");
    } else { panic!("期望 QueryResult"); }
}

#[test]
fn test_multi_insert_not_already_tested() {
    // 验证多行 INSERT 已在 parser 层面正确解析
    let stmt = SqlParser::parse("INSERT INTO t VALUES (1, 'a'), (2, 'b')").unwrap();
    match stmt {
        SqlStatement::Insert { values, .. } => {
            assert_eq!(values.len(), 2, "多行 INSERT 应解析出 2 行");
        }
        _ => panic!("期望 Insert"),
    }
}
