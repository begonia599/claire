//! 本地 HTTP 服务（axum）：daemon 对外的 JSON API
//!
//! 这是阶段 2 的 daemon 接口；阶段 4 的浏览器扩展会直接复用这些路由（尤其 /add）。
//! 所有路由只接受/返回 JSON，监听 127.0.0.1，不对外暴露。

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::categories::Categories;
use crate::manager::{StatusSummary, TaskManager};
use crate::store;
use crate::task::TaskView;

/// 请求体：添加任务（file_path 由客户端解析成绝对路径后传入）
#[derive(Deserialize, Serialize)]
pub struct AddReq {
    pub url: String,
    pub file_path: String,
    pub segments: Option<usize>,
}

/// 请求体：待确认任务（扩展拦截后发这个，等 GUI 弹窗确认）
#[derive(Deserialize, Serialize)]
pub struct PromptReq {
    pub url: String,
    /// 建议文件名（可空，GUI 弹窗里可改）
    pub file_path: String,
}

/// 请求体：确认待确认任务（落定路径 + 分段，转 Queued）
#[derive(Deserialize, Serialize)]
pub struct ConfirmReq {
    pub id: u64,
    pub file_path: String,
    pub segments: usize,
}

#[derive(Serialize, Deserialize)]
pub struct AddResp {
    pub ok: bool,
    pub id: u64,
    pub error: Option<String>,
}

/// 请求体：按 id 操作
#[derive(Deserialize, Serialize)]
pub struct IdReq {
    pub id: u64,
}

#[derive(Deserialize, Serialize)]
pub struct RmReq {
    pub id: u64,
    pub purge: Option<bool>,
}

/// 通用回复（暂停/继续/重试/删除）
#[derive(Serialize, Deserialize)]
pub struct GenericResp {
    pub ok: bool,
    pub error: Option<String>,
}

/// /config 返回：默认下载目录 + 默认分段
#[derive(Serialize)]
pub struct ConfigResp {
    pub default_dir: String,
    pub segments: usize,
}

/// 构建路由
pub fn router(m: TaskManager) -> Router {
    Router::new()
        .route("/add", post(add))
        .route("/prompt", post(prompt))
        .route("/confirm", post(confirm))
        .route("/list", get(list))
        .route("/pause", post(pause))
        .route("/resume", post(resume))
        .route("/retry", post(retry))
        .route("/rm", post(rm))
        .route("/status", get(status))
        .route("/categories", get(get_categories).post(set_categories))
        .route("/config", get(get_config))
        .with_state(m)
}

async fn add(
    State(m): State<TaskManager>,
    Json(req): Json<AddReq>,
) -> Json<AddResp> {
    let segments = req.segments.unwrap_or(8);
    // 文件名空就从 URL 推断；相对路径统一解析到"下载"目录
    let name = if req.file_path.trim().is_empty() {
        store::infer_filename(&req.url)
    } else {
        req.file_path
    };
    let abs = store::resolve_download_path(&name);
    match m.add(req.url, abs.to_string_lossy().to_string(), segments).await {
        Ok(id) => Json(AddResp {
            ok: true,
            id,
            error: None,
        }),
        Err(e) => Json(AddResp {
            ok: false,
            id: 0,
            error: Some(format!("{e:#}")),
        }),
    }
}

async fn list(State(m): State<TaskManager>) -> Json<Vec<TaskView>> {
    Json(m.list().await)
}

async fn pause(
    State(m): State<TaskManager>,
    Json(req): Json<IdReq>,
) -> Json<GenericResp> {
    resp(m.pause(req.id).await)
}

async fn resume(
    State(m): State<TaskManager>,
    Json(req): Json<IdReq>,
) -> Json<GenericResp> {
    resp(m.resume(req.id).await)
}

async fn retry(
    State(m): State<TaskManager>,
    Json(req): Json<IdReq>,
) -> Json<GenericResp> {
    resp(m.retry(req.id).await)
}

async fn rm(
    State(m): State<TaskManager>,
    Json(req): Json<RmReq>,
) -> Json<GenericResp> {
    resp(m.rm(req.id, req.purge.unwrap_or(false)).await)
}

async fn status(State(m): State<TaskManager>) -> Json<StatusSummary> {
    Json(m.status().await)
}

/// 扩展拦截后发这个：建一个待确认任务，等 GUI 弹窗确认
async fn prompt(
    State(m): State<TaskManager>,
    Json(req): Json<PromptReq>,
) -> Json<AddResp> {
    let name = if req.file_path.trim().is_empty() {
        store::infer_filename(&req.url)
    } else {
        req.file_path
    };
    match m.add_awaiting(req.url, name).await {
        Ok(id) => Json(AddResp { ok: true, id, error: None }),
        Err(e) => Json(AddResp { ok: false, id: 0, error: Some(format!("{e:#}")) }),
    }
}

/// GUI 弹窗确认后发这个：落定路径 + 分段，转 Queued 开下
async fn confirm(
    State(m): State<TaskManager>,
    Json(req): Json<ConfirmReq>,
) -> Json<GenericResp> {
    resp(m.confirm(req.id, req.file_path, req.segments).await)
}

/// 读取分类列表
async fn get_categories() -> Json<Categories> {
    Json(Categories::load().unwrap_or_else(|_| Categories::defaults()))
}

/// 保存分类列表
async fn set_categories(Json(cats): Json<Categories>) -> Json<GenericResp> {
    resp(cats.save().map(|_| ()))
}

/// 默认配置（下载目录、默认分段）
async fn get_config() -> Json<ConfigResp> {
    let default_dir = dirs::download_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Json(ConfigResp { default_dir, segments: 8 })
}

/// 把 anyhow::Result 转成通用 JSON 回复
fn resp(r: anyhow::Result<()>) -> Json<GenericResp> {
    match r {
        Ok(()) => Json(GenericResp {
            ok: true,
            error: None,
        }),
        Err(e) => Json(GenericResp {
            ok: false,
            error: Some(format!("{e:#}")),
        }),
    }
}
