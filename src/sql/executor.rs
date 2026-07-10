//! SQL 执行器
//!
//! 根据 SqlStatement 执行对应的数据库操作。
//! 支持：CREATE TABLE, INSERT, SELECT, UPDATE, DELETE, DROP TABLE。

use std::collections::HashMap;

use crate::error::{ExecError, Result};
use crate::sql::parser::{
    compare_values, ComparisonOp, SqlStatement, WhereClause,
};
use crate::sql::planner::Planner;
use crate::sql::types::{ColumnDef, ColumnType, Row, Value};
use crate::storage::engine::SharedEngine;
use crate::storage::schema::{pk_value, row_from_map, TableSchema};

/// 执行结果
#[derive(Debug)]
pub enum ExecuteResult {
    /// 查询结果
    QueryResult {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        rows_affected: usize,
    },
    /// 写入结果
    WriteResult {
        rows_affected: usize,
        last_insert_id: Option<i64>,
    },
}

/// SQL 执行器
#[derive(Clone)]
pub struct Executor {
    engine: SharedEngine,
}

impl Executor {
    pub fn new(engine: SharedEngine) -> Self {
        Self { engine }
    }

    /// 获取存储引擎引用
    pub fn engine(&self) -> &SharedEngine {
        &self.engine
    }

    /// 执行一条 SQL 语句
    pub fn execute(&self, stmt: &SqlStatement) -> Result<ExecuteResult> {
        match stmt {
            SqlStatement::CreateTable {
                name,
                columns,
                if_not_exists,
            } => self.execute_create(name, columns, *if_not_exists),
            SqlStatement::Insert {
                table,
                columns,
                values,
            } => self.execute_insert(table, columns, values),
            SqlStatement::Select { table, .. } => self.execute_select(stmt, table),
            SqlStatement::Update {
                table,
                assignments,
                where_clause,
            } => self.execute_update(table, assignments, where_clause),
            SqlStatement::Delete {
                table,
                where_clause,
            } => self.execute_delete(table, where_clause),
            SqlStatement::DropTable { name } => self.execute_drop(name),
        }
    }

    fn execute_create(
        &self,
        name: &str,
        columns: &[ColumnDef],
        if_not_exists: bool,
    ) -> Result<ExecuteResult> {
        // 检查表是否已存在
        if self.engine.table_exists(name)? {
            if if_not_exists {
                return Ok(ExecuteResult::WriteResult {
                    rows_affected: 0,
                    last_insert_id: None,
                });
            }
            return Err(ExecError::ConstraintViolation(format!("表 '{}' 已存在", name)).into());
        }

        // 验证：必须有主键
        let pk_count = columns.iter().filter(|c| c.is_primary_key).count();
        if pk_count == 0 {
            return Err(ExecError::Validation("每个表必须有主键".into()).into());
        }
        if pk_count > 1 {
            return Err(ExecError::Validation("MVP 只支持单列主键".into()).into());
        }

        // 验证：主键必须非空
        let pk = columns.iter().find(|c| c.is_primary_key).unwrap();
        if pk.nullable {
            return Err(
                ExecError::Validation(format!("主键列 '{}' 不能为 NULL", pk.name)).into(),
            );
        }

        let schema = TableSchema {
            name: name.to_string(),
            columns: columns.to_vec(),
            primary_key: vec![pk.name.clone()],
            comment: None,
        };

        self.engine.create_table(&schema)?;
        Ok(ExecuteResult::WriteResult {
            rows_affected: 0,
            last_insert_id: None,
        })
    }

    fn execute_insert(
        &self,
        table: &str,
        columns: &[String],
        values: &[Vec<Value>],
    ) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        let mut inserted = 0;
        let mut last_id = None;

        for row_values in values {
            let row = if columns.is_empty() {
                // 未指定列，按 schema 顺序
                Row {
                    values: row_values.clone(),
                }
            } else {
                let mut map = HashMap::new();
                for (col, val) in columns.iter().zip(row_values.iter()) {
                    map.insert(col.clone(), val.clone());
                }
                row_from_map(&schema, &map).map_err(|e| ExecError::Validation(e))?
            };

            // 类型转换尝试
            let coerced = self.coerce_row_values(&schema, &row.values)?;
            let row = Row { values: coerced };

            schema
                .validate_row(&row.values)
                .map_err(|e| ExecError::TypeMismatch(e))?;

            self.engine.insert_row(table, row)?;
            inserted += 1;
        }

        // 如果只有一列整数主键，返回 last insert id
        if let Some(pk_idx) = schema.pk_index() {
            if schema.columns[pk_idx].col_type == ColumnType::Integer {
                if let Some(Value::Integer(id)) = values
                    .last()
                    .and_then(|v| {
                        if columns.is_empty() {
                            v.get(pk_idx)
                        } else {
                            // 按列名找到对应的值
                            let pk_name = &schema.columns[pk_idx].name;
                            columns.iter().position(|c| c == pk_name).and_then(|idx| {
                                values.last().and_then(|v| v.get(idx))
                            })
                        }
                    }) {
                    last_id = Some(*id);
                }
            }
        }

        Ok(ExecuteResult::WriteResult {
            rows_affected: inserted,
            last_insert_id: last_id,
        })
    }

    fn execute_select(&self, stmt: &SqlStatement, table: &str) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        // 生成执行计划
        let plan = Planner::plan_select(stmt, &schema)?;

        // 执行计划
        let rows = self.evaluate_plan(&plan, &schema)?;
        let rows_len = rows.len();

        // 提取列名（从 Projection 节点）
        let columns = Self::extract_columns(&plan, &schema);

        Ok(ExecuteResult::QueryResult {
            columns,
            rows: rows.into_iter().map(|r| r.values).collect(),
            rows_affected: rows_len,
        })
    }

    fn execute_update(
        &self,
        table: &str,
        assignments: &[(String, Value)],
        where_clause: &Option<WhereClause>,
    ) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        // 全表扫描
        let rows = self.engine.scan_table(table)?;

        let mut updated = 0;

        for row in &rows {
            // 检查 WHERE 条件
            if let Some(wc) = where_clause {
                if !self.evaluate_predicate(wc, row, &schema) {
                    continue;
                }
            }

            // 应用更新
            let mut new_values = row.values.clone();
            for (col_name, val) in assignments {
                let col_idx = schema
                    .col_index(col_name)
                    .ok_or_else(|| ExecError::ColumnNotFound(col_name.clone()))?;
                new_values[col_idx] = val.clone();
            }

            // 验证新行
            schema
                .validate_row(&new_values)
                .map_err(|e| ExecError::TypeMismatch(e))?;

            // 主键不能被更新
            if let Some(pk_idx) = schema.pk_index() {
                if assignments.iter().any(|(col, _)| {
                    schema.columns[pk_idx].name == *col
                }) {
                    return Err(ExecError::Validation("不能更新主键列".into()).into());
                }
            }

            let pk = pk_value(row, &schema).unwrap();
            self.engine
                .update_row(table, pk, Row { values: new_values })?;
            updated += 1;
        }

        Ok(ExecuteResult::WriteResult {
            rows_affected: updated,
            last_insert_id: None,
        })
    }

    fn execute_delete(
        &self,
        table: &str,
        where_clause: &Option<WhereClause>,
    ) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        let rows = self.engine.scan_table(table)?;
        let mut deleted = 0;

        for row in &rows {
            if let Some(wc) = where_clause {
                if !self.evaluate_predicate(wc, row, &schema) {
                    continue;
                }
            }

            let pk = pk_value(row, &schema).unwrap();
            self.engine.delete_row(table, pk)?;
            deleted += 1;
        }

        Ok(ExecuteResult::WriteResult {
            rows_affected: deleted,
            last_insert_id: None,
        })
    }

    fn execute_drop(&self, name: &str) -> Result<ExecuteResult> {
        if !self.engine.table_exists(name)? {
            return Err(ExecError::TableNotFound(name.to_string()).into());
        }
        self.engine.drop_table(name)?;
        Ok(ExecuteResult::WriteResult {
            rows_affected: 0,
            last_insert_id: None,
        })
    }

    // ── 执行计划求值 ──

    /// 递归执行计划节点
    fn evaluate_plan(&self, plan: &super::planner::PlanNode, schema: &TableSchema) -> Result<Vec<Row>> {
        use super::planner::PlanNode;
        match plan {
            PlanNode::SeqScan { table } => self.engine.scan_table(table),
            PlanNode::PointLookup { table, pk } => match self.engine.get_row(table, pk)? {
                Some(row) => Ok(vec![row]),
                None => Ok(vec![]),
            },
            PlanNode::Filter { input, predicate } => {
                let rows = self.evaluate_plan(input, schema)?;
                Ok(rows
                    .into_iter()
                    .filter(|row| self.evaluate_predicate(predicate, row, schema))
                    .collect())
            }
            PlanNode::Projection { input, columns } => {
                let rows = self.evaluate_plan(input, schema)?;
                let indices: Vec<usize> = columns
                    .iter()
                    .map(|c| {
                        schema.columns.iter().position(|col| col.name == *c).unwrap_or(0)
                    })
                    .collect();
                Ok(rows
                    .into_iter()
                    .map(|row| Row {
                        values: indices.iter().map(|&i| row.values[i].clone()).collect(),
                    })
                    .collect())
            }
            PlanNode::Sort { input, order_by } => {
                let mut rows = self.evaluate_plan(input, schema)?;
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == order_by.column)
                    .unwrap_or(0);
                rows.sort_by(|a, b| {
                    let cmp = compare_values(&a.values[col_idx], &b.values[col_idx]);
                    if order_by.ascending {
                        cmp
                    } else {
                        cmp.reverse()
                    }
                });
                Ok(rows)
            }
            PlanNode::Limit {
                input,
                limit,
                offset,
            } => {
                let rows = self.evaluate_plan(input, schema)?;
                Ok(rows.into_iter().skip(*offset).take(*limit).collect())
            }
        }
    }

    /// 计算 WHERE 条件
    fn evaluate_predicate(
        &self,
        predicate: &WhereClause,
        row: &Row,
        schema: &TableSchema,
    ) -> bool {
        match predicate {
            WhereClause::Simple {
                column,
                operator,
                value,
            } => {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == *column)
                    .unwrap();
                let row_val = &row.values[col_idx];

                let cmp = compare_values(row_val, value);
                match operator {
                    ComparisonOp::Eq => cmp == std::cmp::Ordering::Equal,
                    ComparisonOp::NotEq => cmp != std::cmp::Ordering::Equal,
                    ComparisonOp::Lt => cmp == std::cmp::Ordering::Less,
                    ComparisonOp::LtEq => cmp != std::cmp::Ordering::Greater,
                    ComparisonOp::Gt => cmp == std::cmp::Ordering::Greater,
                    ComparisonOp::GtEq => cmp != std::cmp::Ordering::Less,
                }
            }
            WhereClause::And(left, right) => {
                self.evaluate_predicate(left, row, schema)
                    && self.evaluate_predicate(right, row, schema)
            }
            WhereClause::Or(left, right) => {
                self.evaluate_predicate(left, row, schema)
                    || self.evaluate_predicate(right, row, schema)
            }
        }
    }

    /// 从计划节点中提取列名
    fn extract_columns(plan: &super::planner::PlanNode, schema: &TableSchema) -> Vec<String> {
        use super::planner::PlanNode;
        match plan {
            PlanNode::Projection { columns, .. } => columns.clone(),
            PlanNode::SeqScan { .. } | PlanNode::PointLookup { .. } => {
                schema.columns.iter().map(|c| c.name.clone()).collect()
            }
            PlanNode::Filter { input, .. }
            | PlanNode::Sort { input, .. }
            | PlanNode::Limit { input, .. } => Self::extract_columns(input, schema),
        }
    }

    /// 尝试对值进行类型转换以匹配 schema
    fn coerce_row_values(&self, schema: &TableSchema, values: &[Value]) -> Result<Vec<Value>> {
        let mut coerced = values.to_vec();
        for (i, val) in coerced.iter_mut().enumerate() {
            if i < schema.columns.len() {
                let col = &schema.columns[i];
                if *val != Value::Null {
                    if let Some(c) = val.coerce_for_type(&col.col_type) {
                        *val = c;
                    }
                }
            }
        }
        Ok(coerced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::parser::SqlParser;
    use crate::storage::redb_engine::RedbEngine;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn setup_executor() -> (TempDir, Executor) {
        let dir = TempDir::new().unwrap();
        let engine = Arc::new(RedbEngine::open(dir.path().join("test.db")).unwrap());
        let executor = Executor::new(engine);
        (dir, executor)
    }

    #[test]
    fn test_create_and_insert_and_select() {
        let (_dir, executor) = setup_executor();

        // CREATE TABLE
        let stmt = SqlParser::parse("CREATE TABLE sensors (id INT PRIMARY KEY, name TEXT, value FLOAT)").unwrap();
        executor.execute(&stmt).unwrap();

        // INSERT
        let stmt = SqlParser::parse("INSERT INTO sensors VALUES (1, 'temperature', 25.6)").unwrap();
        let result = executor.execute(&stmt).unwrap();
        assert!(matches!(result, ExecuteResult::WriteResult{rows_affected: 1, ..}));

        // INSERT second row
        let stmt = SqlParser::parse("INSERT INTO sensors VALUES (2, 'humidity', 60.5)").unwrap();
        executor.execute(&stmt).unwrap();

        // SELECT
        let stmt = SqlParser::parse("SELECT name, value FROM sensors WHERE value > 20").unwrap();
        let result = executor.execute(&stmt).unwrap();
        match result {
            ExecuteResult::QueryResult { columns, rows, .. } => {
                assert_eq!(columns, vec!["name", "value"]);
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("期望 QueryResult"),
        }
    }

    #[test]
    fn test_update() {
        let (_dir, executor) = setup_executor();

        executor.execute(&SqlParser::parse("CREATE TABLE test (id INT PRIMARY KEY, val TEXT)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO test VALUES (1, 'hello')").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("UPDATE test SET val = 'world' WHERE id = 1").unwrap()).unwrap();

        let result = executor.execute(&SqlParser::parse("SELECT val FROM test WHERE id = 1").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows[0][0], Value::Text("world".into()));
            }
            _ => panic!("期望 QueryResult"),
        }
    }

    #[test]
    fn test_delete() {
        let (_dir, executor) = setup_executor();

        executor.execute(&SqlParser::parse("CREATE TABLE test (id INT PRIMARY KEY, val TEXT)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO test VALUES (1, 'a')").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO test VALUES (2, 'b')").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("DELETE FROM test WHERE id = 1").unwrap()).unwrap();

        let result = executor.execute(&SqlParser::parse("SELECT * FROM test").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("期望 QueryResult"),
        }
    }

    #[test]
    fn test_drop_table() {
        let (_dir, executor) = setup_executor();

        executor.execute(&SqlParser::parse("CREATE TABLE test (id INT PRIMARY KEY, val TEXT)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("DROP TABLE test").unwrap()).unwrap();

        let result = executor.execute(&SqlParser::parse("SELECT * FROM test").unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_where_conditions() {
        let (_dir, executor) = setup_executor();

        executor.execute(&SqlParser::parse("CREATE TABLE t (id INT PRIMARY KEY, val INT)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO t VALUES (1, 10)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO t VALUES (2, 20)").unwrap()).unwrap();
        executor.execute(&SqlParser::parse("INSERT INTO t VALUES (3, 30)").unwrap()).unwrap();

        // Test AND
        let result = executor.execute(&SqlParser::parse("SELECT id FROM t WHERE id > 1 AND id < 3").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Integer(2));
            }
            _ => panic!("期望 QueryResult"),
        }

        // Test OR
        let result = executor.execute(&SqlParser::parse("SELECT id FROM t WHERE id = 1 OR id = 3").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("期望 QueryResult"),
        }

        // Test ORDER BY
        let result = executor.execute(&SqlParser::parse("SELECT id FROM t ORDER BY id DESC").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows[0][0], Value::Integer(3));
                assert_eq!(rows[2][0], Value::Integer(1));
            }
            _ => panic!("期望 QueryResult"),
        }

        // Test LIMIT
        let result = executor.execute(&SqlParser::parse("SELECT id FROM t LIMIT 2").unwrap()).unwrap();
        match result {
            ExecuteResult::QueryResult { rows, .. } => {
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("期望 QueryResult"),
        }
    }
}
