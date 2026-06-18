//! rdm 桌面 GUI（Tauri v2）
//!
//! 启动时在同一进程里跑起：
//!   - TaskManager + 调度器（复用 rdm 库）
//!   - axum:127.0.0.1:7319（供 rdm CLI 和未来浏览器扩展连接）
//! 前端通过 Tauri 命令（invoke）操作任务，不走 HTTP，避免 webview 跨源问题。
// force generate_context! 重新读取前端文件（改动 frontend/ 不一定触发宏重展开）

use rdm::categories::Categories;
use rdm::manager::{StatusSummary, TaskManager};
use rdm::server;
use rdm::task::TaskView;
use std::path::PathBuf;
use tauri::{Manager, State};

/// 添加任务。file_path 若是相对路径，落到用户"下载"目录。
#[tauri::command]
async fn add(
    state: State<'_, TaskManager>,
    url: String,
    file_path: String,
    segments: Option<usize>,
) -> Result<u64, String> {
    let abs = resolve_abs(file_path);
    let segs = segments.unwrap_or(8);
    if segs == 0 {
        return Err("segments 不能为 0".into());
    }
    state
        .add(url, abs.to_string_lossy().to_string(), segs)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn list(state: State<'_, TaskManager>) -> Result<Vec<TaskView>, String> {
    Ok(state.list().await)
}

#[tauri::command]
async fn pause(state: State<'_, TaskManager>, id: u64) -> Result<(), String> {
    state.pause(id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn resume(state: State<'_, TaskManager>, id: u64) -> Result<(), String> {
    state.resume(id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn retry(state: State<'_, TaskManager>, id: u64) -> Result<(), String> {
    state.retry(id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn rm(state: State<'_, TaskManager>, id: u64, purge: Option<bool>) -> Result<(), String> {
    state.rm(id, purge.unwrap_or(false)).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn status(state: State<'_, TaskManager>) -> Result<StatusSummary, String> {
    Ok(state.status().await)
}

/// 扩展/GUI 触发"待确认"任务（弹窗确认前）
#[tauri::command]
async fn prompt(
    state: State<'_, TaskManager>,
    url: String,
    file_path: String,
) -> Result<u64, String> {
    state.add_awaiting(url, file_path).await.map_err(|e| format!("{e:#}"))
}

/// 弹窗确认后：落定路径 + 分段，转 Queued
#[tauri::command]
async fn confirm(
    state: State<'_, TaskManager>,
    id: u64,
    file_path: String,
    segments: usize,
) -> Result<(), String> {
    state.confirm(id, file_path, segments).await.map_err(|e| format!("{e:#}"))
}

/// 读取分类列表
#[tauri::command]
async fn get_categories() -> Result<Categories, String> {
    Categories::load().map_err(|e| format!("{e:#}"))
}

/// 保存分类列表
#[tauri::command]
async fn set_categories(cats: Categories) -> Result<(), String> {
    cats.save().map_err(|e| format!("{e:#}"))
}

#[derive(serde::Serialize)]
struct ConfigResp {
    default_dir: String,
    segments: usize,
}

/// 默认配置
#[tauri::command]
async fn get_config() -> Result<ConfigResp, String> {
    Ok(ConfigResp {
        default_dir: dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        segments: 8,
    })
}

/// 把文件名解析成绝对路径：绝对路径原样用，相对路径基于"下载"目录
fn resolve_abs(file_path: String) -> PathBuf {
    let p = PathBuf::from(&file_path);
    if p.is_absolute() {
        return p;
    }
    let base = dirs::download_dir().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    });
    base.join(p)
}

/// 打开原生目录选择器，返回所选目录的绝对路径（取消返回 null）
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog().file().blocking_pick_folder();
    Ok(path.map(|p| p.to_string()))
}

/// 把主窗口拉到前台并聚焦（收到待确认下载时调用，确保用户看到弹窗）
/// 用"临时置顶"trick 绕过 Windows 不允许后台进程抢前台的限制。
#[tauri::command]
fn focus_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_always_on_top(true);
        let _ = w.set_focus();
        let w2 = w.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let _ = w2.set_always_on_top(false);
        });
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 创建任务管理器（加载已有 tasks.json）
            let mgr = TaskManager::new(3).expect("创建任务管理器失败");

            // 在 Tauri 的 async runtime 里启动调度器 + axum 服务
            let mgr_for_sched = mgr.clone();
            let mgr_for_http = mgr.clone();
            tauri::async_runtime::spawn(async move {
                mgr_for_sched.spawn_scheduler();
                let router = server::router(mgr_for_http);
                match tokio::net::TcpListener::bind("127.0.0.1:7319").await {
                    Ok(listener) => {
                        println!("rdm 内嵌 daemon 监听 127.0.0.1:7319");
                        if let Err(e) = axum::serve(listener, router).await {
                            eprintln!("axum 服务退出: {e}");
                        }
                    }
                    Err(e) => eprintln!(
                        "绑定 127.0.0.1:7319 失败（可能 `rdm serve` 已在运行）: {e}"
                    ),
                }
            });

            // 把管理器注册为 Tauri state，供命令访问
            app.manage(mgr);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add, list, pause, resume, retry, rm, status, pick_folder, focus_window,
            prompt, confirm, get_categories, set_categories, get_config
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
