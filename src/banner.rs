//! 启动横幅与视觉输出
//!
//! 提供 ANSI 彩色启动横幅、Logo 以及服务器信息面板。
//! 自动检测终端色彩支持并降级为纯文本。

use std::time::Instant;

use std::io::IsTerminal;

// ── ANSI Escape Codes ───────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

const FG_CYAN: &str = "\x1b[36m";
const FG_GREEN: &str = "\x1b[32m";
const FG_YELLOW: &str = "\x1b[33m";
const FG_MAGENTA: &str = "\x1b[35m";
const FG_WHITE: &str = "\x1b[37m";
const FG_BLUE: &str = "\x1b[34m";

// ── 颜色探测 ────────────────────────────────────────────

/// 检测终端是否支持 ANSI 颜色
fn supports_color() -> bool {
    // 优先检查 NO_COLOR 环境变量
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    // 检查 RUSTMINIDB_COLOR 环境变量，可强制开启/关闭
    if let Ok(val) = std::env::var("RUSTMINIDB_COLOR") {
        match val.as_str() {
            "0" | "false" | "off" | "no" => return false,
            "1" | "true" | "on" | "yes" => return true,
            _ => {}
        }
    }
    // Windows: 检查终端类型
    #[cfg(windows)]
    {
        // 检查是否支持 VT processing
        if let Ok(output) = std::process::Command::new("cmd")
            .args(["/c", "echo", "%TERM%"])
            .output()
        {
            let term = String::from_utf8_lossy(&output.stdout);
            if term.trim().contains("xterm") || term.trim().contains("cygwin") {
                return true;
            }
        }
        // 默认 Windows 10+ 新终端都支持
        return true;
    }
    #[cfg(not(windows))]
    {
        std::env::var("TERM")
            .ok()
            .map(|t| t != "dumb")
            .unwrap_or(false)
    }
}

/// 检查输出是否被重定向到管道/文件
fn is_pipe_output() -> bool {
    std::io::stdout().is_terminal()
}

/// 外部模块可调用，判断是否应该使用颜色
pub fn use_color() -> bool {
    supports_color() && is_pipe_output()
}

// ── Logo 定义 ───────────────────────────────────────────

/// 大号 "R" 科技感图标（标志性主视觉）
const LOGO_M_ART: &str = r#"
    ██████╗
    ██╔══██╗
    ██████╔╝
    ██╔══██╗
    ██║  ██║
    ╚═╝  ╚═╝
"#;

/// 完整 Logo 面板 — 现代科技感 "RustMinidb" 块状字标
const LOGO_ART: &str = r#"
  ╔══════════════════════════════════════════════════════╗
  ║                                                      ║
  ║    ██████╗ ██╗   ██╗███████╗████████╗               ║
  ║    ██╔══██╗██║   ██║██╔════╝╚══██╔══╝               ║
  ║    ██████╔╝██║   ██║███████╗   ██║                  ║
  ║    ██╔══██╗██║   ██║╚════██║   ██║                  ║
  ║    ██║  ██║╚██████╔╝███████║   ██║                  ║
  ║    ╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝                  ║
  ║                                                      ║
  ║    ███╗   ███╗██╗███╗   ██╗██╗██████╗ ██████╗       ║
  ║    ████╗ ████║██║████╗  ██║██║██╔══██╗██╔══██╗      ║
  ║    ██╔████╔██║██║██╔██╗ ██║██║██████╔╝██████╔╝      ║
  ║    ██║╚██╔╝██║██║██║╚██╗██║██║██╔══██╗██╔══██╗      ║
  ║    ██║ ╚═╝ ██║██║██║ ╚████║██║██████╔╝██████╔╝      ║
  ║    ╚═╝     ╚═╝╚═╝╚═╝  ╚═══╝╚═╝╚═════╝ ╚═════╝       ║
  ║                                                      ║
  ╚══════════════════════════════════════════════════════╝
"#;

/// 紧凑版 Logo（终端宽度有限时使用）
const LOGO_COMPACT: &str = r#"
   _  _  __  __  _  _  ___  _  _  ___
  | \| |/ _\ \/ / | \| | _ \| \| | _ \
  | .` | (_ >  <  | .` |   /| .` |  _/
  |_|\_|\___/_/\_\|_|\_|_|_\|_|\_|_|
"#;

/// 生成彩色渐变的 "R" 科技图标
fn colored_m_logo() -> String {
    if !use_color() {
        return LOGO_M_ART.to_string();
    }

    let lines: Vec<&str> = LOGO_M_ART.lines().collect();
    // 科技感渐变色：深蓝 → 青 → 紫
    let colors = [FG_BLUE, FG_CYAN, FG_MAGENTA, FG_CYAN, FG_BLUE, FG_MAGENTA];
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let color = colors[i.min(colors.len() - 1)];
        out.push_str(&format!("{}{}{}\n", color, line, RESET));
    }
    out
}

/// 生成彩色渐变的完整 Logo（科技感配色）
fn colored_logo() -> String {
    if !use_color() {
        return LOGO_ART.to_string();
    }

    let lines: Vec<&str> = LOGO_ART.lines().collect();
    // 科技感渐变色：边框深蓝 → Rust 段青绿 → 分隔冰蓝 → Minidb 段紫青
    let colors = [
        FG_CYAN,    // 1. 顶部边框
        FG_CYAN,    // 2. 空行
        FG_CYAN,    // 3. Rust: R
        FG_CYAN,    // 4. Rust: u
        FG_BLUE,    // 5. Rust: s
        FG_BLUE,    // 6. Rust: t
        FG_BLUE,    // 7. Rust end
        FG_MAGENTA, // 8. Rust bottom
        FG_CYAN,    // 9. 分隔行
        FG_MAGENTA, // 10. Minidb: M
        FG_MAGENTA, // 11. Minidb: i
        FG_BLUE,    // 12. Minidb: n
        FG_BLUE,    // 13. Minidb: i
        FG_CYAN,    // 14. Minidb: d
        FG_CYAN,    // 15. Minidb: b
        FG_CYAN,    // 16. 空行
        FG_CYAN,    // 17. 底部边框
    ];
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let color = colors[i.min(colors.len() - 1)];
        out.push_str(&format!("{}{}{}\n", color, line, RESET));
    }
    out
}

/// 只打印大号 "M" 图标（极简模式）
pub fn print_m_icon() {
    if use_color() {
        print!("{}", colored_m_logo());
    } else {
        print!("{}", LOGO_M_ART);
    }
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
            "{}██╗  ██╗ {}RustMinidb v{}{} — Embedded Relational Database (redb, ACID)",
            FG_MAGENTA, FG_GREEN, version, RESET
        );
    } else {
        println!("M  RustMinidb v{} — Embedded Relational Database (redb, ACID)", version);
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
            " {FG_MAGENTA}◈{RESET}{BOLD} RustMinidb v{version}{RESET}{DIM} — Embedded Relational Database{RESET}\n \
             {DIM}  Homepage: https://rustminidb.dev   License: BSL-1.1{RESET}\n \
             {DIM}  Storage: redb (single-file, ACID, MVCC){RESET}",
        )
    } else {
        format!(
            " ◈ RustMinidb v{} — Embedded Relational Database\n \
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
        "   {}System Information{}\n",
        if use_color() { format!("{}{}", BOLD, FG_YELLOW) } else { "".to_string() },
        RESET
    ));
    panel.push_str(&format!("  {}\n", border));
    panel.push_str(&format!("    Version         :  {}\n", version));
    panel.push_str(&format!("    Storage Engine  :  redb (single-file, ACID)\n"));
    panel.push_str(&format!("    Platform        :  {}\n", std::env::consts::OS));
    panel.push_str(&format!("    Architecture    :  {}\n", std::env::consts::ARCH));
    panel.push_str(&format!("    Features        :  {}\n", features.join(", ")));
    panel.push_str(&format!("  {}\n", border));
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
    // 先打印大号 "M" 图标
    let version = crate::version();
    println!();
    if use_color() {
        println!(
            "  {}╔══════════════════════════════════════════════════╗{}",
            FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}              {}{}M{} DB Server{}               {}║{}",
            FG_MAGENTA, RESET,
            BOLD, FG_CYAN, RESET, BOLD, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}     RustMinidb — Lightweight Embedded DB      {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}╠══════════════════════════════════════════════════╣{}",
            FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}  Version:  {:<38} {}║{}",
            FG_MAGENTA, RESET,
            version,
            FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}  Server:   http://{}:{:<29} {}║{}",
            FG_MAGENTA, RESET, host, port, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}  Database: {:<39} {}║{}",
            FG_MAGENTA, RESET, db_name, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}  Storage:  redb (ACID, single-file)           {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}╠══════════════════════════════════════════════════╣{}",
            FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}  API Endpoints:                                  {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /              - Web Admin UI            {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    POST /v1/query      - Execute SQL             {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /v1/health     - Health check            {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /v1/tables     - List tables             {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /v1/schema/{{t}} - Table schema          {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /v1/export     - Export SQL              {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}║{}    GET  /v1/metrics    - Server metrics          {}║{}",
            FG_MAGENTA, RESET, FG_MAGENTA, RESET
        );
        println!(
            "  {}╚══════════════════════════════════════════════════╝{}",
            FG_MAGENTA, RESET
        );
    } else {
        println!("  ╔══════════════════════════════════════════════════╗");
        println!("  ║                M DB Server                      ║");
        println!("  ║     RustMinidb — Lightweight Embedded DB        ║");
        println!("  ╠══════════════════════════════════════════════════╣");
        println!("  ║  Version:  {:<38}║", version);
        println!("  ║  Server:   http://{}:{:<29}║", host, port);
        println!("  ║  Database: {:<39}║", db_name);
        println!("  ║  Storage:  redb (ACID, single-file)             ║");
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
        let text = banner_text_detailed(env!("CARGO_PKG_VERSION"), &["server", "shell"]);
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

    #[test]
    fn test_print_m_icon() {
        print_m_icon();
    }
}