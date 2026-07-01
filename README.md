# RustMinidb

**轻量级嵌入式关系型数据库 · 原生 REST API · 单文件存储**

RustMinidb 是一个使用 Rust 编写的轻量级嵌入式关系型数据库，基于 [redb](https://github.com/cberner/redb) 存储引擎（ACID、MVCC、单文件）。原生内置 HTTP REST API 服务器，适合物联网、边缘计算和嵌入式场景。

---

## 特性

### ⚡ 核心能力

| 特性 | 说明 |
|---|---|
| **嵌入式** | 无需独立服务进程，可作为 Rust 库直接嵌入你的程序 |
| **SQL 支持** | 标准 SQL 语法（CREATE / INSERT / SELECT / UPDATE / DELETE / DROP） |
| **ACID 事务** | 基于 redb 引擎，支持原子性、一致性、隔离性、持久性 |
| **MVCC** | 多版本并发控制，读写互不阻塞 |
| **单文件存储** | 整个数据库存储在一个 `.db` 文件中，零配置 |
| **多数据类型** | INTEGER、FLOAT、TEXT、BLOB、BOOLEAN、TIMESTAMP |

### 🌐 REST API 服务器

| 端点 | 方法 | 认证 | 说明 |
|---|---|---|---|---|
| `/` | GET | ❌ 公开 | Web 管理后台页面 |
| `/v1/health` | GET | ❌ 公开 | 健康检查 |
| `/v1/query` | POST | ✅ Bearer Token | 执行 SQL 语句 |
| `/v1/tables` | GET | ✅ Bearer Token | 列出所有表 |
| `/v1/schema/{table}` | GET | ✅ Bearer Token | 查看表结构 |
| `/v1/export` | GET | ✅ Bearer Token | 导出数据库为 SQL |
| `/v1/metrics` | GET | ✅ Bearer Token | 运行时监控指标 |
| `/v1/databases` | GET | ✅ Bearer Token | 多数据库管理 |
| `/v1/import` | POST | ✅ Bearer Token | 数据导入 |

### 🛠️ 实用工具

- **交互式 Shell** — 类似 `sqlite3` 的命令行控制台
- **SQL 导出迁移** — 支持 Standard / MySQL / PostgreSQL / SQLite 四种方言
- **监控仪表盘** — 运行时 QPS、延迟、连接数等指标
- **彩色 Banner** — 增强型启动欢迎画面（ANSI 彩色 Logo）
- **请求追踪** — UUID 级请求链路追踪
- **🔒 API 认证** — Bearer Token 保护全部数据接口

---

## 使用场景

| 场景 | 说明 |
|---|---|
| **🔌 物联网 / 边缘计算** | 嵌入式设备上的本地数据存储，通过 REST API 远程查询 |
| **📱 移动 / 桌面应用** | 作为应用内数据库，替代 SQLite 的 Rust 原生方案 |
| **🧪 测试 / 原型开发** | 快速搭建数据层原型，无需安装独立数据库 |
| **🔐 嵌入式安全场景** | 单文件加密存储，无网络端口暴露（library 模式） |
| **📦 CI/CD 管道** | 轻量级测试数据库，毫秒级启动 |

---

## 快速开始

### 安装

**方式一：下载预编译二进制**

从 [Releases](https://github.com/rustminidb/rustminidb/releases) 下载对应平台的二进制文件。

**方式二：从源码编译**

```bash
# 确保已安装 Rust 工具链（https://rustup.rs）
cargo install rustminidb
```

**方式三：作为依赖库（Cargo）**

```toml
[dependencies]
rustminidb = "0.1"
```

### 基本使用

#### 🔹 命令行模式

```bash
# 初始化数据库
rustminidb init --db mydata.db

# 执行单条 SQL
rustminidb exec --db mydata.db "CREATE TABLE sensors (id INT PRIMARY KEY, value FLOAT)"
rustminidb exec --db mydata.db "INSERT INTO sensors VALUES (1, 25.6)"
rustminidb exec --db mydata.db "SELECT * FROM sensors"

# 启动交互式 Shell
rustminidb shell --db mydata.db

# 启动 HTTP 服务器（无认证，仅限开发环境）
rustminidb serve --host 0.0.0.0 --port 8080 --db mydata.db

# 启动 HTTP 服务器（推荐：开启 Bearer Token 认证）
rustminidb serve --host 0.0.0.0 --port 8080 --db mydata.db --api-token "your-secret-token"

# 也可通过环境变量设置 Token（避免命令行泄露）
set RUSTMINIDB_API_TOKEN=your-secret-token
rustminidb serve --host 0.0.0.0 --port 8080 --db mydata.db

# 导出数据库为 SQL
rustminidb export --db mydata.db --output backup.sql

# 查看版本
rustminidb version
```

#### 🔹 交互式 Shell 命令

在 `shell` 模式下可使用以下内置命令：

| 命令 | 说明 |
|---|---|
| `.tables` | 列出所有表 |
| `.schema` | 查看所有表结构 |
| `.monitor` | 显示运行时指标 |
| `.export` | 导出整个数据库为 SQL |
| `.exit` / `.quit` | 退出 Shell |
| `.help` | 显示帮助 |

#### 🔹 REST API 使用

```bash
# 启动服务器（推荐开启认证）
rustminidb serve --host 0.0.0.0 --port 8080 --db mydata.db --api-token "my-secret-token"

# 执行 SQL（需在请求头中携带 Token）
curl -X POST http://localhost:8080/v1/query \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer my-secret-token" \
  -d '{"sql": "SELECT * FROM sensors"}'

# 健康检查（公开端点，无需 Token）
curl http://localhost:8080/v1/health

# 查看监控指标（需 Token）
curl -H "Authorization: Bearer my-secret-token" http://localhost:8080/v1/metrics

# 导出数据库（需 Token）
curl -H "Authorization: Bearer my-secret-token" http://localhost:8080/v1/export
```

#### 🔹 嵌入式 Rust 库

```rust
use rustminidb::Database;

// 打开数据库
let db = Database::open("mydata.db").unwrap();

// 建表
db.execute("CREATE TABLE sensors (id INT PRIMARY KEY, value FLOAT)").unwrap();

// 插入数据
db.execute("INSERT INTO sensors VALUES (1, 25.6)").unwrap();

// 查询数据
let rows = db.query("SELECT * FROM sensors").unwrap();
for row in rows {
    println!("{:?}", row);
}
```

---

## 🔒 安全

### API 访问认证

RustMinidb 内置 **Bearer Token 认证** 保护所有数据接口，防止未授权访问。

```bash
# 方式一：命令行参数（推荐）
rustminidb serve --api-token "your-secret-token"

# 方式二：环境变量（避免 Token 出现在进程列表）
export RUSTMINIDB_API_TOKEN="your-secret-token"
rustminidb serve
```

**认证规则：**

| 规则 | 说明 |
|---|---|
| 未设置 Token | 所有接口完全公开（仅限开发环境） |
| 设置 Token | 除 `/` 和 `/v1/health` 外全部接口需 `Authorization: Bearer <token>` |
| Token 为空 | 等同于未设置，不启用认证 |
| 认证失败 | 返回 HTTP `401 Unauthorized` |

**公开端点（免认证）：**

| 端点 | 用途 |
|---|---|
| `GET /` | Web 管理后台页面 |
| `GET /v1/health` | 健康检查（用于负载均衡器监控） |

**安全建议：**

- 生产环境**必须**设置 `--api-token`
- 通过环境变量 `RUSTMINIDB_API_TOKEN` 传入 Token，避免命令行历史泄露
- 使用反向代理（nginx / caddy）终止 TLS/SSL，确保传输加密
- 定期更换 Token

---

## 配置

RustMinidb 支持 TOML 配置文件，默认值如下：

```toml
[server]
host = "0.0.0.0"
port = 8080
maxConnections = 100
queryTimeoutMs = 5000

[storage]
dbPath = "data.db"
cacheSizeMb = 64

[logging]
level = "info"
format = "text"
```

可通过命令行参数覆盖：

```bash
rustminidb serve --host 127.0.0.1 --port 9090 --db mydb.db
```

---

## SQL 支持

RustMinidb 支持以下 SQL 语法（MVP 阶段）：

| 语句 | 示例 |
|---|---|
| CREATE TABLE | `CREATE TABLE users (id INT PRIMARY KEY, name TEXT, age INT)` |
| INSERT | `INSERT INTO users VALUES (1, 'Alice', 30)` |
| SELECT | `SELECT * FROM users WHERE age > 20` |
| UPDATE | `UPDATE users SET age = 31 WHERE id = 1` |
| DELETE | `DELETE FROM users WHERE id = 2` |
| DROP TABLE | `DROP TABLE IF EXISTS users` |

支持的数据类型：`INTEGER`、`FLOAT`、`TEXT`、`BLOB`、`BOOLEAN`、`TIMESTAMP`

支持的比较运算符：`=`、`!=`、`<`、`>`、`<=`、`>=`、`AND`、`OR`

---

## 数据迁移与导出

支持四种 SQL 方言导出：

```bash
# 标准格式导出
rustminidb export --db mydata.db --output export.sql

# 导出兼容 MySQL
# （可通过 Rust API 配置 dialect: SqlDialect::MySQL）

# 导出兼容 PostgreSQL
# （可通过 Rust API 配置 dialect: SqlDialect::PostgreSQL）
```

导出特性：
- ✅ CREATE TABLE IF NOT EXISTS
- ✅ 批量 INSERT（可配置每批行数）
- ✅ DROP TABLE IF EXISTS 前缀
- ✅ 事务包裹（BEGIN / COMMIT）
- ✅ 表注释导出
- ✅ 多方言类型映射
- ✅ 进度回调

---

## 构建

```bash
# 克隆仓库
git clone https://github.com/rustminidb/rustminidb.git
cd rustminidb

# 构建（默认启用 server 特性）
cargo build --release

# 仅构建 library（无 HTTP 服务器）
cargo build --release --no-default-features

# 运行测试
cargo test --lib

# 查看帮助
./target/release/rustminidb --help
```

---

## 技术栈

| 组件 | 技术 |
|---|---|
| 编程语言 | Rust (edition 2021) |
| 存储引擎 | [redb](https://github.com/cberner/redb) (ACID, MVCC, 单文件) |
| SQL 解析 | [sqlparser-rs](https://github.com/sqlparser-rs/sqlparser-rs) |
| REST 框架 | [axum](https://github.com/tokio-rs/axum) |
| 运行时 | [tokio](https://github.com/tokio-rs/tokio) |
| 序列化 | serde / bincode / serde_json |
| 日志 | tracing / tracing-subscriber |

---

## 许可证

本项目基于 **BSL-1.1 许可证**（Boost Software License 1.0）发布。

```
Boost Software License - Version 1.0

Permission is hereby granted, free of charge, to any person or organization
obtaining a copy of the software and accompanying documentation...
```

---

## 相关链接

- **仓库**: https://github.com/yujian2025/rustminidb
- **文档**: https://docs.rs/rustminidb
- **主页**: https://rustminidb.dev
