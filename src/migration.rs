//! 数据迁移与导出功能（增强版）
//!
//! 支持将当前数据库中的所有表和数据导出为标准 SQL 语句，
//! 支持多种 SQL 方言、进度回调和分批事务。
//!
//! # 特性
//!
//! - 完整导出：CREATE TABLE + INSERT 数据
//! - 多方言支持：Standard / MySQL / PostgreSQL / SQLite
//! - DROP TABLE IF EXISTS 前缀
//! - 批量 INSERT（可配置每批行数）
//! - 进度回调（用于大数据库）
//! - 事务包裹导出
//! - SQL 语句拆分与导入

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::error::Result;
use crate::sql::types::{ColumnType, Value};
use crate::storage::engine::SharedEngine;
use crate::storage::schema::TableSchema;

// ── 导出配置 ──

/// SQL 导出配置
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// 是否包含 CREATE TABLE 语句
    pub include_create: bool,
    /// 是否包含 INSERT 数据
    pub include_data: bool,
    /// 是否包含 DROP TABLE IF EXISTS 前缀
    pub include_drop_table: bool,
    /// 每批 INSERT 的行数（0 表示全部放在一条语句中）
    pub batch_size: usize,
    /// 目标 SQL 方言兼容模式
    pub dialect: SqlDialect,
    /// 是否用事务包裹整个导出
    pub wrap_in_transaction: bool,
    /// 导出注释/说明
    pub include_comments: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            include_create: true,
            include_data: true,
            include_drop_table: false,
            batch_size: 100,
            dialect: SqlDialect::Standard,
            wrap_in_transaction: false,
            include_comments: true,
        }
    }
}

// ── SQL 方言 ──

/// SQL 方言
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SqlDialect {
    /// 标准 SQL（RustMinidb 原生）
    Standard,
    /// 兼容 MySQL
    MySQL,
    /// 兼容 PostgreSQL
    PostgreSQL,
    /// 兼容 SQLite
    SQLite,
}

impl SqlDialect {
    /// 返回列类型的 SQL 关键字
    pub fn type_name(&self, col_type: &ColumnType) -> &'static str {
        match (self, col_type) {
            (SqlDialect::MySQL, ColumnType::Integer) => "INT",
            (SqlDialect::MySQL, ColumnType::Float) => "DOUBLE",
            (SqlDialect::MySQL, ColumnType::Text) => "VARCHAR(65535)",
            (SqlDialect::MySQL, ColumnType::Blob) => "LONGBLOB",
            (SqlDialect::MySQL, ColumnType::Boolean) => "TINYINT(1)",
            (SqlDialect::MySQL, ColumnType::Timestamp) => "DATETIME(6)",
            (SqlDialect::PostgreSQL, ColumnType::Integer) => "BIGINT",
            (SqlDialect::PostgreSQL, ColumnType::Float) => "DOUBLE PRECISION",
            (SqlDialect::PostgreSQL, ColumnType::Text) => "TEXT",
            (SqlDialect::PostgreSQL, ColumnType::Blob) => "BYTEA",
            (SqlDialect::PostgreSQL, ColumnType::Boolean) => "BOOLEAN",
            (SqlDialect::PostgreSQL, ColumnType::Timestamp) => "TIMESTAMP",
            (SqlDialect::SQLite, ColumnType::Integer) => "INTEGER",
            (SqlDialect::SQLite, ColumnType::Float) => "REAL",
            (SqlDialect::SQLite, ColumnType::Text) => "TEXT",
            (SqlDialect::SQLite, ColumnType::Blob) => "BLOB",
            (SqlDialect::SQLite, ColumnType::Boolean) => "INTEGER",
            (SqlDialect::SQLite, ColumnType::Timestamp) => "TEXT",
            (SqlDialect::Standard, ColumnType::Integer) => "INTEGER",
            (SqlDialect::Standard, ColumnType::Float) => "FLOAT",
            (SqlDialect::Standard, ColumnType::Text) => "TEXT",
            (SqlDialect::Standard, ColumnType::Blob) => "BLOB",
            (SqlDialect::Standard, ColumnType::Boolean) => "BOOLEAN",
            (SqlDialect::Standard, ColumnType::Timestamp) => "TIMESTAMP",
            _ => "TEXT",
        }
    }

    /// 返回方言的表名引用
    pub fn quote_identifier(&self, name: &str) -> String {
        match self {
            SqlDialect::MySQL => format!("`{}`", name),
            SqlDialect::PostgreSQL => format!("\"{}\"", name),
            SqlDialect::SQLite => format!("`{}`", name),
            SqlDialect::Standard => name.to_string(),
        }
    }

    /// 返回值的 SQL 字面量表示
    pub fn format_value(&self, val: &Value) -> String {
        match val {
            Value::Integer(v) => v.to_string(),
            Value::Float(v) => {
                if *v == v.trunc() {
                    format!("{}.0", v)
                } else {
                    format!("{}", v)
                }
            }
            Value::Text(v) => {
                let escaped = v.replace('\'', "''");
                format!("'{}'", escaped)
            }
            Value::Blob(v) => {
                match self {
                    SqlDialect::PostgreSQL => format!("'\\x{}'", hex::encode(v)),
                    _ => format!("X'{}'", hex::encode(v)),
                }
            }
            Value::Boolean(v) => {
                match self {
                    SqlDialect::PostgreSQL => {
                        if *v { "TRUE".to_string() } else { "FALSE".to_string() }
                    }
                    _ => {
                        if *v { "1".to_string() } else { "0".to_string() }
                    }
                }
            }
            Value::Timestamp(v) => {
                let dt = chrono::DateTime::from_timestamp_micros(*v)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "1970-01-01 00:00:00".to_string());
                format!("'{}'", dt)
            }
            Value::Null => "NULL".to_string(),
        }
    }

    /// 返回方言的自增关键字
    pub fn auto_increment(&self) -> &'static str {
        match self {
            SqlDialect::MySQL => "AUTO_INCREMENT",
            SqlDialect::PostgreSQL => "GENERATED BY DEFAULT AS IDENTITY",
            SqlDialect::SQLite => "AUTOINCREMENT",
            SqlDialect::Standard => "AUTO_INCREMENT",
        }
    }

    /// 返回方言的引擎/表选项后缀
    pub fn table_options_suffix(&self) -> &'static str {
        match self {
            SqlDialect::MySQL => " ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
            _ => "",
        }
    }

    /// 返回方言的事务控制语句
    pub fn begin_transaction(&self) -> &'static str {
        "BEGIN;\n"
    }

    pub fn commit_transaction(&self) -> &'static str {
        "COMMIT;\n"
    }
}

// ── 导出器 ──

/// 导出器
pub struct Exporter {
    engine: SharedEngine,
    config: ExportConfig,
}

impl Exporter {
    /// 创建新的导出器
    pub fn new(engine: SharedEngine) -> Self {
        Self {
            engine,
            config: ExportConfig::default(),
        }
    }

    /// 创建导出器并指定配置
    pub fn with_config(engine: SharedEngine, config: ExportConfig) -> Self {
        Self { engine, config }
    }

    /// 获取当前配置引用
    pub fn config(&self) -> &ExportConfig {
        &self.config
    }

    /// 获取配置的可变引用
    pub fn config_mut(&mut self) -> &mut ExportConfig {
        &mut self.config
    }

    // ── 导出到字符串 ──

    /// 将整个数据库导出为 SQL 字符串
    pub fn export_to_string(&self) -> Result<String> {
        let mut output = String::new();
        let dialect = self.config.dialect;
        let q = |s: &str| dialect.quote_identifier(s);

        // ── 文件头 ──
        if self.config.include_comments {
            output.push_str(&format!(
                "-- ============================================================\n\
                 -- RustMinidb Database Export\n\
                 -- Generated: {}\n\
                 -- Engine: redb (ACID, MVCC)\n\
                 -- Dialect: {:?}\n\
                 -- ============================================================\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                self.config.dialect,
            ));
        }

        // ── 事务包裹 ──
        if self.config.wrap_in_transaction {
            output.push_str(dialect.begin_transaction());
        }

        let tables = self.engine.list_tables()?;
        if tables.is_empty() {
            if self.config.include_comments {
                output.push_str("-- (empty database - no tables)\n");
            }
            if self.config.wrap_in_transaction {
                output.push_str(dialect.commit_transaction());
            }
            return Ok(output);
        }

        for table_name in &tables {
            let schema = self.engine.get_schema(table_name)?.ok_or_else(|| {
                crate::error::RustMinidbError::Exec(
                    crate::error::ExecError::TableNotFound(table_name.clone()),
                )
            })?;

            if self.config.include_comments {
                output.push_str(&format!(
                    "--\n-- Table structure for {}\n--\n\n",
                    q(table_name)
                ));
            }

            // DROP TABLE IF EXISTS
            if self.config.include_drop_table {
                output.push_str(&format!(
                    "DROP TABLE IF EXISTS {};\n",
                    q(table_name)
                ));
                output.push('\n');
            }

            // CREATE TABLE
            if self.config.include_create {
                output.push_str(&self.generate_create_table(&schema));
                output.push('\n');
            }

            // INSERT data
            if self.config.include_data {
                if self.config.include_comments {
                    output.push_str(&format!(
                        "--\n-- Data for {}\n--\n\n",
                        q(table_name)
                    ));
                }
                let data_sql = self.generate_insert_data(&schema)?;
                output.push_str(&data_sql);
                output.push('\n');
            }
        }

        // ── 文件尾 ──
        if self.config.wrap_in_transaction {
            output.push_str(dialect.commit_transaction());
        }

        if self.config.include_comments {
            output.push_str("-- Export complete\n");
        }

        Ok(output)
    }

    /// 导出单张表为 SQL 字符串
    pub fn export_table_to_string(&self, table_name: &str) -> Result<String> {
        let mut output = String::new();
        let dialect = self.config.dialect;
        let q = |s: &str| dialect.quote_identifier(s);

        let schema = self.engine.get_schema(table_name)?.ok_or_else(|| {
            crate::error::RustMinidbError::Exec(
                crate::error::ExecError::TableNotFound(table_name.to_string()),
            )
        })?;

        if self.config.include_comments {
            output.push_str(&format!(
                "-- Export table: {}\n\n",
                q(table_name)
            ));
        }

        // DROP TABLE IF EXISTS
        if self.config.include_drop_table {
            output.push_str(&format!(
                "DROP TABLE IF EXISTS {};\n\n",
                q(table_name)
            ));
        }

        // CREATE TABLE
        if self.config.include_create {
            output.push_str(&self.generate_create_table(&schema));
            output.push('\n');
        }

        // INSERT data
        if self.config.include_data {
            let data_sql = self.generate_insert_data(&schema)?;
            output.push_str(&data_sql);
        }

        Ok(output)
    }

    // ── 导出到文件 ──

    /// 将整个数据库导出到文件
    pub fn export_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let sql = self.export_to_string()?;
        fs::write(path, sql)?;
        Ok(())
    }

    /// 将整个数据库导出到 writer
    pub fn export_to_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let sql = self.export_to_string().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, e.to_string())
        })?;
        writer.write_all(sql.as_bytes())?;
        Ok(())
    }

    /// 带进度回调的流式导出到 writer
    pub fn export_to_writer_with_progress<W: Write>(
        &self,
        writer: &mut W,
        on_progress: Option<&dyn Fn(&str, usize, usize)>,
    ) -> io::Result<()> {
        let dialect = self.config.dialect;
        let _q = |s: &str| dialect.quote_identifier(s);

        // 文件头
        let header = if self.config.include_comments {
            format!(
                "-- ============================================================\n\
                 -- RustMinidb Database Export\n\
                 -- Generated: {}\n\
                 -- Engine: redb (ACID, MVCC)\n\
                 -- ============================================================\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            )
        } else {
            String::new()
        };
        writer.write_all(header.as_bytes())?;

        if self.config.wrap_in_transaction {
            writer.write_all(dialect.begin_transaction().as_bytes())?;
        }

        let tables = self.engine.list_tables().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, e.to_string())
        })?;

        for table_name in &tables {
            let schema_result = self.engine.get_schema(table_name);
            let schema = match schema_result {
                Ok(Some(s)) => s,
                _ => continue,
            };

            if let Some(cb) = on_progress {
                cb(table_name, 0, 0);
            }

            if self.config.include_drop_table {
                let drop = format!(
                    "DROP TABLE IF EXISTS {};\n\n",
                    dialect.quote_identifier(table_name)
                );
                writer.write_all(drop.as_bytes())?;
            }

            if self.config.include_create {
                let create = self.generate_create_table(&schema);
                writer.write_all(create.as_bytes())?;
                writer.write_all(b"\n")?;
            }

            if self.config.include_data {
                let rows = self.engine.scan_table(table_name).map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, e.to_string())
                })?;
                let total = rows.len();
                if total > 0 {
                    let insert_sql = self.generate_insert_data_from_rows(&schema, &rows);
                    writer.write_all(insert_sql.as_bytes())?;
                    writer.write_all(b"\n")?;

                    if let Some(cb) = on_progress {
                        cb(table_name, total, total);
                    }
                }
            }
        }

        if self.config.wrap_in_transaction {
            writer.write_all(dialect.commit_transaction().as_bytes())?;
        }

        writer.write_all(b"-- Export complete\n")?;
        writer.flush()?;
        Ok(())
    }

    // ── 内部辅助方法 ──

    /// 生成 CREATE TABLE 语句（增强版）
    fn generate_create_table(&self, schema: &TableSchema) -> String {
        let dialect = self.config.dialect;
        let q = |s: &str| dialect.quote_identifier(s);
        let mut sql = format!("CREATE TABLE IF NOT EXISTS {} (\n", q(&schema.name));

        let col_defs: Vec<String> = schema
            .columns
            .iter()
            .map(|col| {
                let mut def = format!("    {} {}", q(&col.name), dialect.type_name(&col.col_type));

                // 主键标记在列上
                if col.is_primary_key {
                    if dialect == SqlDialect::PostgreSQL && col.col_type == ColumnType::Integer {
                        def = format!(
                            "    {} {} {}",
                            q(&col.name),
                            dialect.type_name(&col.col_type),
                            dialect.auto_increment()
                        );
                    } else if dialect == SqlDialect::MySQL && col.col_type == ColumnType::Integer {
                        def.push_str(&format!(" {} NOT NULL", dialect.auto_increment()));
                    } else {
                        def.push_str(" PRIMARY KEY");
                    }
                }

                // NOT NULL（非主键列）
                if !col.is_primary_key {
                    if !col.nullable {
                        def.push_str(" NOT NULL");
                    } else {
                        def.push_str(" NULL");
                    }
                }

                // DEFAULT
                if let Some(ref default) = col.default {
                    def.push_str(&format!(" DEFAULT {}", dialect.format_value(default)));
                }

                // COMMENT
                if let Some(ref comment) = col.comment {
                    match dialect {
                        SqlDialect::MySQL => {
                            def.push_str(&format!(" COMMENT '{}'", comment.replace('\'', "''")));
                        }
                        _ => {
                            def.push_str(&format!(" /* {} */", comment));
                        }
                    }
                }

                def
            })
            .collect();

        sql.push_str(&col_defs.join(",\n"));

        // 追加表级 PRIMARY KEY
        let pk_cols: Vec<&str> = schema
            .columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| c.name.as_str())
            .collect();

        if pk_cols.len() > 1 || (dialect != SqlDialect::PostgreSQL && !pk_cols.is_empty()) {
            let pk_quoted: Vec<String> = pk_cols.iter().map(|c| q(c)).collect();
            if !pk_quoted.is_empty() {
                sql.push_str(",\n");
                sql.push_str(&format!("    PRIMARY KEY ({})", pk_quoted.join(", ")));
            }
        }

        sql.push_str("\n)");

        // 表选项
        let options = dialect.table_options_suffix();
        if !options.is_empty() {
            sql.push_str(options);
        }

        sql.push(';');

        // 表注释
        if let Some(ref comment) = schema.comment {
            match dialect {
                SqlDialect::MySQL => {
                    sql.push_str(&format!(
                        "\nALTER TABLE {} COMMENT = '{}';",
                        q(&schema.name),
                        comment.replace('\'', "''")
                    ));
                }
                _ => {
                    sql = format!("/* {} */\n{}", comment, sql);
                }
            }
        }

        sql
    }

    /// 生成 INSERT 语句（从存储引擎读取数据）
    fn generate_insert_data(&self, schema: &TableSchema) -> Result<String> {
        let rows = self.engine.scan_table(&schema.name)?;
        Ok(self.generate_insert_data_from_rows(schema, &rows))
    }

    /// 从已加载的行数据生成 INSERT 语句
    fn generate_insert_data_from_rows(&self, schema: &TableSchema, rows: &[crate::sql::types::Row]) -> String {
        if rows.is_empty() {
            return String::new();
        }

        let dialect = self.config.dialect;
        let q = |s: &str| dialect.quote_identifier(s);
        let col_names: Vec<String> = schema.columns.iter().map(|c| q(&c.name)).collect();
        let cols_part = col_names.join(", ");
        let batch_size = self.config.batch_size;
        let mut sql = String::new();

        if batch_size == 0 || batch_size >= rows.len() {
            sql.push_str(&self.build_single_insert(schema.name.as_str(), &cols_part, rows, dialect));
        } else {
            for chunk in rows.chunks(batch_size) {
                sql.push_str(&self.build_single_insert(
                    schema.name.as_str(),
                    &cols_part,
                    chunk,
                    dialect,
                ));
                sql.push('\n');
            }
        }

        sql
    }

    /// 构建单条 INSERT 语句
    fn build_single_insert(
        &self,
        table_name: &str,
        cols_part: &str,
        rows: &[crate::sql::types::Row],
        dialect: SqlDialect,
    ) -> String {
        let q = |s: &str| dialect.quote_identifier(s);
        let values_parts: Vec<String> = rows
            .iter()
            .map(|row| {
                let vals: Vec<String> = row
                    .values
                    .iter()
                    .map(|v| dialect.format_value(v))
                    .collect();
                format!("({})", vals.join(", "))
            })
            .collect();

        format!(
            "INSERT INTO {} ({}) VALUES\n{};",
            q(table_name),
            cols_part,
            values_parts.join(",\n")
        )
    }
}

// ── 便捷函数 ──

/// 快速导出整个数据库到文件
pub fn export_database(engine: SharedEngine, path: &str) -> Result<()> {
    let exporter = Exporter::new(engine);
    exporter.export_to_file(path)
}

/// 快速导出整个数据库到字符串
pub fn export_database_to_string(engine: SharedEngine) -> Result<String> {
    let exporter = Exporter::new(engine);
    exporter.export_to_string()
}

/// 将 SQL 导出字符串拆分为独立语句（用于批量导入）
pub fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut string_char = '\'';
    let mut prev_char = '\0';

    for ch in sql.chars() {
        if in_string {
            current.push(ch);
            if ch == string_char && prev_char != '\\' {
                in_string = false;
            }
        } else {
            if ch == '\'' || ch == '"' {
                in_string = true;
                string_char = ch;
                current.push(ch);
            } else if ch == ';' {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    statements.push(trimmed.to_string());
                }
                current.clear();
            } else if ch == '\n' || ch == '\r' {
                let trimmed = current.trim();
                if trimmed.starts_with("--") {
                    current.clear();
                } else {
                    current.push(ch);
                }
            } else {
                current.push(ch);
            }
        }
        prev_char = ch;
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() && !trimmed.starts_with("--")
        && trimmed != "BEGIN" && trimmed != "COMMIT"
    {
        statements.push(trimmed.to_string());
    }

    statements
}

/// 导入 SQL 文件到数据库（逐语句执行）
pub fn import_sql_file(
    engine: SharedEngine,
    path: &Path,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<()> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    import_sql_string(engine, &content, on_progress)
}

/// 导入 SQL 字符串到数据库
pub fn import_sql_string(
    engine: SharedEngine,
    sql: &str,
    on_progress: Option<&dyn Fn(usize, usize)>,
) -> Result<()> {
    use crate::sql::executor::Executor;
    use crate::sql::parser::SqlParser;

    let executor = Executor::new(engine);
    let statements = split_sql_statements(sql);
    let total = statements.len();

    for (i, stmt_str) in statements.iter().enumerate() {
        let trimmed = stmt_str.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("--")
            || trimmed == "BEGIN"
            || trimmed == "COMMIT"
        {
            continue;
        }
        match SqlParser::parse(trimmed) {
            Ok(stmt) => {
                executor.execute(&stmt)?;
            }
            Err(e) => {
                tracing::warn!(
                    "Skipping statement {}: {} — SQL: {}",
                    i + 1,
                    e,
                    trimmed
                );
            }
        }
        if let Some(cb) = on_progress {
            cb(i + 1, total);
        }
    }

    Ok(())
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::sql::executor::Executor;
    use crate::sql::parser::SqlParser;
    use crate::storage::redb_engine::RedbEngine;
    use tempfile::TempDir;

    fn setup_db() -> (TempDir, SharedEngine) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_export.db");
        let engine = Arc::new(RedbEngine::open(&path).unwrap()) as SharedEngine;
        let executor = Executor::new(engine.clone());

        executor
            .execute(&SqlParser::parse(
                "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)",
            ).unwrap())
            .unwrap();
        executor
            .execute(&SqlParser::parse("INSERT INTO users VALUES (1, 'Alice', 30)").unwrap())
            .unwrap();
        executor
            .execute(&SqlParser::parse("INSERT INTO users VALUES (2, 'Bob', 25)").unwrap())
            .unwrap();

        (dir, engine)
    }

    #[test]
    fn test_export_contains_create_table() {
        let (_dir, engine) = setup_db();
        let exporter = Exporter::new(engine);
        let sql = exporter.export_to_string().unwrap();
        assert!(sql.contains("CREATE TABLE"));
        assert!(sql.contains("users"));
    }

    #[test]
    fn test_export_contains_insert_data() {
        let (_dir, engine) = setup_db();
        let exporter = Exporter::new(engine);
        let sql = exporter.export_to_string().unwrap();
        assert!(sql.contains("INSERT INTO"));
        assert!(sql.contains("Alice"));
        assert!(sql.contains("Bob"));
    }

    #[test]
    fn test_export_table_specific() {
        let (_dir, engine) = setup_db();
        let exporter = Exporter::new(engine);
        let sql = exporter.export_table_to_string("users").unwrap();
        assert!(sql.contains("users"));
        assert!(sql.contains("Alice"));
    }

    #[test]
    fn test_export_with_drop_table() {
        let (_dir, engine) = setup_db();
        let mut config = ExportConfig::default();
        config.include_drop_table = true;
        let exporter = Exporter::with_config(engine, config);
        let sql = exporter.export_to_string().unwrap();
        assert!(sql.contains("DROP TABLE IF EXISTS"));
    }

    #[test]
    fn test_export_with_transaction() {
        let (_dir, engine) = setup_db();
        let mut config = ExportConfig::default();
        config.wrap_in_transaction = true;
        let exporter = Exporter::with_config(engine, config);
        let sql = exporter.export_to_string().unwrap();
        assert!(sql.contains("BEGIN") || sql.contains("BEGIN;"));
        assert!(sql.contains("COMMIT"));
    }

    #[test]
    fn test_export_without_data() {
        let (_dir, engine) = setup_db();
        let mut config = ExportConfig::default();
        config.include_data = false;
        let exporter = Exporter::with_config(engine, config);
        let sql = exporter.export_to_string().unwrap();
        assert!(sql.contains("CREATE TABLE"));
        assert!(!sql.contains("INSERT INTO"));
    }

    #[test]
    fn test_export_empty_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.db");
        let engine = Arc::new(RedbEngine::open(&path).unwrap()) as SharedEngine;
        let sql = Exporter::new(engine).export_to_string().unwrap();
        assert!(sql.contains("no tables") || sql.contains("Export complete"));
    }

    #[test]
    fn test_split_statements() {
        let sql = "CREATE TABLE t (id INT);\nINSERT INTO t VALUES (1);";
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("CREATE"));
        assert!(stmts[1].contains("INSERT"));
    }

    #[test]
    fn test_dialect_mysql_types() {
        let d = SqlDialect::MySQL;
        assert_eq!(d.type_name(&ColumnType::Integer), "INT");
        assert_eq!(d.type_name(&ColumnType::Text), "VARCHAR(65535)");
        assert_eq!(d.type_name(&ColumnType::Boolean), "TINYINT(1)");
    }

    #[test]
    fn test_dialect_postgres_types() {
        let d = SqlDialect::PostgreSQL;
        assert_eq!(d.type_name(&ColumnType::Integer), "BIGINT");
        assert_eq!(d.type_name(&ColumnType::Boolean), "BOOLEAN");
        assert_eq!(d.type_name(&ColumnType::Blob), "BYTEA");
    }

    #[test]
    fn test_dialect_sqlite_types() {
        let d = SqlDialect::SQLite;
        assert_eq!(d.type_name(&ColumnType::Float), "REAL");
        assert_eq!(d.type_name(&ColumnType::Boolean), "INTEGER");
    }

    #[test]
    fn test_format_value_boolean() {
        let d = SqlDialect::Standard;
        assert_eq!(d.format_value(&Value::Boolean(true)), "1");
        assert_eq!(d.format_value(&Value::Boolean(false)), "0");
    }

    #[test]
    fn test_format_value_postgres_boolean() {
        let d = SqlDialect::PostgreSQL;
        assert_eq!(d.format_value(&Value::Boolean(true)), "TRUE");
        assert_eq!(d.format_value(&Value::Boolean(false)), "FALSE");
    }

    #[test]
    fn test_format_value_null() {
        let d = SqlDialect::Standard;
        assert_eq!(d.format_value(&Value::Null), "NULL");
    }

    #[test]
    fn test_format_value_integer() {
        let d = SqlDialect::Standard;
        assert_eq!(d.format_value(&Value::Integer(42)), "42");
    }

    #[test]
    fn test_export_to_file() {
        let (_dir, engine) = setup_db();
        let out_dir = TempDir::new().unwrap();
        let out_path = out_dir.path().join("export.sql");
        let exporter = Exporter::new(engine);
        exporter.export_to_file(&out_path).unwrap();
        assert!(out_path.exists());
        let content = std::fs::read_to_string(&out_path).unwrap();
        assert!(content.contains("CREATE TABLE"));
        assert!(content.contains("Alice"));
    }

    #[test]
    fn test_export_database_public_fn() {
        let (_dir, engine) = setup_db();
        let result = export_database_to_string(engine);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("users"));
    }
}
