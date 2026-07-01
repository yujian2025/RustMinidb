//! 查询计划器
//!
//! 将 SqlStatement 转换为执行计划（PlanNode）。
//! MVP 的优化策略：主键等值查询走 PointLookup，其他走 SeqScan + 内存过滤。

use crate::sql::parser::{ComparisonOp, OrderBy, SqlStatement, WhereClause};
use crate::sql::types::Value;
use crate::storage::schema::TableSchema;

/// 查询计划节点
#[derive(Debug)]
pub enum PlanNode {
    /// 全表扫描
    SeqScan { table: String },
    /// 主键点查（走索引）
    PointLookup { table: String, pk: Value },
    /// 过滤
    Filter {
        input: Box<PlanNode>,
        predicate: WhereClause,
    },
    /// 投影（选择列）
    Projection {
        input: Box<PlanNode>,
        columns: Vec<String>,
    },
    /// 排序
    Sort {
        input: Box<PlanNode>,
        order_by: OrderBy,
    },
    /// 分页限制
    Limit {
        input: Box<PlanNode>,
        limit: usize,
        offset: usize,
    },
}

/// 查询计划器
pub struct Planner;

impl Planner {
    /// 为 SELECT 语句生成执行计划
    pub fn plan_select(stmt: &SqlStatement, schema: &TableSchema) -> PlanNode {
        match stmt {
            SqlStatement::Select {
                table,
                columns,
                where_clause,
                order_by,
                limit,
                offset,
            } => {
                let mut plan: PlanNode = PlanNode::SeqScan {
                    table: table.clone(),
                };

                // 优化：如果 WHERE 是主键等值查询，走 PointLookup
                if let Some(wc) = where_clause {
                    if let Some(pk_value) = Self::is_pk_equals(wc, schema) {
                        plan = PlanNode::PointLookup {
                            table: table.clone(),
                            pk: pk_value,
                        };
                    } else {
                        plan = PlanNode::Filter {
                            input: Box::new(plan),
                            predicate: wc.clone(),
                        };
                    }
                }

                // 投影：解析列选择
                let cols = if columns.is_empty() || (columns.len() == 1 && columns[0] == "*") {
                    schema.columns.iter().map(|c| c.name.clone()).collect()
                } else {
                    columns.clone()
                };

                // 排序（在投影之前，使用 schema 列）
                if let Some(ob) = order_by {
                    plan = PlanNode::Sort {
                        input: Box::new(plan),
                        order_by: ob.clone(),
                    };
                }

                plan = PlanNode::Projection {
                    input: Box::new(plan),
                    columns: cols,
                };

                // 分页
                if limit.is_some() || offset.is_some() {
                    let limit_val = limit.unwrap_or(usize::MAX);
                    let offset_val = offset.unwrap_or(0);
                    plan = PlanNode::Limit {
                        input: Box::new(plan),
                        limit: limit_val,
                        offset: offset_val,
                    };
                }

                plan
            }
            _ => panic!("plan_select 只接收 SELECT 语句"),
        }
    }

    /// 检查 WHERE 是否为主键等值查询
    fn is_pk_equals(wc: &WhereClause, schema: &TableSchema) -> Option<Value> {
        if let WhereClause::Simple {
            column,
            operator,
            value,
        } = wc
        {
            if matches!(operator, ComparisonOp::Eq) {
                if let Some(pk_col) = schema
                    .columns
                    .iter()
                    .find(|c| c.is_primary_key && c.name == *column)
                {
                    return Some(value.clone());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::types::ColumnType;

    #[test]
    fn test_plan_pk_point_lookup() {
        let schema = TableSchema {
            name: "test".into(),
            columns: vec![
                crate::sql::types::ColumnDef {
                    name: "id".into(),
                    col_type: ColumnType::Integer,
                    nullable: false,
                    is_primary_key: true,
                    default: None,
                },
                crate::sql::types::ColumnDef {
                    name: "name".into(),
                    col_type: ColumnType::Text,
                    nullable: false,
                    is_primary_key: false,
                    default: None,
                },
            ],
            primary_key: vec!["id".into()],
        };

        let stmt = SqlStatement::Select {
            table: "test".into(),
            columns: vec!["*".into()],
            where_clause: Some(WhereClause::Simple {
                column: "id".into(),
                operator: ComparisonOp::Eq,
                value: Value::Integer(1),
            }),
            order_by: None,
            limit: None,
            offset: None,
        };

        let plan = Planner::plan_select(&stmt, &schema);
        match plan {
            PlanNode::Projection { input, .. } => match *input {
                PlanNode::PointLookup { table, pk } => {
                    assert_eq!(table, "test");
                    assert_eq!(pk, Value::Integer(1));
                }
                _ => panic!("期望 PointLookup"),
            },
            _ => panic!("期望 Projection"),
        }
    }

    #[test]
    fn test_plan_seq_scan() {
        let schema = TableSchema {
            name: "test".into(),
            columns: vec![crate::sql::types::ColumnDef {
                name: "id".into(),
                col_type: ColumnType::Integer,
                nullable: false,
                is_primary_key: true,
                default: None,
            }],
            primary_key: vec!["id".into()],
        };

        let stmt = SqlStatement::Select {
            table: "test".into(),
            columns: vec!["*".into()],
            where_clause: None,
            order_by: None,
            limit: None,
            offset: None,
        };

        let plan = Planner::plan_select(&stmt, &schema);
        match plan {
            PlanNode::Projection { input, .. } => match *input {
                PlanNode::SeqScan { table } => assert_eq!(table, "test"),
                _ => panic!("期望 SeqScan"),
            },
            _ => panic!("期望 Projection"),
        }
    }
}
