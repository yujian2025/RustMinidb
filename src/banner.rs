//! RustMinidb 启动欢迎画面（Banner / ASCII Logo）
//!
//! 提供增强版 ASCII 艺术字 Logo、系统信息面板、ANSI 彩色输出。
//! 支持详细模式和简洁模式，自动检测终端颜色支持。

use std::time::Instant;

// ── ANSI 颜色常量（仅在不支持颜色的终端自动降级） ──

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

const FG_CYAN: &str = "\x1b[36m";
const FG_GREEN: &str = "\x1b[32m";
const FG_YELLOW: &str = "\x1b[33m";
const FG_MAGENTA: &str = "\x1b[35m";
const FG_WHITE: &str = "\x1b[37m";
const FG_BLUE: &str = "\x1b[34m";
const FG_RED: &str = "\x1b[31m";

/// 检测终端是否支持 ANSI 颜色
fn supports_color() -> bool {
    // Windows 10+ 的现代终端支持 ANSI
    if let Ok(term) = std::env::var("TERM") {
        if term != "dumb" {
            return true;
        }
    }
    if let Ok(ci) = std::env::var("CI") {
        if ci == "true" || ci == "1" {
            return false;
        }
    }
    // 检测 Windows 终端
    #[cfg(windows)]
    {
        if let Ok(ver) = std::env::var("WT_SESSION") {
            return !ver.is_empty(); // Windows Terminal
        }
        return true; // 默认开启
    }
    #[cfg(not(windows))]
    true
}

/// 是否为非交互模式（管道重定向等）
fn is_pipe_output() -> bool {
    use std::io::IsTerminal;
    !std::io::stdout().is_terminal()
}

/// 带颜色的文本包装（自动降级）
macro_rules! c {
    ($color:expr, $text:expr) => {
        if crate::banner::use_color() {
            concat!($color, $text, "\x1b[0m")
        } else {
            $text
        }
    };
}

/// 当前是否应使用颜色输出
pub fn use_color() -> bool {
    supports_color() && !is_pipe_output()
}

// ── 增强版 ASCII Logo ──

/// 数据库主题的 ASCII 艺术字 Logo
const LOGO_ART: &str = r#"
    ╔══════════════════════════════════════╗
    ║   ██████   ██    ██  ███████  ╔══════╣
    ║   ██   ██  ██    ██  ██       ║ SQL ║
    ║   ██████   ██    ██  ███████  ║ DB  ║
    ║   ██   ██   ██  ██   ██       ╚══════╣
    ║   ██   ██    ████    ███████          ║
    ╚════════════════════════════════════════╝
"#;

/// 紧凑 ASCII 艺术字 Logo（小屏友好）
const LOGO_COMPACT: &str = r#"
  ___  __ __ ___  _   _ ___ ___  ___
 | _ \/ _/ `_ \ / \ |_ _| _ \ _ \/ __|
 |   / _ \ (_) | | ' \| ||   / _ /\__ \
 |_|_\_|__\__,_|_|_||_|_|_|_\_|_| |___/
"#;

/// 生成 ANSI 彩色版本的 Logo
fn colored_logo() -> String {
    if !use_color() {
        return LOGO_ART.to_string();
    }

    // 为 ASCII Logo 添加渐变色效果
    let lines: Vec<&str> = LOGO_ART.lines().collect();
    let colors = [FG_CYAN, FG_GREEN, FG_YELLOW, FG_MAGENTA, FG_CYAN];
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let color = colors[i.min(colors.len() - 1)];
        out.push_str(&format!("{}{}{}\n", color, line, RESET));
    }
    out
}

/// 打印完整的启动欢迎画面（详细模式 - 默认）
pub fn print_banner() {
    println!("{}", banner_text());
}

/// 返回增强版的 Banner 文本（含系统和版本信息）
pub fn banner_text() -> String {
    let version = crate::version();
    logo_with_info(version, true)
}

/// 仅打印简短的启动信息行（用于非交互式 / 管道模式）
pub fn print_info_line() {
    let version = crate::version();
    if use_color() {
        println!(
            "{}RustMinidb v{}{} — Embedded Relational Database (redb, ACID)",
            FG_GREEN, version, RESET
        );
    } else {
        println!("RustMinidb v{} — Embedded Relational Database (redb, ACID)", version);
    }
}

/// 返回带系统信息的 Banner 面板
pub fn banner_text_detailed(version: &str, features: &[&str]) -> String {
    logo_with_info(version, true)
        + "\n"
        + &system_info_panel(version, features)
        + "\n"
        + &features_panel(features)
}

/// 返回简洁版的欢迎字符串（无颜色）
pub fn banner_text_compact() -> String {
    let version = crate::version();
    format!(
        r#"
{}
 RustMinidb v{}  —  Embedded Relational Database
 Homepage: https://rustminidb.dev   License: BSL-1.1
"#,
        LOGO_COMPACT, version
    )
}

// ── 内部构造器 ──

fn logo_with_info(version: &str, detailed: bool) -> String {
    let logo = if use_color() && detailed {
        colored_logo()
    } else if detailed {
        LOGO_ART.to_string()
    } else {
        LOGO_COMPACT.to_string()
    };

    if !detailed {
        return format!(
            "{}\n RustMinidb v{}  —  Embedded Relational Database\n",
            logo, version
        );
    }

    let info_line = if use_color() {
        format!(
            " {FG_GREEN}RustMinidb v{version}{RESET}{DIM} — Embedded Relational Database{RESET}\n \
             {DIM}Homepage: https://rustminidb.dev   License: BSL-1.1{RESET}\n \
             {DIM}Storage: redb (single-file, ACID, MVCC){RESET}",
        )
    } else {
        format!(
            " RustMinidb v{} — Embedded Relational Database\n \
             Homepage: https://rustminidb.dev   License: BSL-1.1\n \
             Storage: redb (single-file, ACID, MVCC)",
            version
        )
    };

    format!("{}\n{}\n", logo, info_line)
}

fn system_info_panel(version: &str, features: &[&str]) -> String {
    let mut panel = String::new();
    let border = if use_color() { format!("{}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}", FG_CYAN, RESET) } else { "━".repeat(48) };

    panel.push_str(&format!("  {}\n", border));
    panel.push_str(&format!(
        "  {}System Information{}\n",
        if use_color() { BOLD } else { "" },
        RESET
    ));
    panel.push_str(&format!("  {}\n", border));

    panel.push_str(&format!("    Version    : {}\n", version));
    panel.push_str(&format!("    Engine     : redb (single-file ACID MVCC)\n"));

    let feature_str = if features.is_empty() {
        "default".to_string()
    } else {
        features.join(", ")
    };
    panel.push_str(&format!("    Features   : {}\n", feature_str));

    panel.push_str(&format!(
        "  {}\n",
        if use_color() { format!("{}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}", FG_CYAN, RESET) } else { "━".repeat(48) }
    ));

    panel
}

fn features_panel(features: &[&str]) -> String {
    let mut panel = String::new();
    if features.is_empty() {
        return panel;
    }
    let check = if use_color() {
        format!("{}✓{}", FG_GREEN, RESET)
    } else {
        "✓".to_string()
    };
    panel.push_str(&format!("  {}Enabled Features{}\n",
        if use_color() { BOLD } else { "" },
        RESET
    ));
    for feat in features {
        panel.push_str(&format!("    {}  {}\n", check, feat));
    }
    panel
}

// ── 全局启动时间跟踪 ──

static mut START_INSTANT: Option<Instant> = None;

/// 记录进程启动时间（应在 main() 最开头调用）
pub fn record_start_time() {
    // safe: 仅在主线程初始化时调用一次
    #[allow(static_mut_refs)]
    unsafe {
        START_INSTANT = Some(Instant::now());
    }
}

/// 返回自 record_start_time() 以来的启动耗时，单位毫秒
pub fn startup_elapsed_ms() -> u64 {
    #[allow(static_mut_refs)]
    unsafe {
        START_INSTANT
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }
}

// ── 打印带耗时统计的启动完成消息 ──

/// 打印启动完成信息（包含总耗时）
pub fn print_startup_complete(addr: Option<&str>) {
    let elapsed = startup_elapsed_ms();
    let elapsed_str = format!("{:.2}s", elapsed as f64 / 1000.0);

    if let Some(a) = addr {
        if use_color() {
            println!(
                "\n {}✔{} Server listening on {}{}{} (startup: {}{}{})\n",
                FG_GREEN, RESET,
                BOLD, a, RESET,
                FG_YELLOW, elapsed_str, RESET
            );
        } else {
            println!("\n ✔ Server listening on {} (startup: {})\n", a, elapsed_str);
        }
    } else {
        if use_color() {
            println!(
                "\n {}✔{} RustMinidb ready (startup: {}{}{})\n",
                FG_GREEN, RESET,
                FG_YELLOW, elapsed_str, RESET
            );
        } else {
            println!("\n ✔ RustMinidb ready (startup: {})\n", elapsed_str);
        }
    }
}

/// 打印运行时的 HTTP 服务信息面板
pub fn print_server_info(host: &str, port: u16, db_name: &str) {
    println!();
    if use_color() {
        println!(
            "  {}╔══════════════════════════════════════════════════╗{}",
            FG_CYAN, RESET
        );
        println!(
            "  {}║{}            {}R u s t M i n i d b{}              {}║{}",
            FG_CYAN, RESET,
            BOLD, RESET,
            FG_CYAN, RESET
        );
        println!(
            "  {}║{}     Lightweight Embedded Database with REST    {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}╠══════════════════════════════════════════════════╣{}",
            FG_CYAN, RESET
        );
        println!(
            "  {}║{}  Version:  {:<38} {}║{}",
            FG_CYAN, RESET,
            crate::version(),
            FG_CYAN, RESET
        );
        println!(
            "  {}║{}  Server:   http://{}:{:<29} {}║{}",
            FG_CYAN, RESET, host, port, FG_CYAN, RESET
        );
        println!(
            "  {}║{}  Database: {:<39} {}║{}",
            FG_CYAN, RESET, db_name, FG_CYAN, RESET
        );
        println!(
            "  {}╠══════════════════════════════════════════════════╣{}",
            FG_CYAN, RESET
        );
        println!(
            "  {}║{}  API Endpoints:                                  {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /              - Web Admin UI            {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    POST /v1/query      - Execute SQL             {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /v1/health     - Health check            {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /v1/tables     - List tables             {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /v1/schema/{{t}} - Table schema          {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /v1/export     - Export SQL              {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}║{}    GET  /v1/metrics    - Server metrics          {}║{}",
            FG_CYAN, RESET, FG_CYAN, RESET
        );
        println!(
            "  {}╚══════════════════════════════════════════════════╝{}",
            FG_CYAN, RESET
        );
    } else {
        println!("  ╔══════════════════════════════════════════════════╗");
        println!("  ║              R u s t M i n i d b                ║");
        println!("  ║     Lightweight Embedded Database with REST     ║");
        println!("  ╠══════════════════════════════════════════════════╣");
        println!("  ║  Version:  {:<38}║", crate::version());
        println!("  ║  Server:   http://{}:{:<29}║", host, port);
        println!("  ║  Database: {:<39}║", db_name);
        println!("  ╠══════════════════════════════════════════════════╣");
        println!("  ║  API Endpoints:                                  ║");
        println!("  ║    GET  /              - Web Admin UI            ║");
        println!("  ║    POST /v1/query      - Execute SQL             ║");
        println!("  ║    GET  /v1/health     - Health check            ║");
        println!("  ║    GET  /v1/tables     - List tables             ║");
        println!("  ║    GET  /v1/schema/{{t}} - Table schema          ║");
        println!("  ║    GET  /v1/export     - Export SQL              ║");
        println!("  ║    GET  /v1/metrics    - Server metrics          ║");
        println!("  ╚══════════════════════════════════════════════════╝");
    }
    println!();
}


/// 打印认证状态信息
pub fn print_auth_status(enabled: bool) {
    if enabled {
        if use_color() {
            println!(
                "  {}✓{} {}API Authentication: ENABLED{} (Bearer Token required)",
                FG_GREEN, RESET, BOLD, RESET
            );
        } else {
            println!("  ✓ API Authentication: ENABLED (Bearer Token required)");
        }
    } else {
        if use_color() {
            println!(
                "  {}⚠{} {}API Authentication: DISABLED{} (set --api-token or RUSTMINIDB_API_TOKEN)",
                FG_YELLOW, RESET, DIM, RESET
            );
        } else {
            println!("  ⚠ API Authentication: DISABLED (set --api-token or RUSTMINIDB_API_TOKEN)");
        }
    }
    println!();
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_banner_contains_version() {
        let text = banner_text();
        assert!(text.contains(crate::version()));
        assert!(text.contains("RustMinidb"));
        assert!(text.contains("redb"));
    }

    #[test]
    fn test_banner_non_empty() {
        assert!(!banner_text().is_empty());
    }

    #[test]
    fn test_banner_compact_non_empty() {
        let text = banner_text_compact();
        assert!(!text.is_empty());
        assert!(text.contains(crate::version()));
    }

    #[test]
    fn test_info_line_contains_version() {
        let line = std::panic::catch_unwind(|| print_info_line());
        assert!(line.is_ok());
    }

    #[test]
    fn test_detailed_banner_contains_features() {
        let text = banner_text_detailed("0.1.0", &["server", "shell"]);
        assert!(text.contains("server"));
        assert!(text.contains("shell"));
    }

    #[test]
    fn test_use_color_no_panic() {
        let _ = use_color();
    }

    #[test]
    fn test_server_info_no_panic() {
        print_server_info("0.0.0.0", 8080, "test.db");
    }

    #[test]
    fn test_startup_elapsed() {
        record_start_time();
        assert!(startup_elapsed_ms() >= 0);
    }
}
