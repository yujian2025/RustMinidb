//! 查询计划器
//!
//! 将 SqlStatement 转换为执行计划（PlanNode）。
//! MVP 的优化策略：主键等值查询走 PointLookup，其他走 SeqScan + 内存过滤。

use crate::sql::parser::{AggregateDef, ComparisonOp, OrderBy, SqlStatement, WhereClause};
use crate::sql::types::Value;
use crate::error::Result;
use crate::storage::schema::{IndexDef, TableSchema};

/// 查询计划节点
#[derive(Debug)]
#[non_exhaustive]
pub enum PlanNode {
    /// 全表扫描
    SeqScan { table: String },
    /// 主键点查（走主键索引）
    PointLookup { table: String, pk: Value },
    /// 二级索引扫描（走二级索引）
    IndexScan {
        table: String,
        index_name: String,
        key: Value,
    },
    /// 过滤
    Filter {
        input: Box<PlanNode>,
        predicate: WhereClause,
    },
    /// 聚合（COUNT/SUM/AVG/MIN/MAX + GROUP BY）
    Aggregate {
        input: Box<PlanNode>,
        aggregates: Vec<AggregateDef>,
        group_by: Vec<String>,
    },
    /// HAVING 过滤（聚合后过滤）
    Having {
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
    pub fn plan_select(
        stmt: &SqlStatement,
        schema: &TableSchema,
        indexes: &[IndexDef],
    ) -> Result<PlanNode> {
        match stmt {
            SqlStatement::Select {
                table,
                columns,
                where_clause,
                aggregates,
                group_by,
                having,
                order_by,
                limit,
                offset,
            } => {
                let mut plan: PlanNode = PlanNode::SeqScan {
                    table: table.clone(),
                };

                // 如果使用了聚合函数或 GROUP BY，需要聚合节点
                let has_aggregation = !aggregates.is_empty() || !group_by.is_empty();

                // 优化：检查能否使用索引（主键索引优先，二级索引其次）
                if let Some(wc) = where_clause {
                    if let Some(pk_value) = Self::is_pk_equals(wc, schema) {
                        // 主键等值查询 → PointLookup
                        plan = PlanNode::PointLookup {
                            table: table.clone(),
                            pk: pk_value,
                        };
                    } else if let Some((idx_name, idx_key)) =
                        Self::is_index_match(wc, schema, indexes)
                    {
                        // 二级索引等值查询 → IndexScan
                        plan = PlanNode::IndexScan {
                            table: table.clone(),
                            index_name: idx_name,
                            key: idx_key,
                        };
                    } else {
                        plan = PlanNode::Filter {
                            input: Box::new(plan),
                            predicate: wc.clone(),
                        };
                    }
                }

                // 插入聚合节点（如果有聚合函数或 GROUP BY）
                if has_aggregation {
                    plan = PlanNode::Aggregate {
                        input: Box::new(plan),
                        aggregates: aggregates.clone(),
                        group_by: group_by.clone(),
                    };
                }

                // HAVING 过滤（聚合后）
                if let Some(h) = having {
                    plan = PlanNode::Having {
                        input: Box::new(plan),
                        predicate: h.clone(),
                    };
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

                Ok(plan)
            }
            _ => Err(crate::error::ExecError::NotImplemented(
                "plan_select 只接收 SELECT 语句".into()
            ).into()),
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
                if let Some(_pk_col) = schema
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

    /// 检查 WHERE 是否能匹配二级索引（等值查询）
    fn is_index_match(
        wc: &WhereClause,
        _schema: &TableSchema,
        indexes: &[IndexDef],
    ) -> Option<(String, Value)> {
        if let WhereClause::Simple {
            column,
            operator,
            value,
        } = wc
        {
            if matches!(operator, ComparisonOp::Eq) {
                // 检查该列是否有索引
                for idx in indexes {
                    if idx.columns.len() == 1 && idx.columns[0] == *column {
                        // 单列索引匹配
                        return Some((idx.name.clone(), value.clone()));
                    }
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
                    comment: None,
                },
                crate::sql::types::ColumnDef {
                    name: "name".into(),
                    col_type: ColumnType::Text,
                    nullable: false,
                    is_primary_key: false,
                    default: None,
                    comment: None,
                },
            ],
            primary_key: vec!["id".into()],
            comment: None,
        };

        let stmt = SqlStatement::Select {
            table: "test".into(),
            columns: vec!["*".into()],
            where_clause: Some(WhereClause::Simple {
                column: "id".into(),
                operator: ComparisonOp::Eq,
                value: Value::Integer(1),
            }),
            aggregates: vec![],
            group_by: vec![],
            having: None,
            order_by: None,
            limit: None,
            offset: None,
        };

        let plan = Planner::plan_select(&stmt, &schema, &[]).unwrap();
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
                comment: None,
            }],
            primary_key: vec!["id".into()],
            comment: None,
        };

        let stmt = SqlStatement::Select {
            table: "test".into(),
            columns: vec!["*".into()],
            where_clause: None,
            aggregates: vec![],
            group_by: vec![],
            having: None,
            order_by: None,
            limit: None,
            offset: None,
        };

        let plan = Planner::plan_select(&stmt, &schema, &[]).unwrap();
        match plan {
            PlanNode::Projection { input, .. } => match *input {
                PlanNode::SeqScan { table } => assert_eq!(table, "test"),
                _ => panic!("期望 SeqScan"),
            },
            _ => panic!("期望 Projection"),
        }
    }
}
