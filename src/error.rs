//! RustMinidb 错误类型体系
//!
//! 分层设计：顶层 `RustMinidbError` 聚合所有子模块错误，
//! 各子模块有自己的专用错误类型。

use thiserror::Error;

/// 顶层错误类型
#[derive(Error, Debug)]
pub enum RustMinidbError {
    #[error("存储引擎错误: {0}")]
    Engine(#[from] EngineError),

    #[error("SQL 解析错误: {0}")]
    Parse(#[from] ParseError),

    #[error("SQL 执行错误: {0}")]
    Exec(#[from] ExecError),

    #[error("序列化错误: {0}")]
    Serialization(String),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("配置错误: {0}")]
    Config(String),
}

impl From<bincode::Error> for RustMinidbError {
    fn from(e: bincode::Error) -> Self {
        RustMinidbError::Serialization(e.to_string())
    }
}

// redb 错误到顶层错误的直接转换（用于 ? 操作符）
impl From<redb::DatabaseError> for RustMinidbError {
    fn from(e: redb::DatabaseError) -> Self {
        RustMinidbError::Engine(EngineError::RedbDatabase(e.to_string()))
    }
}

impl From<redb::TransactionError> for RustMinidbError {
    fn from(e: redb::TransactionError) -> Self {
        RustMinidbError::Engine(EngineError::RedbTransaction(e.to_string()))
    }
}

impl From<redb::TableError> for RustMinidbError {
    fn from(e: redb::TableError) -> Self {
        RustMinidbError::Engine(EngineError::RedbTable(e.to_string()))
    }
}

impl From<redb::StorageError> for RustMinidbError {
    fn from(e: redb::StorageError) -> Self {
        RustMinidbError::Engine(EngineError::RedbStorage(e.to_string()))
    }
}

impl From<redb::CommitError> for RustMinidbError {
    fn from(e: redb::CommitError) -> Self {
        RustMinidbError::Engine(EngineError::RedbCommit(e.to_string()))
    }
}

/// 存储引擎错误
#[derive(Error, Debug)]
pub enum EngineError {
    #[error("表 '{0}' 已存在")]
    TableAlreadyExists(String),

    #[error("表 '{0}' 不存在")]
    TableNotFound(String),

    #[error("表 '{0}' 没有主键")]
    NoPrimaryKey(String),

    #[error("主键冲突: {0}")]
    PrimaryKeyConflict(String),

    #[error("redb 数据库错误: {0}")]
    RedbDatabase(String),

    #[error("redb 事务错误: {0}")]
    RedbTransaction(String),

    #[error("redb 表错误: {0}")]
    RedbTable(String),

    #[error("redb 存储错误: {0}")]
    RedbStorage(String),

    #[error("redb 提交错误: {0}")]
    RedbCommit(String),

    #[error("bincode 序列化错误: {0}")]
    Bincode(#[from] bincode::Error),

    #[error("表 '{0}' 行数超出限制")]
    RowCountExceeded(String),
}

impl From<redb::DatabaseError> for EngineError {
    fn from(e: redb::DatabaseError) -> Self {
        EngineError::RedbDatabase(e.to_string())
    }
}

impl From<redb::TransactionError> for EngineError {
    fn from(e: redb::TransactionError) -> Self {
        EngineError::RedbTransaction(e.to_string())
    }
}

impl From<redb::TableError> for EngineError {
    fn from(e: redb::TableError) -> Self {
        EngineError::RedbTable(e.to_string())
    }
}

impl From<redb::StorageError> for EngineError {
    fn from(e: redb::StorageError) -> Self {
        EngineError::RedbStorage(e.to_string())
    }
}

impl From<redb::CommitError> for EngineError {
    fn from(e: redb::CommitError) -> Self {
        EngineError::RedbCommit(e.to_string())
    }
}

/// SQL 解析错误
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("SQL 语法错误: {0}")]
    Syntax(String),

    #[error("不支持的 SQL 语句: {0}")]
    Unsupported(String),

    #[error("SQL 为空")]
    Empty,

    #[error("MVP 只支持单条语句")]
    MultipleStatements(String),
}

/// SQL 执行错误
#[derive(Error, Debug)]
pub enum ExecError {
    #[error("表 '{0}' 不存在")]
    TableNotFound(String),

    #[error("列 '{0}' 不存在")]
    ColumnNotFound(String),

    #[error("类型不匹配: {0}")]
    TypeMismatch(String),

    #[error("约束违反: {0}")]
    ConstraintViolation(String),

    #[error("验证错误: {0}")]
    Validation(String),

    #[error("未实现的 SQL 特性: {0}")]
    NotImplemented(String),
}

/// 通用 Result 类型
pub type Result<T> = std::result::Result<T, RustMinidbError>;
