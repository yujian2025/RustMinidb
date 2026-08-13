//! 基于 redb 的存储引擎实现
//!
//! redb 是一个纯 Rust 的嵌入式 KV 存储引擎，支持 ACID 事务。
//! 架构：
//! - `__schemas__` 表存储所有表的元数据（TableSchema）
//! - `__data__{table}` 表存储用户数据（主键 → 行）

use std::path::Path;
use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};

use crate::error::{EngineError, Result};
use crate::sql::types::{Row, Value};
use crate::storage::encoding::serialize_value;
use crate::storage::engine::{SharedEngine, StorageEngine};
use crate::storage::schema::{IndexDef, TableSchema};

/// 基于 redb 的存储引擎实现
pub struct RedbEngine {
    db: Database,
}

// redb 的表定义
const SCHEMA_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("__schemas__");
const INDEXES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("__indexes__");
const DATA_TABLE_PREFIX: &str = "__data__";

impl RedbEngine {
    /// 打开数据库文件，不存在则创建
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Database::create(path)?;
        // 初始化 schema 表和索引元数据表
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(SCHEMA_TABLE)?;
            let _ = write_txn.open_table(INDEXES_TABLE)?;
        }
        write_txn.commit()?;
        Ok(Self { db })
    }

    /// 获取用户数据表的 redb 名称
    fn data_table_name(table: &str) -> String {
        format!("{}{}", DATA_TABLE_PREFIX, table)
    }
}

impl StorageEngine for RedbEngine {
    fn create_table(&self, schema: &TableSchema) -> Result<()> {
        let write_txn = self.db.begin_write()?;

        // 检查表是否已存在
        {
            let schema_table = write_txn.open_table(SCHEMA_TABLE)?;
            if schema_table.get(schema.name.as_str())?.is_some() {
                return Err(EngineError::TableAlreadyExists(schema.name.clone()).into());
            }
        }

        // 保存 schema
        {
            let mut schema_table = write_txn.open_table(SCHEMA_TABLE)?;
            let encoded = bincode::serialize(schema)?;
            schema_table.insert(schema.name.as_str(), encoded.as_slice())?;
        }

        // 创建数据表
        {
            let data_table_name = Self::data_table_name(&schema.name);
            let _ = write_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
        }

        write_txn.commit()?;
        Ok(())
    }

    fn drop_table(&self, name: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;

        // 删除 schema
        {
            let mut schema_table = write_txn.open_table(SCHEMA_TABLE)?;
            schema_table.remove(name)?;
        }

        // 删除数据表
        {
            let data_table_name = Self::data_table_name(name);
            // redb 不支持直接删除表，但可以清空
            // 这里我们通过删除 schema 来标记表已删除
            // 数据表在下次 compact 时会被回收
            let mut data_table = write_txn.open_table::<&[u8], &[u8]>(
                TableDefinition::new(&data_table_name),
            )?;
            // 遍历删除所有数据
            let keys: Vec<Vec<u8>> = data_table
                .iter()?
                .map(|item| item.map(|(k, _)| k.value().to_vec()))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for key in keys {
                data_table.remove(key.as_slice())?;
            }
        }

        write_txn.commit()?;
        Ok(())
    }

    fn get_schema(&self, name: &str) -> Result<Option<TableSchema>> {
        let read_txn = self.db.begin_read()?;
        let schema_table = read_txn.open_table(SCHEMA_TABLE)?;

        match schema_table.get(name)? {
            Some(bytes) => {
                let schema: TableSchema = bincode::deserialize(bytes.value())?;
                Ok(Some(schema))
            }
            None => Ok(None),
        }
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let read_txn = self.db.begin_read()?;
        let schema_table = read_txn.open_table(SCHEMA_TABLE)?;

        let mut tables = Vec::new();
        for item in schema_table.iter()? {
            let (key, _) = item?;
            tables.push(key.value().to_string());
        }
        Ok(tables)
    }

    fn insert_row(&self, table: &str, row: Row) -> Result<()> {
        let schema = self
            .get_schema(table)?
            .ok_or_else(|| EngineError::TableNotFound(table.to_string()))?;

        // 获取主键值
        let pk_idx = schema
            .pk_index()
            .ok_or_else(|| EngineError::NoPrimaryKey(table.to_string()))?;
        let pk = &row.values[pk_idx];

        // 检查主键是否已存在
        let pk_bytes = serialize_value(pk);
        let read_txn = self.db.begin_read()?;
        let data_table_name = Self::data_table_name(table);
        let data_table = read_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
        if data_table.get(pk_bytes.as_slice())?.is_some() {
            return Err(EngineError::PrimaryKeyConflict(format!(
                "表 '{}' 中主键 '{:?}' 已存在",
                table, pk
            ))
            .into());
        }
        drop(read_txn);

        // 序列化行数据
        let row_bytes = bincode::serialize(&row)?;

        let write_txn = self.db.begin_write()?;
        {
            let data_table_name = Self::data_table_name(table);
            let mut data_table = write_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
            data_table.insert(pk_bytes.as_slice(), row_bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn get_row(&self, table: &str, pk: &Value) -> Result<Option<Row>> {
        let read_txn = self.db.begin_read()?;
        let data_table_name = Self::data_table_name(table);
        let data_table = read_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;

        let pk_bytes = serialize_value(pk);
        match data_table.get(pk_bytes.as_slice())? {
            Some(value_bytes) => {
                let row: Row = bincode::deserialize(value_bytes.value())?;
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }

    fn scan_table(&self, table: &str) -> Result<Vec<Row>> {
        let read_txn = self.db.begin_read()?;
        let data_table_name = Self::data_table_name(table);
        let data_table = read_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;


        let mut rows = Vec::new();
        for item in data_table.iter()? {
            let (_, value) = item?;
            let row: Row = bincode::deserialize(value.value())?;
            rows.push(row);
        }
        Ok(rows)
    }

    fn update_row(&self, table: &str, pk: &Value, row: Row) -> Result<()> {
        let schema = self
            .get_schema(table)?
            .ok_or_else(|| EngineError::TableNotFound(table.to_string()))?;

        // 验证新数据的列数
        if row.values.len() != schema.columns.len() {
            return Err(EngineError::NoPrimaryKey(format!(
                "列数不匹配: 期望 {} 列，实际 {} 列",
                schema.columns.len(),
                row.values.len()
            ))
            .into());
        }

        let pk_bytes = serialize_value(pk);
        let row_bytes = bincode::serialize(&row)?;

        let write_txn = self.db.begin_write()?;
        {
            let data_table_name = Self::data_table_name(table);
            let mut data_table = write_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
            // redb 的 insert 会覆盖已有值，等同于 update
            data_table.insert(pk_bytes.as_slice(), row_bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn delete_row(&self, table: &str, pk: &Value) -> Result<bool> {
        let pk_bytes = serialize_value(pk);
        let write_txn = self.db.begin_write()?;
        let data_table_name = Self::data_table_name(table);
        let mut data_table = write_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
        let result = data_table.remove(pk_bytes.as_slice())?.is_some();
        drop(data_table);
        write_txn.commit()?;
        Ok(result)
    }

    fn row_count(&self, table: &str) -> Result<u64> {
        let read_txn = self.db.begin_read()?;
        let data_table_name = Self::data_table_name(table);
        let data_table = read_txn.open_table(TableDefinition::<&[u8], &[u8]>::new(&data_table_name))?;
        let mut count = 0u64;
        for _ in data_table.iter()? {
            count += 1;
        }
        Ok(count)
    }

    fn table_exists(&self, name: &str) -> Result<bool> {
        Ok(self.get_schema(name)?.is_some())
    }

    fn update_schema(&self, schema: &TableSchema) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut schema_table = write_txn.open_table(SCHEMA_TABLE)?;
            if schema_table.get(schema.name.as_str())?.is_some() {
                let encoded = bincode::serialize(schema)?;
                schema_table.insert(schema.name.as_str(), encoded.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── 索引方法实现 ──

    fn create_index(&self, index: &IndexDef) -> Result<()> {
        let write_txn = self.db.begin_write()?;

        // 检查索引是否已存在
        {
            let indexes_table = write_txn.open_table(INDEXES_TABLE)?;
            let key = format!("{}:{}", index.table_name, index.name);
            if indexes_table.get(key.as_str())?.is_some() {
                return Err(EngineError::RedbTable(
                    format!("索引 '{}' 已存在于表 '{}'", index.name, index.table_name)
                ).into());
            }
        }
        drop(write_txn);

        let write_txn = self.db.begin_write()?;

        // 保存索引元数据
        {
            let mut indexes_table = write_txn.open_table(INDEXES_TABLE)?;
            let encoded = bincode::serialize(index)?;
            let key = format!("{}:{}", index.table_name, index.name);
            indexes_table.insert(key.as_str(), encoded.as_slice())?;
        }

        // 创建索引数据表
        {
            let internal_name = index.internal_table_name();
            let _ = write_txn.open_table::<&[u8], &[u8]>(
                TableDefinition::new(&internal_name),
            )?;
        }

        write_txn.commit()?;
        Ok(())
    }

    fn drop_index(&self, table: &str, index_name: &str) -> Result<()> {
        let write_txn = self.db.begin_write()?;

        // 删除索引元数据
        {
            let mut indexes_table = write_txn.open_table(INDEXES_TABLE)?;
            let key = format!("{}:{}", table, index_name);
            indexes_table.remove(key.as_str())?;
        }

        // 清空索引数据表（redb 不支持直接删除表）
        {
            let internal_name = format!("__idx__{}__{}", table, index_name);
            let mut idx_table = write_txn.open_table::<&[u8], &[u8]>(
                TableDefinition::new(&internal_name),
            )?;
            let keys: Vec<Vec<u8>> = idx_table
                .iter()?
                .map(|item| item.map(|(k, _)| k.value().to_vec()))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for key in keys {
                idx_table.remove(key.as_slice())?;
            }
        }

        write_txn.commit()?;
        Ok(())
    }

    fn list_indexes(&self, table: &str) -> Result<Vec<IndexDef>> {
        let read_txn = self.db.begin_read()?;
        let indexes_table = read_txn.open_table(INDEXES_TABLE)?;

        let mut indexes = Vec::new();
        let prefix = format!("{}:", table);
        for item in indexes_table.iter()? {
            let (key, value) = item?;
            let key_str = key.value();
            if key_str.starts_with(&prefix) {
                let index: IndexDef = bincode::deserialize(value.value())?;
                indexes.push(index);
            }
        }
        Ok(indexes)
    }

    fn insert_index_entry(&self, index: &IndexDef, key: &[u8], pk: &[u8]) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        let internal_name = index.internal_table_name();
        let mut idx_table = write_txn.open_table::<&[u8], &[u8]>(
            TableDefinition::new(&internal_name),
        )?;

        // 复合键: 索引值 + 主键（支持重复键值）
        let mut composite_key = key.to_vec();
        composite_key.extend_from_slice(pk);
        idx_table.insert(composite_key.as_slice(), pk)?;
        drop(idx_table);

        write_txn.commit()?;
        Ok(())
    }

    fn delete_index_entry(&self, index: &IndexDef, key: &[u8], pk: &[u8]) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        let internal_name = index.internal_table_name();
        let mut idx_table = write_txn.open_table::<&[u8], &[u8]>(
            TableDefinition::new(&internal_name),
        )?;

        let mut composite_key = key.to_vec();
        composite_key.extend_from_slice(pk);
        idx_table.remove(composite_key.as_slice())?;
        drop(idx_table);

        write_txn.commit()?;
        Ok(())
    }

    fn scan_index_eq(&self, index: &IndexDef, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        let read_txn = self.db.begin_read()?;
        let internal_name = index.internal_table_name();
        let idx_table = read_txn.open_table::<&[u8], &[u8]>(
            TableDefinition::new(&internal_name),
        )?;

        let mut pks = Vec::new();
        for item in idx_table.iter()? {
            let (k, value) = item?;
            let k_bytes = k.value();
            // 匹配前缀：key 和 composite_key 的前 len(key) 字节相同
            if k_bytes.len() >= key.len() && k_bytes[..key.len()] == key[..] {
                pks.push(value.value().to_vec());
            }
        }
        Ok(pks)
    }
}

/// 创建共享的 RedbEngine 实例
pub fn create_engine<P: AsRef<Path>>(path: P) -> Result<SharedEngine> {
    let engine = RedbEngine::open(path)?;
    Ok(Arc::new(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::types::{ColumnDef, ColumnType, Value};
    use tempfile::TempDir;

    fn create_test_schema() -> TableSchema {
        TableSchema {
            name: "test".into(),
            columns: vec![
                ColumnDef {
                    name: "id".into(),
                    col_type: ColumnType::Integer,
                    nullable: false,
                    is_primary_key: true,
                    default: None,
                    auto_increment: false,
                    unique: false,
                    comment: None,
                },
                ColumnDef {
                    name: "name".into(),
                    col_type: ColumnType::Text,
                    nullable: false,
                    is_primary_key: false,
                    default: None,
                    auto_increment: false,
                    unique: false,
                    comment: None,
                },
            ],
            primary_key: vec!["id".into()],
            comment: None,
        }
    }

    #[test]
    fn test_create_table() {
        let dir = TempDir::new().unwrap();
        let engine = RedbEngine::open(dir.path().join("test.db")).unwrap();
        let schema = create_test_schema();
        engine.create_table(&schema).unwrap();

        let loaded = engine.get_schema("test").unwrap().unwrap();
        assert_eq!(loaded.name, "test");
        assert_eq!(loaded.columns.len(), 2);
    }

    #[test]
    fn test_insert_and_get() {
        let dir = TempDir::new().unwrap();
        let engine = RedbEngine::open(dir.path().join("test.db")).unwrap();
        engine.create_table(&create_test_schema()).unwrap();

        engine
            .insert_row(
                "test",
                Row {
                    values: vec![Value::Integer(1), Value::Text("Alice".into())],
                },
            )
            .unwrap();
        engine
            .insert_row(
                "test",
                Row {
                    values: vec![Value::Integer(2), Value::Text("Bob".into())],
                },
            )
            .unwrap();

        let row = engine.get_row("test", &Value::Integer(1)).unwrap().unwrap();
        assert_eq!(row.values[1], Value::Text("Alice".into()));

        let rows = engine.scan_table("test").unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_update_and_delete() {
        let dir = TempDir::new().unwrap();
        let engine = RedbEngine::open(dir.path().join("test.db")).unwrap();
        engine.create_table(&create_test_schema()).unwrap();

        engine
            .insert_row(
                "test",
                Row {
                    values: vec![Value::Integer(1), Value::Text("Alice".into())],
                },
            )
            .unwrap();

        engine
            .update_row(
                "test",
                &Value::Integer(1),
                Row {
                    values: vec![Value::Integer(1), Value::Text("Updated".into())],
                },
            )
            .unwrap();

        let row = engine.get_row("test", &Value::Integer(1)).unwrap().unwrap();
        assert_eq!(row.values[1], Value::Text("Updated".into()));

        let deleted = engine.delete_row("test", &Value::Integer(1)).unwrap();
        assert!(deleted);
        assert!(engine.get_row("test", &Value::Integer(1)).unwrap().is_none());
    }

    #[test]
    fn test_primary_key_conflict() {
        let dir = TempDir::new().unwrap();
        let engine = RedbEngine::open(dir.path().join("test.db")).unwrap();
        engine.create_table(&create_test_schema()).unwrap();

        engine
            .insert_row(
                "test",
                Row {
                    values: vec![Value::Integer(1), Value::Text("Alice".into())],
                },
            )
            .unwrap();

        let result = engine.insert_row(
            "test",
            Row {
                values: vec![Value::Integer(1), Value::Text("Bob".into())],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_list_tables() {
        let dir = TempDir::new().unwrap();
        let engine = RedbEngine::open(dir.path().join("test.db")).unwrap();
        engine.create_table(&create_test_schema()).unwrap();

        let tables = engine.list_tables().unwrap();
        assert!(tables.contains(&"test".to_string()));
    }
}
