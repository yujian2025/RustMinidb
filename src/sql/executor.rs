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
                on_conflict,
            } => self.execute_insert(table, columns, values, on_conflict),
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
            SqlStatement::CreateIndex {
                name,
                table,
                columns,
                unique,
            } => self.execute_create_index(name, table, columns, *unique),
            SqlStatement::DropIndex { name, table } => self.execute_drop_index(name, table),
            SqlStatement::AlterTable { table, operation } => {
                self.execute_alter_table(table, operation)
            }
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
        on_conflict: &Option<crate::sql::parser::OnConflictAction>,
    ) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        let mut inserted = 0;
        let mut updated = 0;
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
            let mut row = Row { values: coerced };

            // 自动填充 AUTO_INCREMENT 列
            self.fill_auto_increment(table, &schema, &mut row)?;

            schema
                .validate_row(&row.values)
                .map_err(|e| ExecError::TypeMismatch(e))?;

            // 尝试插入，主键冲突时根据 on_conflict 处理
            match self.engine.insert_row(table, row.clone()) {
                Ok(_) => {
                    // 插入成功后维护索引
                    if let Err(e) = self.update_indexes_for_insert(table, &schema, &row) {
                        tracing::warn!("索引维护失败: {}", e);
                    }
                    inserted += 1;
                }
                Err(e) => {
                    // 检查是否是主键冲突
                    if let crate::error::RustMinidbError::Engine(
                        crate::error::EngineError::PrimaryKeyConflict(_),
                    ) = &e
                    {
                        match on_conflict {
                            Some(crate::sql::parser::OnConflictAction::DoNothing) => {
                                // 跳过冲突行
                            }
                            Some(crate::sql::parser::OnConflictAction::DoUpdate(assigns)) => {
                                // 更新冲突行：先获取旧行，再合并更新
                                let pk = {
                                    let pk_idx = schema.pk_index().ok_or_else(|| {
                                        ExecError::ConstraintViolation(
                                            "UPSERT 目标表没有主键".to_string(),
                                        )
                                    })?;
                                    row_values
                                        .get(pk_idx)
                                        .ok_or_else(|| {
                                            ExecError::Validation(
                                                "缺少主键值".to_string(),
                                            )
                                        })?
                                        .clone()
                                };
                                // 获取旧行
                                let old_row = self.engine.get_row(table, &pk)?.ok_or_else(|| {
                                    ExecError::TableNotFound(table.to_string())
                                })?;
                                // 合并赋值
                                let mut new_values = old_row.values.clone();
                                for (col, val) in assigns {
                                    if let Some(idx) =
                                        schema.columns.iter().position(|c| c.name == *col)
                                    {
                                        new_values[idx] = val.clone();
                                    }
                                }
                                self.engine
                                    .update_row(table, &pk, Row::new(new_values))?;
                                updated += 1;
                            }
                            None => {
                                // 没有 ON CONFLICT 子句，向上传播主键冲突错误
                                return Err(e);
                            }
                        }
                    } else {
                        // 非主键冲突错误，直接向上传播
                        return Err(e);
                    }
                }
            }
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
            rows_affected: inserted + updated,
            last_insert_id: last_id,
        })
    }

    fn execute_select(&self, stmt: &SqlStatement, table: &str) -> Result<ExecuteResult> {
        let schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        // 获取表上的索引列表
        let indexes = self.engine.list_indexes(table).unwrap_or_default();

        // 生成执行计划（传入索引信息以供二级索引优化）
        let plan = Planner::plan_select(stmt, &schema, &indexes)?;

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

            // 删除旧索引条目（在更新数据之前）
            if let Err(e) = self.remove_indexes_for_delete(table, &schema, row) {
                tracing::warn!("索引维护失败(删除旧条目): {}", e);
            }

            // 更新行
            let new_row = Row { values: new_values };
            self.engine
                .update_row(table, pk, new_row.clone())?;

            // 插入新索引条目（在更新数据之后）
            if let Err(e) = self.update_indexes_for_insert(table, &schema, &new_row) {
                tracing::warn!("索引维护失败(插入新条目): {}", e);
            }

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

            // 删除索引条目（在删除数据之前）
            if let Err(e) = self.remove_indexes_for_delete(table, &schema, row) {
                tracing::warn!("索引维护失败: {}", e);
            }

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

    /// 为 AUTO_INCREMENT 列自动分配递增值
    fn fill_auto_increment(
        &self,
        table: &str,
        schema: &crate::storage::schema::TableSchema,
        row: &mut Row,
    ) -> Result<()> {
        for (i, col) in schema.columns.iter().enumerate() {
            if col.auto_increment && i < row.values.len() {
                // 如果该列的值为 NULL 或 0，自动分配
                let needs_fill = match &row.values[i] {
                    Value::Null => true,
                    Value::Integer(v) => *v == 0,
                    _ => false,
                };
                if needs_fill {
                    // 获取当前最大值
                    let next_id = self.get_next_auto_increment_id(table, &col.name)?;
                    row.values[i] = Value::Integer(next_id);
                }
            }
        }
        Ok(())
    }

    /// 获取下一个 AUTO_INCREMENT 的值
    fn get_next_auto_increment_id(&self, table: &str, col_name: &str) -> Result<i64> {
        let rows = self.engine.scan_table(table)?;
        let mut max_val: i64 = 0;
        // 尝试获取 schema 来确定列索引
        if let Ok(Some(schema)) = self.engine.get_schema(table) {
            if let Some(col_idx) = schema.col_index(col_name) {
                for row in &rows {
                    if col_idx < row.values.len() {
                        if let Value::Integer(v) = row.values[col_idx] {
                            if v > max_val {
                                max_val = v;
                            }
                        }
                    }
                }
            }
        }
        Ok(max_val + 1)
    }

    fn execute_create_index(
        &self,
        name: &str,
        table: &str,
        columns: &[String],
        unique: bool,
    ) -> Result<ExecuteResult> {
        // 检查表是否存在
        let _schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        // 检查列是否存在
        for col_name in columns {
            if _schema.col_index(col_name).is_none() {
                return Err(ExecError::ColumnNotFound(col_name.clone()).into());
            }
        }

        let index = crate::storage::schema::IndexDef {
            name: name.to_string(),
            table_name: table.to_string(),
            columns: columns.to_vec(),
            unique,
        };

        self.engine.create_index(&index)?;
        Ok(ExecuteResult::WriteResult {
            rows_affected: 0,
            last_insert_id: None,
        })
    }

    fn execute_drop_index(&self, name: &str, table: &str) -> Result<ExecuteResult> {
        if !table.is_empty() {
            self.engine.drop_index(table, name)?;
        } else {
            // 没有指定表名时，尝试在所有表中查找该索引
            let tables = self.engine.list_tables()?;
            let mut found = false;
            for tbl in &tables {
                let indexes = self.engine.list_indexes(tbl)?;
                if indexes.iter().any(|idx| idx.name == name) {
                    self.engine.drop_index(tbl, name)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(ExecError::TableNotFound(
                    format!("索引 '{}' 不存在", name)
                ).into());
            }
        }
        Ok(ExecuteResult::WriteResult {
            rows_affected: 0,
            last_insert_id: None,
        })
    }

    fn execute_alter_table(
        &self,
        table: &str,
        operation: &crate::sql::parser::AlterTableOperation,
    ) -> Result<ExecuteResult> {
        use crate::sql::parser::AlterTableOperation;
        let mut schema = self
            .engine
            .get_schema(table)?
            .ok_or_else(|| ExecError::TableNotFound(table.to_string()))?;

        match operation {
            AlterTableOperation::AddColumn { column_def } => {
                // 检查列是否已存在
                if schema.col_index(&column_def.name).is_some() {
                    return Err(ExecError::Validation(
                        format!("列 '{}' 已存在于表 '{}'", column_def.name, table)
                    ).into());
                }
                schema.columns.push(column_def.clone());
            }
            AlterTableOperation::DropColumn { column_name } => {
                let idx = schema.col_index(column_name)
                    .ok_or_else(|| ExecError::ColumnNotFound(column_name.clone()))?;
                // 主键列不能被删除
                if schema.columns[idx].is_primary_key {
                    return Err(ExecError::Validation(
                        format!("不能删除主键列 '{}'", column_name)
                    ).into());
                }
                schema.columns.remove(idx);
            }
        }

        // 更新 schema 元数据
        self.engine.update_schema(&schema)?;

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
            PlanNode::IndexScan {
                table,
                index_name,
                key,
            } => {
                // 通过索引查找主键，再回表获取完整行
                let indexes = self.engine.list_indexes(table).unwrap_or_default();
                let index = indexes.iter().find(|idx| idx.name == *index_name);
                match index {
                    Some(idx) => {
                        use crate::storage::encoding::serialize_value;
                        let key_bytes = serialize_value(key);
                        let pks = self.engine.scan_index_eq(idx, &key_bytes)?;
                        let mut rows = Vec::new();
                        for pk_bytes in &pks {
                            use crate::storage::encoding::deserialize_value;
                            let pk_val = deserialize_value(pk_bytes);
                            if let Some(row) = self.engine.get_row(table, &pk_val)? {
                                rows.push(row);
                            }
                        }
                        Ok(rows)
                    }
                    None => {
                        // 索引不存在，回退到全表扫描
                        self.engine.scan_table(table)
                    }
                }
            }
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
                // 多列排序：按 items 的顺序逐级比较
                let sort_items = order_by.items.clone();
                rows.sort_by(|a, b| {
                    for item in &sort_items {
                        let col_idx = schema
                            .columns
                            .iter()
                            .position(|c| c.name == item.column)
                            .unwrap_or(0);
                        let cmp = compare_values(&a.values[col_idx], &b.values[col_idx]);
                        if cmp != std::cmp::Ordering::Equal {
                            return if item.ascending { cmp } else { cmp.reverse() };
                        }
                    }
                    std::cmp::Ordering::Equal
                });
                Ok(rows)
            }
            PlanNode::Aggregate {
                input,
                aggregates,
                group_by,
            } => {
                let rows = self.evaluate_plan(input, schema)?;
                Ok(self.execute_aggregate(&rows, schema, aggregates, group_by))
            }
            PlanNode::Having { input, predicate } => {
                let rows = self.evaluate_plan(input, schema)?;
                Ok(rows
                    .into_iter()
                    .filter(|row| self.evaluate_predicate(predicate, row, schema))
                    .collect())
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
            WhereClause::Like {
                column,
                pattern,
                negated,
            } => {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == *column)
                    .unwrap();
                let row_val = &row.values[col_idx];

                // 检查是否为 ILike（大小写不敏感）
                let (is_insensitive, actual_pattern) = if let Some(rest) = pattern.strip_prefix("ILike:") {
                    (true, rest)
                } else {
                    (false, pattern.as_str())
                };

                let matched = match row_val {
                    Value::Text(s) => {
                        let s = if is_insensitive { s.to_lowercase() } else { s.clone() };
                        Self::like_match(actual_pattern, &s, is_insensitive)
                    }
                    _ => false,
                };

                if *negated { !matched } else { matched }
            }
            WhereClause::And(left, right) => {
                self.evaluate_predicate(left, row, schema)
                    && self.evaluate_predicate(right, row, schema)
            }
            WhereClause::Or(left, right) => {
                self.evaluate_predicate(left, row, schema)
                    || self.evaluate_predicate(right, row, schema)
            }
            WhereClause::InList {
                column,
                values,
                negated,
            } => {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == *column)
                    .unwrap();
                let row_val = &row.values[col_idx];
                let found = values.iter().any(|v| compare_values(row_val, v) == std::cmp::Ordering::Equal);
                if *negated { !found } else { found }
            }
            WhereClause::IsNull {
                column,
                negated,
            } => {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == *column)
                    .unwrap();
                let row_val = &row.values[col_idx];
                let is_null = matches!(row_val, Value::Null);
                if *negated { !is_null } else { is_null }
            }
            WhereClause::Between {
                column,
                low,
                high,
            } => {
                let col_idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name == *column)
                    .unwrap();
                let row_val = &row.values[col_idx];
                let cmp_low = compare_values(row_val, low);
                let cmp_high = compare_values(row_val, high);
                // BETWEEN is inclusive: col >= low AND col <= high
                cmp_low != std::cmp::Ordering::Less && cmp_high != std::cmp::Ordering::Greater
            }
        }
    }

    /// SQL LIKE 模式匹配（支持 % 和 _ 通配符）
    fn like_match(pattern: &str, s: &str, case_insensitive: bool) -> bool {
        let pattern = if case_insensitive {
            pattern.to_lowercase()
        } else {
            pattern.to_string()
        };
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let s_chars: Vec<char> = s.chars().collect();

        let mut pi = 0; // pattern index
        let mut si = 0; // string index
        let mut star_pi: Option<usize> = None; // last % position in pattern
        let mut star_si: usize = 0; // last matched position in string

        while si < s_chars.len() {
            if pi < pattern_chars.len()
                && (pattern_chars[pi] == s_chars[si] || pattern_chars[pi] == '_')
            {
                pi += 1;
                si += 1;
            } else if pi < pattern_chars.len() && pattern_chars[pi] == '%' {
                // % matches zero or more characters
                star_pi = Some(pi);
                star_si = si;
                pi += 1;
            } else if let Some(sp) = star_pi {
                // backtrack: the last % consumed one more character
                pi = sp + 1;
                star_si += 1;
                si = star_si;
            } else {
                return false;
            }
        }

        // skip trailing %
        while pi < pattern_chars.len() && pattern_chars[pi] == '%' {
            pi += 1;
        }

        pi == pattern_chars.len()
    }

    /// 从计划节点中提取列名
    fn extract_columns(plan: &super::planner::PlanNode, schema: &TableSchema) -> Vec<String> {
        use super::planner::PlanNode;
        match plan {
            PlanNode::Projection { columns, .. } => columns.clone(),
            PlanNode::SeqScan { .. }
            | PlanNode::PointLookup { .. }
            | PlanNode::IndexScan { .. } => {
                schema.columns.iter().map(|c| c.name.clone()).collect()
            }
            PlanNode::Filter { input, .. }
            | PlanNode::Sort { input, .. }
            | PlanNode::Aggregate { input, .. }
            | PlanNode::Having { input, .. }
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

    // ── 索引维护辅助方法 ──

    /// 插入一行后，为该行的所有索引列创建索引条目
    fn update_indexes_for_insert(
        &self,
        table: &str,
        schema: &TableSchema,
        row: &Row,
    ) -> Result<()> {
        use crate::storage::encoding::serialize_value;
        let indexes = self.engine.list_indexes(table).unwrap_or_default();
        let pk_val = pk_value(row, schema);
        for index in &indexes {
            if let Some(pk) = pk_val {
                let pk_bytes = serialize_value(pk);
                let mut col_bytes = Vec::new();
                for col_name in &index.columns {
                    if let Some(idx) = schema.col_index(col_name) {
                        let val_bytes = serialize_value(&row.values[idx]);
                        col_bytes.extend_from_slice(&val_bytes);
                    }
                }
                if !col_bytes.is_empty() {
                    self.engine.insert_index_entry(index, &col_bytes, &pk_bytes)?;
                }
            }
        }
        Ok(())
    }

    /// 删除一行前，删除该行的所有索引条目
    fn remove_indexes_for_delete(
        &self,
        table: &str,
        schema: &TableSchema,
        row: &Row,
    ) -> Result<()> {
        use crate::storage::encoding::serialize_value;
        let indexes = self.engine.list_indexes(table).unwrap_or_default();
        let pk_val = pk_value(row, schema);
        for index in &indexes {
            if let Some(pk) = pk_val {
                let pk_bytes = serialize_value(pk);
                let mut col_bytes = Vec::new();
                for col_name in &index.columns {
                    if let Some(idx) = schema.col_index(col_name) {
                        let val_bytes = serialize_value(&row.values[idx]);
                        col_bytes.extend_from_slice(&val_bytes);
                    }
                }
                if !col_bytes.is_empty() {
                    self.engine.delete_index_entry(index, &col_bytes, &pk_bytes)?;
                }
            }
        }
        Ok(())
    }

    /// 执行聚合函数（COUNT/SUM/AVG/MIN/MAX）和 GROUP BY
    fn execute_aggregate(
        &self,
        rows: &[Row],
        schema: &TableSchema,
        aggregates: &[crate::sql::parser::AggregateDef],
        group_by: &[String],
    ) -> Vec<Row> {

        if rows.is_empty() {
            // 空表：返回一行全 NULL 或 0
            let mut values = Vec::new();
            for agg in aggregates {
                match agg.function.as_str() {
                    "COUNT" => values.push(Value::Integer(0)),
                    _ => values.push(Value::Null),
                }
            }
            return vec![Row { values }];
        }

        // 按 GROUP BY 列分组
        let group_indices: Vec<usize> = group_by
            .iter()
            .filter_map(|col| schema.columns.iter().position(|c| c.name == *col))
            .collect();

        let mut groups: Vec<(Vec<Value>, Vec<&Row>)> = Vec::new();

        for row in rows {
            let key: Vec<Value> = group_indices
                .iter()
                .map(|&i| row.values[i].clone())
                .collect();

            if let Some(existing) = groups.iter_mut().find(|(k, _)| *k == key) {
                existing.1.push(row);
            } else {
                groups.push((key, vec![row]));
            }
        }

        // 如果没有 GROUP BY，整个结果集作为一个组
        if groups.is_empty() && !group_by.is_empty() {
            groups.push((vec![], rows.iter().collect()));
        } else if groups.is_empty() {
            groups.push((vec![], rows.iter().collect()));
        }

        // 对每个组计算聚合值
        let mut result_rows = Vec::new();
        for (group_key, group_rows) in &groups {
            let mut values = group_key.clone();

            for agg in aggregates {
                let agg_val = self.compute_aggregate(agg, group_rows, schema);
                values.push(agg_val);
            }

            result_rows.push(Row { values });
        }

        result_rows
    }

    /// 计算单个聚合函数
    fn compute_aggregate(
        &self,
        agg: &crate::sql::parser::AggregateDef,
        rows: &[&Row],
        schema: &TableSchema,
    ) -> Value {
        match agg.function.as_str() {
            "COUNT" => {
                if agg.column == "*" {
                    // COUNT(*) 计数所有行
                    Value::Integer(rows.len() as i64)
                } else {
                    // COUNT(col) 计数非 NULL 行
                    let col_idx = schema.columns.iter().position(|c| c.name == agg.column);
                    match col_idx {
                        Some(idx) => {
                            let count = rows
                                .iter()
                                .filter(|row| row.values[idx] != Value::Null)
                                .count();
                            Value::Integer(count as i64)
                        }
                        None => Value::Integer(0),
                    }
                }
            }
            "SUM" => {
                let col_idx = match schema.columns.iter().position(|c| c.name == agg.column) {
                    Some(idx) => idx,
                    None => return Value::Null,
                };
                let mut sum: Option<f64> = None;
                for row in rows {
                    match &row.values[col_idx] {
                        Value::Integer(i) => {
                            sum = Some(sum.unwrap_or(0.0) + *i as f64);
                        }
                        Value::Float(f) => {
                            sum = Some(sum.unwrap_or(0.0) + f);
                        }
                        _ => {}
                    }
                }
                match sum {
                    Some(s) => {
                        // 如果所有值都是整数，返回整数
                        if rows.iter().all(|r| matches!(r.values[col_idx], Value::Integer(_) | Value::Null)) {
                            Value::Integer(s as i64)
                        } else {
                            Value::Float(s)
                        }
                    }
                    None => Value::Null,
                }
            }
            "AVG" => {
                let col_idx = match schema.columns.iter().position(|c| c.name == agg.column) {
                    Some(idx) => idx,
                    None => return Value::Null,
                };
                let mut sum: f64 = 0.0;
                let mut count = 0;
                for row in rows {
                    match &row.values[col_idx] {
                        Value::Integer(i) => {
                            sum += *i as f64;
                            count += 1;
                        }
                        Value::Float(f) => {
                            sum += f;
                            count += 1;
                        }
                        _ => {}
                    }
                }
                if count > 0 {
                    Value::Float(sum / count as f64)
                } else {
                    Value::Null
                }
            }
            "MIN" => {
                let col_idx = match schema.columns.iter().position(|c| c.name == agg.column) {
                    Some(idx) => idx,
                    None => return Value::Null,
                };
                rows.iter()
                    .filter_map(|row| {
                        if row.values[col_idx] != Value::Null {
                            Some(&row.values[col_idx])
                        } else {
                            None
                        }
                    })
                    .min_by(|a, b| compare_values(a, b))
                    .cloned()
                    .unwrap_or(Value::Null)
            }
            "MAX" => {
                let col_idx = match schema.columns.iter().position(|c| c.name == agg.column) {
                    Some(idx) => idx,
                    None => return Value::Null,
                };
                rows.iter()
                    .filter_map(|row| {
                        if row.values[col_idx] != Value::Null {
                            Some(&row.values[col_idx])
                        } else {
                            None
                        }
                    })
                    .max_by(|a, b| compare_values(a, b))
                    .cloned()
                    .unwrap_or(Value::Null)
            }
            _ => Value::Null,
        }
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
