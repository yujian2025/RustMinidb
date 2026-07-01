//! 全局配置系统
//!
//! 支持 TOML 配置文件加载和命令行参数覆盖。

use serde::Deserialize;
use std::path::Path;

/// 数据库服务器配置
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub logging: LogConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_connections: u32,
    pub query_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfig {
    pub db_path: String,
    pub cache_size_mb: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfig {
    pub level: String,
    pub format: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
                max_connections: 100,
                query_timeout_ms: 5000,
            },
            storage: StorageConfig {
                db_path: "data.db".into(),
                cache_size_mb: 64,
            },
            logging: LogConfig {
                level: "info".into(),
                format: "text".into(),
            },
        }
    }
}

impl Config {
    /// 从 TOML 配置文件加载
    pub fn load(path: &Path) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {}", e))?;
        let config: Config =
            toml::de::from_str(&content).map_err(|e| format!("解析配置文件失败: {}", e))?;
        Ok(config)
    }

    /// 加载配置，若文件不存在则返回默认值
    pub fn load_or_default(path: Option<&Path>) -> Self {
        match path {
            Some(p) if p.exists() => Self::load(p).unwrap_or_default(),
            _ => Self::default(),
        }
    }

    /// 更新 db_path
    pub fn with_db_path(mut self, path: String) -> Self {
        self.storage.db_path = path;
        self
    }

    /// 更新 host
    pub fn with_host(mut self, host: String) -> Self {
        self.server.host = host;
        self
    }

    /// 更新 port
    pub fn with_port(mut self, port: u16) -> Self {
        self.server.port = port;
        self
    }
}
