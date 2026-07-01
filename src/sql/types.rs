//! SQL 数据类型系统
//!
//! 定义 RustMinidb 支持的 SQL 数据类型、运行时值和行结构。

use serde::{Deserialize, Serialize};

/// RustMinidb 支持的 SQL 数据类型（MVP）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ColumnType {
    /// 有符号 64 位整数
    Integer,
    /// 64 位浮点数
    Float,
    /// UTF-8 字符串，最大 64KB
    Text,
    /// 二进制数据，最大 1MB
    Blob,
    /// 布尔值
    Boolean,
    /// 时间戳（微秒精度，UTC）
    Timestamp,
    /// 空值（仅作为占位，不作为列类型）
    Null,
}

impl ColumnType {
    /// 获取默认值
    pub fn default_value(&self) -> Value {
        match self {
            ColumnType::Integer => Value::Integer(0),
            ColumnType::Float => Value::Float(0.0),
            ColumnType::Text => Value::Text(String::new()),
            ColumnType::Blob => Value::Blob(Vec::new()),
            ColumnType::Boolean => Value::Boolean(false),
            ColumnType::Timestamp => Value::Timestamp(0),
            ColumnType::Null => Value::Null,
        }
    }
}

/// 运行时值
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Integer(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
    Boolean(bool),
    Timestamp(i64), // Unix 微秒
    Null,
}

impl Value {
    /// 将值转换为可 JSON 序列化的格式
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Value::Integer(v) => serde_json::Value::Number((*v).into()),
            Value::Float(v) => {
                serde_json::Value::Number(serde_json::Number::from_f64(*v).unwrap_or(
                    serde_json::Number::from_f64(0.0).unwrap(),
                ))
            }
            Value::Text(v) => serde_json::Value::String(v.clone()),
            Value::Blob(v) => serde_json::Value::String(hex::encode(v)),
            Value::Boolean(v) => serde_json::Value::Bool(*v),
            Value::Timestamp(v) => serde_json::Value::String(
                chrono::DateTime::from_timestamp_micros(*v)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "invalid_timestamp".to_string()),
            ),
            Value::Null => serde_json::Value::Null,
        }
    }

    /// 尝试将 Value 解析为指定的 ColumnType
    pub fn coerce_for_type(&self, target: &ColumnType) -> Option<Value> {
        match (target, self) {
            // 文本到时间戳的转换
            (ColumnType::Timestamp, Value::Text(s)) => {
                // 尝试解析 ISO 8601
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                    return Some(Value::Timestamp(dt.timestamp_micros()));
                }
                // 尝试解析 UTC
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&format!("{}T00:00:00Z", s))
                {
                    return Some(Value::Timestamp(dt.timestamp_micros()));
                }
                None
            }
            // 整数到浮点数的转换
            (ColumnType::Float, Value::Integer(i)) => Some(Value::Float(*i as f64)),
            // 浮点数到整数的转换
            (ColumnType::Integer, Value::Float(f)) => Some(Value::Integer(*f as i64)),
            // 文本到数字的转换
            (ColumnType::Integer, Value::Text(s)) => s.parse::<i64>().ok().map(Value::Integer),
            (ColumnType::Float, Value::Text(s)) => s.parse::<f64>().ok().map(Value::Float),
            // 其他类型直接匹配
            _ if type_matches(target, self) => Some(self.clone()),
            _ => None,
        }
    }
}

/// 检查值是否匹配列类型
pub fn type_matches(col_type: &ColumnType, val: &Value) -> bool {
    matches!(
        (col_type, val),
        (ColumnType::Integer, Value::Integer(_))
            | (ColumnType::Float, Value::Float(_))
            | (ColumnType::Text, Value::Text(_))
            | (ColumnType::Blob, Value::Blob(_))
            | (ColumnType::Boolean, Value::Boolean(_))
            | (ColumnType::Timestamp, Value::Timestamp(_))
    )
}

/// 列定义
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub col_type: ColumnType,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub default: Option<Value>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl std::fmt::Display for ColumnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColumnType::Integer => write!(f, "INTEGER"),
            ColumnType::Float => write!(f, "FLOAT"),
            ColumnType::Text => write!(f, "TEXT"),
            ColumnType::Blob => write!(f, "BLOB"),
            ColumnType::Boolean => write!(f, "BOOLEAN"),
            ColumnType::Timestamp => write!(f, "TIMESTAMP"),
            ColumnType::Null => write!(f, "NULL"),
        }
    }
}

/// 数据库行：值的有序列表
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub values: Vec<Value>,
}

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }
}
