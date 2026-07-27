//! SQL 解析器
//!
//! 基于 sqlparser-rs crate，将其 AST 转换为 RustMinidb 内部表示。
//! MVP 支持：CREATE TABLE, INSERT, SELECT, UPDATE, DELETE, DROP TABLE。

use sqlparser::ast::{
    ColumnDef as SqlColumnDef, ColumnOption, DataType, Expr, ObjectType, Query, SelectItem,
    SetExpr, Statement, TableWithJoins, Value as SqlValue, Ident,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParserParser;

use crate::error::{ParseError, Result};
use crate::sql::types::{ColumnDef, ColumnType, Value};

/// UPSERT / ON CONFLICT 动作
#[derive(Debug, Clone)]
pub enum OnConflictAction {
    /// ON CONFLICT DO NOTHING — 冲突时跳过
    DoNothing,
    /// ON CONFLICT DO UPDATE SET col1=val1, col2=val2 — 冲突时更新
    DoUpdate(Vec<(String, Value)>),
}

/// RustMinidb 支持的 SQL 语句类型（MVP）
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SqlStatement {
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
        if_not_exists: bool,
    },
    Insert {
        table: String,
        columns: Vec<String>,
        values: Vec<Vec<Value>>,
        on_conflict: Option<OnConflictAction>,
    },
    Select {
        table: String,
        columns: Vec<String>,
        where_clause: Option<WhereClause>,
        aggregates: Vec<AggregateDef>,
        group_by: Vec<String>,
        having: Option<WhereClause>,
        order_by: Option<OrderBy>,
        limit: Option<usize>,
        offset: Option<usize>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Value)>,
        where_clause: Option<WhereClause>,
    },
    Delete {
        table: String,
        where_clause: Option<WhereClause>,
    },
    DropTable {
        name: String,
    },
    /// CREATE INDEX 语句
    CreateIndex {
        name: String,
        table: String,
        columns: Vec<String>,
        unique: bool,
    },
    /// DROP INDEX 语句
    DropIndex {
        name: String,
        table: String,
    },
    /// ALTER TABLE 语句
    AlterTable {
        table: String,
        operation: AlterTableOperation,
    },
}

/// ALTER TABLE 操作类型
#[derive(Debug, Clone)]
pub enum AlterTableOperation {
    AddColumn {
        column_def: crate::sql::types::ColumnDef,
    },
    DropColumn {
        column_name: String,
    },
}

/// WHERE 条件
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum WhereClause {
    Simple {
        column: String,
        operator: ComparisonOp,
        value: Value,
    },
    /// LIKE / NOT LIKE 模式匹配
    Like {
        column: String,
        pattern: String,
        negated: bool,
    },
    /// IN / NOT IN 列表匹配
    InList {
        column: String,
        values: Vec<Value>,
        negated: bool,
    },
    /// IS NULL / IS NOT NULL
    IsNull {
        column: String,
        negated: bool,
    },
    /// BETWEEN 范围匹配
    Between {
        column: String,
        low: Value,
        high: Value,
    },
    And(Box<WhereClause>, Box<WhereClause>),
    Or(Box<WhereClause>, Box<WhereClause>),
}

/// 比较操作符
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ComparisonOp {
    Eq,    // =
    NotEq, // <>
    Lt,    // <
    LtEq,  // <=
    Gt,    // >
    GtEq,  // >=
}

/// 聚合函数定义
#[derive(Debug, Clone)]
pub struct AggregateDef {
    /// 函数名: COUNT, SUM, AVG, MIN, MAX
    pub function: String,
    /// 列名或 "*"
    pub column: String,
    /// 别名（SELECT COUNT(*) AS cnt 中的 "cnt"）
    pub alias: Option<String>,
}

/// 单列排序规则
#[derive(Debug, Clone)]
pub struct OrderByItem {
    pub column: String,
    pub ascending: bool,
}

/// 排序规则（支持多列）
#[derive(Debug, Clone)]
pub struct OrderBy {
    pub items: Vec<OrderByItem>,
}

/// SQL 解析器
pub struct SqlParser;

impl SqlParser {
    /// 解析 SQL 文本为内部语句
    pub fn parse(sql: &str) -> Result<SqlStatement> {
        let dialect = GenericDialect {};
        let statements = SqlParserParser::parse_sql(&dialect, sql)
            .map_err(|e| ParseError::Syntax(e.to_string()))?;

        if statements.is_empty() {
            return Err(ParseError::Empty.into());
        }

        if statements.len() > 1 {
            return Err(ParseError::MultipleStatements("MVP 只支持单条语句".to_string()).into());
        }

        Self::convert_statement(&statements[0])
    }

    /// 将 sqlparser AST 转换为内部表示
    fn convert_statement(stmt: &Statement) -> Result<SqlStatement> {
        match stmt {
            Statement::CreateTable(create) => {
                Self::convert_create_table(&create.name, &create.columns, create.if_not_exists)
            }
            Statement::Insert(insert) => {
                let table_name = &insert.table;
                let columns = &insert.columns;
                let source = &insert.source;
                let on_conflict = &insert.on;
                Self::convert_insert(table_name, columns, source, on_conflict)
            }
            Statement::Query(query) => Self::convert_select(query),
            Statement::Update {
                table,
                assignments,
                selection,
                ..
            } => Self::convert_update(table, assignments, selection.as_ref()),
            Statement::Delete(delete) => {
                let table_name = if !delete.tables.is_empty() {
                    Self::object_name_to_string(&delete.tables[0])
                } else {
                    // 从 from 字段提取表名
                    Self::from_table_to_string(&delete.from)
                };
                Self::convert_delete(&table_name, delete.selection.as_ref())
            }
            Statement::Drop {
                object_type,
                names,
                ..
            } => {
                // 处理 DROP INDEX
                if matches!(object_type, ObjectType::Index) {
                    if names.is_empty() {
                        return Err(ParseError::Syntax("缺少索引名".to_string()).into());
                    }
                    let index_name = Self::object_name_to_string(&names[0]);
                    return Ok(SqlStatement::DropIndex {
                        name: index_name,
                        table: String::new(), // 用户后续可指定表名
                    });
                }
                Self::convert_drop(*object_type, names)
            }
            Statement::CreateIndex(create_index) => {
                Self::convert_create_index(create_index)
            }
            Statement::AlterTable {
                name,
                operations,
                ..
            } => Self::convert_alter_table(name, operations),
            _ => Err(ParseError::Unsupported(format!(
                "不支持的 SQL 语句: {:?}",
                stmt
            ))
            .into()),
        }
    }

    fn convert_create_index(stmt: &sqlparser::ast::CreateIndex) -> Result<SqlStatement> {
        let name = stmt
            .name
            .as_ref()
            .map(|n| Self::object_name_to_string(n))
            .unwrap_or_default();
        let table = Self::object_name_to_string(&stmt.table_name);
        let columns: Vec<String> = stmt.columns.iter().map(|c| c.to_string()).collect();
        let unique = stmt.unique;

        if name.is_empty() {
            return Err(ParseError::Syntax("CREATE INDEX 需要索引名".to_string()).into());
        }
        if columns.is_empty() {
            return Err(ParseError::Syntax("CREATE INDEX 需要至少一个列".to_string()).into());
        }

        Ok(SqlStatement::CreateIndex {
            name,
            table,
            columns,
            unique,
        })
    }

    fn convert_alter_table(
        name: &sqlparser::ast::ObjectName,
        operations: &[sqlparser::ast::AlterTableOperation],
    ) -> Result<SqlStatement> {
        let table = Self::object_name_to_string(name);

        if operations.is_empty() {
            return Err(ParseError::Syntax("ALTER TABLE 需要至少一个操作".to_string()).into());
        }

        // MVP 只支持第一个操作
        let op = &operations[0];
        match op {
            sqlparser::ast::AlterTableOperation::AddColumn { column_def, .. } => {
                let col_type = Self::convert_data_type(&column_def.data_type)?;
                let mut nullable = true;
                let mut is_primary_key = false;
                let mut default = None;
                let mut comment = None;

                for opt in &column_def.options {
                    match &opt.option {
                        sqlparser::ast::ColumnOption::NotNull => nullable = false,
                        sqlparser::ast::ColumnOption::Unique { is_primary: true, .. } => {
                            is_primary_key = true;
                            nullable = false;
                        }
                        sqlparser::ast::ColumnOption::Default(expr) => {
                            default = Some(Self::expr_to_value(expr)?);
                        }
                        sqlparser::ast::ColumnOption::Comment(s) => {
                            comment = Some(s.clone());
                        }
                        _ => {}
                    }
                }

                let col = crate::sql::types::ColumnDef {
                    name: column_def.name.to_string(),
                    col_type,
                    nullable,
                    is_primary_key,
                    default,
                    auto_increment: false,
                    comment,
                };

                Ok(SqlStatement::AlterTable {
                    table,
                    operation: AlterTableOperation::AddColumn { column_def: col },
                })
            }
            sqlparser::ast::AlterTableOperation::DropColumn { column_name, .. } => {
                Ok(SqlStatement::AlterTable {
                    table,
                    operation: AlterTableOperation::DropColumn {
                        column_name: column_name.to_string(),
                    },
                })
            }
            _ => Err(ParseError::Unsupported(
                format!("不支持的 ALTER TABLE 操作: {:?}", op)
            ).into()),
        }
    }

    fn convert_create_table(
        name: &sqlparser::ast::ObjectName,
        columns: &[SqlColumnDef],
        if_not_exists: bool,
    ) -> Result<SqlStatement> {
        let table_name = Self::object_name_to_string(name);
        let mut cols = Vec::new();

        for col in columns {
            let col_type = Self::convert_data_type(&col.data_type)?;
            let mut nullable = true;
            let mut is_primary_key = false;
            let mut auto_increment = false;
            let mut default = None;
            let mut comment = None;

            for opt in &col.options {
                match &opt.option {
                    ColumnOption::NotNull => nullable = false,
                    ColumnOption::Unique { is_primary: true, .. }
                    | ColumnOption::Unique { is_primary: false, .. } => {}
                    ColumnOption::Default(expr) => {
                        default = Some(Self::expr_to_value(expr)?);
                    }
                    ColumnOption::Comment(s) => {
                        comment = Some(s.clone());
                    }
                    ColumnOption::DialectSpecific(tokens) => {
                        // 检查是否包含 AUTO_INCREMENT 关键字
                        for token in tokens {
                            let s = token.to_string().to_uppercase();
                            if s == "AUTO_INCREMENT" || s == "AUTOINCREMENT" {
                                is_primary_key = true;
                                nullable = false;
                                auto_increment = true;
                            }
                        }
                    }
                    _ => {}
                }
            }

            // 处理列约束中的 PRIMARY KEY
            for opt in &col.options {
                if let ColumnOption::Unique { is_primary: true, .. } = &opt.option {
                    is_primary_key = true;
                    nullable = false;
                }
            }

            cols.push(ColumnDef {
                name: col.name.to_string(),
                col_type,
                nullable,
                is_primary_key,
                default,
                auto_increment,
                comment,
            });
        }

        // 如果没有明确的 PRIMARY KEY，尝试找第一个 "INT PRIMARY KEY" 或 "id" 列
        if !cols.iter().any(|c| c.is_primary_key) {
            // 检查是否有列名包含 "id" 且类型为 Integer
            if let Some(id_col) = cols.iter_mut().find(|c| c.name.to_lowercase() == "id") {
                id_col.is_primary_key = true;
                id_col.nullable = false;
            }
        }

        Ok(SqlStatement::CreateTable {
            name: table_name,
            columns: cols,
            if_not_exists,
        })
    }

    fn convert_insert(
        table_name: &sqlparser::ast::TableObject,
        columns: &[Ident],
        source: &Option<Box<Query>>,
        on_conflict: &Option<sqlparser::ast::OnInsert>,
    ) -> Result<SqlStatement> {
        let table = Self::table_object_to_string(table_name);
        let cols: Vec<String> = columns.iter().map(|c| c.to_string()).collect();

        let values = match source {
            Some(query) => {
                match &*query.body {
                    SetExpr::Values(values) => {
                        let mut rows = Vec::new();
                        for row in &values.rows {
                            let mut row_values = Vec::new();
                            for expr in row {
                                row_values.push(Self::expr_to_value(expr)?);
                            }
                            rows.push(row_values);
                        }
                        rows
                    }
                    _ => return Err(ParseError::Unsupported("MVP 只支持 VALUES 插入".to_string()).into()),
                }
            }
            None => return Err(ParseError::Unsupported("MVP 需要 VALUES 子句".to_string()).into()),
        };

        // 解析 ON CONFLICT 子句
        let on_conflict_action: Option<OnConflictAction> = match on_conflict {
            Some(sqlparser::ast::OnInsert::OnConflict(oc)) => {
                match &oc.action {
                    sqlparser::ast::OnConflictAction::DoNothing => {
                        Some(OnConflictAction::DoNothing)
                    }
                    sqlparser::ast::OnConflictAction::DoUpdate(do_update) => {
                        let mut assigns = Vec::new();
                        for assignment in &do_update.assignments {
                            let col = assignment.target.to_string();
                            let val = Self::expr_to_value(&assignment.value)?;
                            assigns.push((col, val));
                        }
                        Some(OnConflictAction::DoUpdate(assigns))
                    }
                }
            }
            Some(sqlparser::ast::OnInsert::DuplicateKeyUpdate(assignments)) => {
                let mut assigns = Vec::new();
                for assignment in assignments {
                    let col = assignment.target.to_string();
                    let val = Self::expr_to_value(&assignment.value)?;
                    assigns.push((col, val));
                }
                Some(OnConflictAction::DoUpdate(assigns))
            }
            // 处理其他可能新增的 OnInsert 变体
            Some(_) => {
                return Err(ParseError::Unsupported(
                    "不支持的 ON CONFLICT 变体".to_string()
                ).into());
            }
            None => None,
        };

        Ok(SqlStatement::Insert {
            table,
            columns: cols,
            values,
            on_conflict: on_conflict_action,
        })
    }

    fn convert_select(query: &Query) -> Result<SqlStatement> {
        let body = &query.body;
        let select = match &**body {
            SetExpr::Select(s) => s,
            _ => {
                return Err(
                    ParseError::Unsupported("不支持的查询类型".to_string()).into(),
                )
            }
        };

        // 提取表名（只支持单表）
        let table = match select.from.first() {
            Some(TableWithJoins {
                relation, joins, ..
            }) if joins.is_empty() => Self::table_factor_to_string(relation),
            Some(_) => {
                return Err(
                    ParseError::Unsupported("MVP 不支持 JOIN".to_string()).into(),
                )
            }
            None => return Err(ParseError::Syntax("缺少 FROM 子句".to_string()).into()),
        };

        // 提取列和聚合函数
        let mut columns = Vec::new();
        let mut aggregates = Vec::new();

        for item in &select.projection {
            match item {
                SelectItem::UnnamedExpr(Expr::Identifier(id)) => {
                    columns.push(id.to_string());
                }
                SelectItem::UnnamedExpr(Expr::Function(fun)) => {
                    let func_name = fun.name.to_string().to_uppercase();
                    let (col_name, is_star) = Self::extract_function_arg(&fun.args);
                    let col_display = if is_star { "*".to_string() } else { col_name.clone() };
                    aggregates.push(AggregateDef {
                        function: func_name,
                        column: col_display,
                        alias: None,
                    });
                    // 聚合函数列在结果集中使用 function(col) 作为列名
                    let col_alias = if is_star {
                        format!("{}(*)", fun.name)
                    } else {
                        format!("{}({})", fun.name, col_name)
                    };
                    columns.push(col_alias);
                }
                SelectItem::ExprWithAlias {
                    expr,
                    alias,
                } => {
                    let alias_str = alias.to_string();
                    // expr is &Expr when matched by reference (not Box<Expr>)
                    if let Expr::Function(fun) = expr {
                        let func_name = fun.name.to_string().to_uppercase();
                        let (col_name, is_star) = Self::extract_function_arg(&fun.args);
                        let col_display = if is_star { "*".to_string() } else { col_name };
                        aggregates.push(AggregateDef {
                            function: func_name,
                            column: col_display,
                            alias: Some(alias_str.clone()),
                        });
                        columns.push(alias_str);
                    } else {
                        columns.push(alias_str);
                    }
                }
                SelectItem::Wildcard(..) => {
                    columns.push("*".to_string());
                }
                SelectItem::QualifiedWildcard(_, _) => {
                    columns.push("*".to_string());
                }
                _ => {
                    return Err(ParseError::Unsupported(format!(
                        "不支持的 SELECT 项: {:?}",
                        item
                    ))
                    .into())
                }
            }
        }

        // 提取 WHERE
        let where_clause = select
            .selection
            .as_ref()
            .map(|expr| Self::convert_expr_to_where(expr))
            .transpose()?;

        // 提取 GROUP BY
        let group_by: Vec<String> = match &select.group_by {
            sqlparser::ast::GroupByExpr::Expressions(exprs, _) => {
                exprs.iter().filter_map(|expr| {
                    if let Expr::Identifier(id) = expr {
                        Some(id.to_string())
                    } else {
                        None
                    }
                }).collect()
            }
            _ => Vec::new(),
        };

        // 提取 HAVING
        let having = select
            .having
            .as_ref()
            .map(|expr| Self::convert_expr_to_where(expr))
            .transpose()?;

        // 提取 ORDER BY
        let order_by: Option<OrderBy> = {
            let items: Vec<OrderByItem> = query.order_by.as_ref().map_or(vec![], |ob| {
                ob.exprs.iter().filter_map(|expr| {
                    match &expr.expr {
                        Expr::Identifier(id) => Some(OrderByItem {
                            column: id.to_string(),
                            ascending: expr.asc.unwrap_or(true),
                        }),
                        _ => None,
                    }
                }).collect()
            });
            if items.is_empty() { None } else { Some(OrderBy { items }) }
        };

        // 提取 LIMIT / OFFSET
        let limit = query.limit.as_ref().and_then(|l| match l {
            Expr::Value(SqlValue::Number(n, _)) => n.parse::<usize>().ok(),
            _ => None,
        });

        let offset = query.offset.as_ref().and_then(|o| match &o.value {
            Expr::Value(SqlValue::Number(n, _)) => n.parse::<usize>().ok(),
            _ => None,
        });

        Ok(SqlStatement::Select {
            table,
            columns,
            where_clause,
            aggregates,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    /// 从函数参数中提取列名和是否为 COUNT(*)
    fn extract_function_arg(args: &sqlparser::ast::FunctionArguments) -> (String, bool) {
        match args {
            sqlparser::ast::FunctionArguments::List(list) => {
                if list.args.is_empty() {
                    return ("*".to_string(), true);
                }
                if let Some(first) = list.args.first() {
                    match first {
                        sqlparser::ast::FunctionArg::Unnamed(
                            sqlparser::ast::FunctionArgExpr::Wildcard,
                        ) => ("*".to_string(), true),
                        sqlparser::ast::FunctionArg::Unnamed(
                            sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(id)),
                        ) => (id.to_string(), false),
                        sqlparser::ast::FunctionArg::Named { name, .. } => {
                            (name.to_string(), false)
                        }
                        _ => ("*".to_string(), false),
                    }
                } else {
                    ("*".to_string(), true)
                }
            }
            _ => ("*".to_string(), true),
        }
    }

    fn convert_update(
        table: &TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: Option<&Expr>,
    ) -> Result<SqlStatement> {
        let table = Self::table_factor_to_string(&table.relation);

        let mut assigns = Vec::new();
        for assignment in assignments {
            let col = assignment.target.to_string();
            let val = Self::expr_to_value(&assignment.value)?;
            assigns.push((col, val));
        }

        let where_clause = selection
            .map(|expr| Self::convert_expr_to_where(expr))
            .transpose()?;

        Ok(SqlStatement::Update {
            table,
            assignments: assigns,
            where_clause,
        })
    }

    fn convert_delete(
        table_name: &str,
        selection: Option<&Expr>,
    ) -> Result<SqlStatement> {
        let where_clause = selection
            .map(|expr| Self::convert_expr_to_where(expr))
            .transpose()?;

        Ok(SqlStatement::Delete {
            table: table_name.to_string(),
            where_clause,
        })
    }

    #[allow(unreachable_code)]
    fn convert_drop(object_type: ObjectType, names: &[sqlparser::ast::ObjectName]) -> Result<SqlStatement> {
        if matches!(object_type, ObjectType::Table) {
            if names.is_empty() {
                return Err(ParseError::Syntax("缺少表名".to_string()).into());
            }
            let name = Self::object_name_to_string(&names[0]);
            Ok(SqlStatement::DropTable { name })
        } else {
            Err(ParseError::Unsupported("只支持 DROP TABLE".to_string()).into())
        }
    }

    // ── 辅助方法 ──

    fn convert_data_type(dt: &DataType) -> Result<ColumnType> {
        match dt {
            DataType::Int(_) | DataType::Integer(_) | DataType::BigInt(_) => {
                Ok(ColumnType::Integer)
            }
            DataType::Float(_) | DataType::Double(_) | DataType::Real => Ok(ColumnType::Float),
            DataType::Text | DataType::String(_) | DataType::Char(_) | DataType::Varchar(_) => {
                Ok(ColumnType::Text)
            }
            DataType::Boolean => Ok(ColumnType::Boolean),
            DataType::Timestamp(_, _) => {
                Ok(ColumnType::Timestamp)
            }
            DataType::Blob(_) | DataType::Bytes(_) => Ok(ColumnType::Blob),
            _ => {
                // 默认转为 Text
                Ok(ColumnType::Text)
            }
        }
    }

    fn expr_to_value(expr: &Expr) -> Result<Value> {
        match expr {
            Expr::Value(SqlValue::Number(n, _)) => {
                // 尝试解析为整数，失败则解析为浮点数
                if let Ok(v) = n.parse::<i64>() {
                    Ok(Value::Integer(v))
                } else if let Ok(v) = n.parse::<f64>() {
                    Ok(Value::Float(v))
                } else {
                    Ok(Value::Text(n.clone()))
                }
            }
            Expr::Value(SqlValue::SingleQuotedString(s)) => Ok(Value::Text(s.clone())),
            Expr::Value(SqlValue::Boolean(b)) => Ok(Value::Boolean(*b)),
            Expr::Value(SqlValue::Null) => Ok(Value::Null),
            Expr::UnaryOp { op: _, expr } => {
                // 处理负数
                if let Expr::Value(SqlValue::Number(n, _)) = expr.as_ref() {
                    if let Ok(v) = n.parse::<i64>() {
                        return Ok(Value::Integer(-v));
                    } else if let Ok(v) = n.parse::<f64>() {
                        return Ok(Value::Float(-v));
                    }
                }
                Self::expr_to_value(expr)
            }
            Expr::TypedString { data_type: _, value } => {
                Ok(Value::Text(value.clone()))
            }
            Expr::CompoundIdentifier(parts) => {
                Ok(Value::Text(parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".")))
            }
            Expr::Identifier(id) => Ok(Value::Text(id.to_string())),
            _ => Err(ParseError::Syntax(format!("不支持的表达式: {:?}", expr)).into()),
        }
    }

    fn convert_expr_to_where(expr: &Expr) -> Result<WhereClause> {
        match expr {
            Expr::BinaryOp {
                left,
                op,
                right,
            } => {
                let op_str = op.to_string();
                let operator = match op_str.as_str() {
                    "=" => ComparisonOp::Eq,
                    "<>" | "!=" => ComparisonOp::NotEq,
                    "<" => ComparisonOp::Lt,
                    "<=" => ComparisonOp::LtEq,
                    ">" => ComparisonOp::Gt,
                    ">=" => ComparisonOp::GtEq,
                    "AND" | "And" => {
                        return Ok(WhereClause::And(
                            Box::new(Self::convert_expr_to_where(left)?),
                            Box::new(Self::convert_expr_to_where(right)?),
                        ));
                    }
                    "OR" | "Or" => {
                        return Ok(WhereClause::Or(
                            Box::new(Self::convert_expr_to_where(left)?),
                            Box::new(Self::convert_expr_to_where(right)?),
                        ));
                    }
                    other => {
                        return Err(ParseError::Syntax(format!("不支持的运算符: {}", other))
                            .into());
                    }
                };

                let column = match left.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => {
                        return Err(ParseError::Syntax(
                            "WHERE 条件左侧必须是列名".to_string(),
                        )
                        .into())
                    }
                };

                let value = Self::expr_to_value(right.as_ref())?;

                Ok(WhereClause::Simple {
                    column,
                    operator,
                    value,
                })
            }
            // LIKE / ILIKE / NOT LIKE 模式匹配
            Expr::Like {
                negated,
                any: _,
                expr,
                pattern,
                escape_char: _,
            } => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "LIKE 左侧必须是列名".to_string()
                    ).into()),
                };
                let pattern_str = Self::expr_to_string(pattern.as_ref())?
                    .ok_or_else(|| ParseError::Syntax("LIKE 模式必须是字符串".to_string()))?;
                Ok(WhereClause::Like {
                    column,
                    pattern: pattern_str,
                    negated: *negated,
                })
            }
            // 大小写不敏感 LIKE
            Expr::ILike {
                negated,
                any: _,
                expr,
                pattern,
                escape_char: _,
            } => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "ILIKE 左侧必须是列名".to_string()
                    ).into()),
                };
                let pattern_str = Self::expr_to_string(pattern.as_ref())?
                    .ok_or_else(|| ParseError::Syntax("ILIKE 模式必须是字符串".to_string()))?;
                // 大小写不敏感：将模式和列值都转为小写
                // 用 "!" 标记这是一个大小写不敏感的 LIKE
                let transformed = format!("ILike:{}", pattern_str);
                Ok(WhereClause::Like {
                    column,
                    pattern: transformed,
                    negated: *negated,
                })
            }
            // IN / NOT IN
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "IN 左侧必须是列名".to_string()
                    ).into()),
                };
                let mut values = Vec::new();
                for item in list {
                    values.push(Self::expr_to_value(item)?);
                }
                Ok(WhereClause::InList {
                    column,
                    values,
                    negated: *negated,
                })
            }
            // IS NULL / IS NOT NULL
            Expr::IsNull(expr) => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "IS NULL 左侧必须是列名".to_string()
                    ).into()),
                };
                Ok(WhereClause::IsNull {
                    column,
                    negated: false,
                })
            }
            Expr::IsNotNull(expr) => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "IS NOT NULL 左侧必须是列名".to_string()
                    ).into()),
                };
                Ok(WhereClause::IsNull {
                    column,
                    negated: true,
                })
            }
            // BETWEEN
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let column = match expr.as_ref() {
                    Expr::Identifier(id) => id.to_string(),
                    _ => return Err(ParseError::Syntax(
                        "BETWEEN 左侧必须是列名".to_string()
                    ).into()),
                };
                let low_val = Self::expr_to_value(low.as_ref())?;
                let high_val = Self::expr_to_value(high.as_ref())?;
                if *negated {
                    // NOT BETWEEN = col < low OR col > high
                    return Ok(WhereClause::Or(
                        Box::new(WhereClause::Simple {
                            column: column.clone(),
                            operator: ComparisonOp::Lt,
                            value: low_val,
                        }),
                        Box::new(WhereClause::Simple {
                            column,
                            operator: ComparisonOp::Gt,
                            value: high_val,
                        }),
                    ));
                }
                Ok(WhereClause::Between {
                    column,
                    low: low_val,
                    high: high_val,
                })
            }
            _ => Err(ParseError::Syntax(format!("不支持的 WHERE 表达式: {:?}", expr)).into()),
        }
    }

    /// 将 Expr 转为字符串值（针对文字/字符串表达式）
    fn expr_to_string(expr: &Expr) -> Result<Option<String>> {
        match expr {
            Expr::Value(SqlValue::SingleQuotedString(s)) => Ok(Some(s.clone())),
            Expr::Value(SqlValue::DoubleQuotedString(s)) => Ok(Some(s.clone())),
            Expr::Value(SqlValue::Number(n, _)) => Ok(Some(n.clone())),
            _ => Ok(None),
        }
    }

    fn object_name_to_string(name: &sqlparser::ast::ObjectName) -> String {
        name.0
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(".")
    }

    fn table_factor_to_string(tf: &sqlparser::ast::TableFactor) -> String {
        match tf {
            sqlparser::ast::TableFactor::Table { name, .. } => Self::object_name_to_string(name),
            _ => "unknown".to_string(),
        }
    }

    fn table_object_to_string(to: &sqlparser::ast::TableObject) -> String {
        match to {
            sqlparser::ast::TableObject::TableName(name) => Self::object_name_to_string(name),
            _ => to.to_string(),
        }
    }

    /// 将 FromTable 转换为表名字符串
    fn from_table_to_string(ft: &sqlparser::ast::FromTable) -> String {
        let tables = match ft {
            sqlparser::ast::FromTable::WithFromKeyword(tables) => tables,
            sqlparser::ast::FromTable::WithoutKeyword(tables) => tables,
        };
        if let Some(twj) = tables.first() {
            Self::table_factor_to_string(&twj.relation)
        } else {
            "unknown".to_string()
        }
    }

    #[allow(dead_code)]
    fn ident_to_string(id: &Ident) -> String {
        id.to_string()
    }
}

/// 对两个值的排序比较（用于 ORDER BY）
pub fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Integer(ai), Value::Integer(bi)) => ai.cmp(bi),
        (Value::Integer(ai), Value::Float(bf)) => (*ai as f64).total_cmp(bf),
        (Value::Float(af), Value::Integer(bi)) => af.total_cmp(&(*bi as f64)),
        (Value::Float(af), Value::Float(bf)) => af.total_cmp(bf),
        (Value::Text(at), Value::Text(bt)) => at.cmp(bt),
        (Value::Timestamp(at), Value::Timestamp(bt)) => at.cmp(bt),
        (Value::Boolean(at), Value::Boolean(bt)) => at.cmp(bt),
        _ => std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_table() {
        let sql = "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::CreateTable { name, columns, .. } => {
                assert_eq!(name, "users");
                assert_eq!(columns.len(), 3);
                assert!(columns[0].is_primary_key);
                assert_eq!(columns[0].col_type, ColumnType::Integer);
                assert_eq!(columns[1].col_type, ColumnType::Text);
            }
            _ => panic!("期望 CREATE TABLE"),
        }
    }

    #[test]
    fn test_parse_select() {
        let sql = "SELECT name, age FROM users WHERE age > 18 ORDER BY age DESC LIMIT 10";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::Select {
                table,
                columns,
                where_clause,
                order_by,
                limit,
                ..
            } => {
                assert_eq!(table, "users");
                assert_eq!(columns, vec!["name", "age"]);
                assert!(where_clause.is_some());
                assert!(order_by.is_some());
                assert_eq!(limit, Some(10));
            }
            _ => panic!("期望 SELECT"),
        }
    }

    #[test]
    fn test_parse_insert() {
        let sql = "INSERT INTO users VALUES (1, 'Alice', 30)";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::Insert {
                table,
                columns,
                values,
                on_conflict: _,
            } => {
                assert_eq!(table, "users");
                assert!(columns.is_empty());
                assert_eq!(values.len(), 1);
                assert_eq!(values[0][0], Value::Integer(1));
            }
            _ => panic!("期望 INSERT"),
        }
    }

    #[test]
    fn test_parse_syntax_error() {
        let sql = "SLECT * FROME users";
        let result = SqlParser::parse(sql);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::Update {
                table,
                assignments,
                where_clause,
            } => {
                assert_eq!(table, "users");
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].0, "name");
                assert!(where_clause.is_some());
            }
            _ => panic!("期望 UPDATE"),
        }
    }

    #[test]
    fn test_parse_delete() {
        let sql = "DELETE FROM users WHERE id = 1";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::Delete {
                table,
                where_clause,
            } => {
                assert_eq!(table, "users");
                assert!(where_clause.is_some());
            }
            _ => panic!("期望 DELETE"),
        }
    }

    #[test]
    fn test_parse_drop_table() {
        let sql = "DROP TABLE users";
        let stmt = SqlParser::parse(sql).unwrap();
        match stmt {
            SqlStatement::DropTable { name } => {
                assert_eq!(name, "users");
            }
            _ => panic!("期望 DROP TABLE"),
        }
    }
}
