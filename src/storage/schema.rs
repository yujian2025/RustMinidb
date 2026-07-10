//! 表模式（Schema）与便捷构造器

use crate::sql::types::{ColumnDef, ColumnType, Row, Value};

/// 表模式
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl TableSchema {
    /// 验证值数组是否符合模式定义
    pub fn validate_row(&self, values: &[Value]) -> Result<(), String> {
        if values.len() != self.columns.len() {
            return Err(format!(
                "列数不匹配: 期望 {} 列，实际 {} 列",
                self.columns.len(),
                values.len()
            ));
        }

        for (_i, (col, val)) in self.columns.iter().zip(values.iter()).enumerate() {
            if *val == Value::Null && !col.nullable {
                return Err(format!("列 '{}' 不能为 NULL", col.name));
            }

            if *val != Value::Null && !Self::type_matches(&col.col_type, val) {
                return Err(format!(
                    "列 '{}' 类型不匹配: 期望 {:?}，实际 {:?}",
                    col.name, col.col_type, val
                ));
            }
        }

        Ok(())
    }

    /// 获取主键列的索引
    pub fn pk_index(&self) -> Option<usize> {
        self.columns.iter().position(|c| c.is_primary_key)
    }

    /// 获取某列名的索引
    pub fn col_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// 检查类型是否匹配
    fn type_matches(col_type: &ColumnType, val: &Value) -> bool {
        crate::sql::types::type_matches(col_type, val)
    }
}

/// 从列名到值的映射构建行
pub fn row_from_map(schema: &TableSchema, map: &std::collections::HashMap<String, Value>) -> Result<Row, String> {
    let mut values = Vec::with_capacity(schema.columns.len());
    for col in &schema.columns {
        match map.get(&col.name) {
            Some(val) => values.push(val.clone()),
            None if col.nullable => values.push(Value::Null),
            None => {
                // 尝试使用默认值
                if let Some(default) = &col.default {
                    values.push(default.clone());
                } else {
                    return Err(format!("缺少列 '{}' 的值", col.name));
                }
            }
        }
    }
    Ok(Row { values })
}

/// 获取行中主键值
pub fn pk_value<'a>(row: &'a Row, schema: &'a TableSchema) -> Option<&'a Value> {
    let idx = schema.pk_index()?;
    Some(&row.values[idx])
}
