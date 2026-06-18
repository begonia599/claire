//! rdm — Rust Download Manager
//!
//! 用法（daemon 模式）：
//!   rdm serve [--port N] [--max-concurrent N]   启动后台服务
//!   rdm add  <URL> [文件名] [--segments N]       添加下载任务
//!   rdm list                                     查看任务列表
//!   rdm pause <id> | resume <id> | retry <id>    暂停/继续/重试
//!   rdm rm <id> [--purge]                        删除任务（--purge 连文件一起删）
//!   rdm status                                   总览
//!
//! 先 `rdm serve` 启动服务，再用其它命令操作。任务列表持久化在
//! %APPDATA%\rdm\tasks.json，daemon 重启后会自动恢复。

use rdm::{downloader, ipc, manager, server, store};
use anyhow::{anyhow, Context, Result};
use std::future::Future;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print_help();
        return Ok(());
    }

    let cmd = args[0].as_str();
    let rest = &args[1..];

    match cmd {
        "serve" => serve_cmd(rest).await,
        "add" => add_cmd(rest).await,
        "list" | "ls" => list_cmd(rest).await,
        "pause" => id_cmd(rest, ipc::pause).await,
        "resume" => id_cmd(rest, ipc::resume).await,
        "retry" => id_cmd(rest, ipc::retry).await,
        "rm" => rm_cmd(rest).await,
        "status" => status_cmd(rest).await,
        other => Err(anyhow!("未知命令 '{other}'，用 rdm --help 查看用法")),
    }
}

/// rdm serve：启动 daemon
async fn serve_cmd(args: &[String]) -> Result<()> {
    let port = parse_port(args)?;
    let max_concurrent = parse_usize_flag(args, "--max-concurrent").unwrap_or(3);
    if max_concurrent == 0 {
        return Err(anyhow!("--max-concurrent 不能为 0"));
    }

    let mgr = manager::TaskManager::new(max_concurrent)?;
    mgr.clone().spawn_scheduler();

    let app = server::router(mgr);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| {
            format!("绑定 127.0.0.1:{port} 失败，端口是否被占用？daemon 是否已在运行？")
        })?;
    println!(
        "🚀 rdm daemon 已启动：127.0.0.1:{port}（最多 {max_concurrent} 个并发下载）"
    );
    println!("   数据目录：{:?}", store::Registry::data_dir()?);
    println!("   按 Ctrl+C 退出");
    axum::serve(listener, app).await.context("axum 服务异常退出")?;
    Ok(())
}

/// rdm add <URL> [文件名] [--segments N] [--port P]
async fn add_cmd(args: &[String]) -> Result<()> {
    let port = parse_port(args)?;
    let segments = parse_usize_flag(args, "--segments").unwrap_or(8);
    if segments == 0 {
        return Err(anyhow!("--segments 不能为 0"));
    }
    // 取出位置参数（非 -- 开头的）
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let url = positional
        .first()
        .ok_or_else(|| anyhow!("缺少 URL。用法：rdm add <URL> [文件名]"))?
        .to_string();
    let name = positional.get(1).map(|s| s.as_str());

    let file_path = resolve_output(name, &url)?;
    println!("🌐 添加任务：{url}");
    println!("📁 保存到：{}", file_path.display());

    let id = ipc::add(port, url, file_path.to_string_lossy().to_string(), segments).await?;
    println!("✅ 已入队，任务编号 #{id}");
    Ok(())
}

/// rdm list
async fn list_cmd(args: &[String]) -> Result<()> {
    let port = parse_port(args)?;
    let tasks = ipc::list(port).await?;
    if tasks.is_empty() {
        println!("（暂无任务）");
        return Ok(());
    }
    println!(
        "{:<5} {:<8} {:<14} {:<14} {}",
        "ID", "状态", "进度", "大小", "文件"
    );
    println!("{}", "-".repeat(70));
    for t in &tasks {
        let pct = if t.total > 0 {
            t.downloaded * 100 / t.total
        } else {
            0
        };
        let size = if t.total > 0 {
            format!(
                "{} / {}",
                downloader::format_size(t.downloaded),
                downloader::format_size(t.total)
            )
        } else {
            "?".to_string()
        };
        println!(
            "#{:<4} {:<8} {:<14} {:<14} {}",
            t.id,
            t.state,
            format!("{} {}%", render_bar(pct), pct),
            size,
            short_file(&t.file),
        );
        if let Some(e) = &t.error {
            println!("       └ 错误：{e}");
        }
    }
    Ok(())
}

/// rdm pause/resume/retry <id>
async fn id_cmd<F, Fut>(args: &[String], op: F) -> Result<()>
where
    F: FnOnce(u16, u64) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let port = parse_port(args)?;
    let id = parse_id(args)?;
    op(port, id).await?;
    println!("✅ 已发送");
    Ok(())
}

/// rdm rm <id> [--purge]
async fn rm_cmd(args: &[String]) -> Result<()> {
    let port = parse_port(args)?;
    let id = parse_id(args)?;
    let purge = args.iter().any(|a| a == "--purge");
    ipc::rm(port, id, purge).await?;
    println!(
        "✅ 已删除任务 #{id}{}",
        if purge { "（含文件）" } else { "" }
    );
    Ok(())
}

/// rdm status
async fn status_cmd(args: &[String]) -> Result<()> {
    let port = parse_port(args)?;
    let s = ipc::status(port).await?;
    println!(
        "运行中 {} | 排队 {} | 暂停 {} | 完成 {} | 失败 {}",
        s.running, s.queued, s.paused, s.completed, s.failed
    );
    Ok(())
}

// ---------- 参数解析小工具 ----------

/// 解析 --port，默认 7319
fn parse_port(args: &[String]) -> Result<u16> {
    Ok(parse_usize_flag(args, "--port").map(|v| v as u16).unwrap_or(ipc::DEFAULT_PORT))
}

/// 解析 --xxx <数值> 类参数
fn parse_usize_flag(args: &[String], flag: &str) -> Option<usize> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            if let Some(v) = it.next() {
                if let Ok(n) = v.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// 解析任务 id（支持 #1 或 1）
fn parse_id(args: &[String]) -> Result<u64> {
    let pos = args.iter().find(|a| !a.starts_with("--"));
    let raw = pos.ok_or_else(|| anyhow!("缺少任务 id"))?;
    let raw = raw.trim_start_matches('#');
    raw.parse::<u64>()
        .map_err(|_| anyhow!("无效的任务 id：{raw}"))
}

/// 把输出文件名解析成绝对路径
fn resolve_output(name: Option<&str>, url: &str) -> Result<PathBuf> {
    let p = match name {
        Some(n) => PathBuf::from(n),
        None => {
            // 从 URL 末段推断
            let inferred = url
                .split('?')
                .next()
                .and_then(|s| s.rsplit('/').next())
                .filter(|s| !s.is_empty())
                .unwrap_or("download.bin");
            PathBuf::from(inferred)
        }
    };
    // 若是相对路径，基于当前工作目录转成绝对路径
    if p.is_absolute() {
        Ok(p)
    } else {
        let cwd = std::env::current_dir().context("获取当前目录失败")?;
        Ok(cwd.join(p))
    }
}

/// 简单的文本进度条，10 格
fn render_bar(pct: u64) -> String {
    let filled = (pct / 10).min(10) as usize;
    let mut s = String::with_capacity(12);
    s.push('[');
    for i in 0..10 {
        s.push(if i < filled { '#' } else { '-' });
    }
    s.push(']');
    s
}

/// 文件路径只显示文件名，避免列太宽
fn short_file(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string())
}

fn print_help() {
    println!("rdm — Rust Download Manager (阶段2: 队列管理 + daemon)");
    println!();
    println!("先启动服务：  rdm serve [--port 7319] [--max-concurrent 3]");
    println!("再操作任务：");
    println!("  rdm add  <URL> [文件名] [--segments 8]   添加下载");
    println!("  rdm list                                   查看列表");
    println!("  rdm pause <id> | resume <id> | retry <id>  暂停/继续/重试");
    println!("  rdm rm <id> [--purge]                       删除任务");
    println!("  rdm status                                  总览");
    println!();
    println!("任务 id 可写 #1 或 1。中断后重启 rdm serve，任务会自动恢复并续传。");
}
