//! CLI 命令定义
//!
//! 使用 clap 定义子命令：
//! - serve: 启动 HTTP 服务器（支持多数据库）
//! - shell: 交互式 SQL 控制台
//! - exec: 执行单条 SQL
//! - init: 初始化数据库
//! - version: 显示版本信息

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rustminidb")]
#[command(about = "A lightweight embedded database with native REST API")]
#[command(version = "0.2.0")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动数据库 HTTP 服务器（支持多数据库同时加载）
    Serve {
        /// 监听地址
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// 监听端口
        #[arg(long, default_value_t = 8080)]
        port: u16,

        /// 数据库文件路径（可指定多个，实现多数据库同时打开）
        #[arg(long, default_value = "data.db")]
        db: Vec<String>,

        /// 数据库文件所在目录
        #[arg(long, default_value = ".")]
        db_dir: String,

        /// 最大连接数
        #[arg(long, default_value_t = 100)]
        #[allow(dead_code)]
        max_connections: u32,

        /// 启用文件监听自动重载（当数据库文件变化时自动刷新）
        #[arg(long)]
        watch: bool,

        /// API 访问令牌（Bearer Token），为空则不启用认证。
        /// 也可通过环境变量 RUSTMINIDB_API_TOKEN 设置。
        #[arg(long)]
        api_token: Option<String>,
    },

    /// 交互式 SQL Shell（类似 sqlite3）
    Shell {
        /// 数据库文件路径（可指定多个，用 .use 命令切换）
        #[arg(long, default_value = "data.db")]
        db: Vec<String>,

        /// 启用文件监听自动重载
        #[arg(long)]
        watch: bool,
    },

    /// 执行单条 SQL 语句
    Exec {
        /// 数据库文件路径
        #[arg(long, default_value = "data.db")]
        db: String,

        /// SQL 语句
        sql: String,

        /// 输出格式 (json | table)
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// 初始化数据库（创建空数据库文件）
    Init {
        /// 数据库文件路径
        #[arg(long, default_value = "data.db")]
        db: String,
    },

    /// 导出数据库为标准 SQL 语句
    Export {
        /// 数据库文件路径
        #[arg(long, default_value = "data.db")]
        db: String,

        /// 输出文件路径（默认输出到 stdout）
        #[arg(long)]
        output: Option<String>,

        /// 指定要导出的表名（不指定则导出全部）
        #[arg(long)]
        table: Option<String>,

        /// 是否包含数据（INSERT 语句）
        #[arg(long, default_value_t = true)]
        data: bool,

        /// 是否包含建表语句（CREATE TABLE）
        #[arg(long, default_value_t = true)]
        create: bool,
    },

    /// 显示版本信息
    Version,
}
