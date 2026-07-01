//! 存储引擎接口定义
//!
//! StorageEngine trait 定义了存储层的统一抽象，
//! MVP 使用 redb 实现，未来可替换为其他后端。

use std::sync::Arc;

use crate::error::Result;
use crate::sql::types::{Row, Value};
use crate::storage::schema::TableSchema;

/// 存储引擎接口
pub trait StorageEngine: Send + Sync {
    /// 创建表
    fn create_table(&self, schema: &TableSchema) -> Result<()>;

    /// 删除表
    fn drop_table(&self, name: &str) -> Result<()>;

    /// 获取表模式
    fn get_schema(&self, name: &str) -> Result<Option<TableSchema>>;

    /// 列出所有表
    fn list_tables(&self) -> Result<Vec<String>>;

    /// 插入一行
    fn insert_row(&self, table: &str, row: Row) -> Result<()>;

    /// 按主键查询
    fn get_row(&self, table: &str, pk: &Value) -> Result<Option<Row>>;

    /// 全表扫描
    fn scan_table(&self, table: &str) -> Result<Vec<Row>>;

    /// 按主键更新（覆盖）
    fn update_row(&self, table: &str, pk: &Value, row: Row) -> Result<()>;

    /// 按主键删除
    fn delete_row(&self, table: &str, pk: &Value) -> Result<bool>;

    /// 获取表行数
    fn row_count(&self, table: &str) -> Result<u64>;

    /// 表是否存在
    fn table_exists(&self, name: &str) -> Result<bool>;

    /// 更新表模式（不影响已有数据）
    fn update_schema(&self, schema: &TableSchema) -> Result<()>;
}

/// 线程安全共享的存储引擎
pub type SharedEngine = Arc<dyn StorageEngine>;
