//! CLI 客户端：通过 HTTP 和 daemon 通信
//!
//! 每条客户端命令（add/list/pause/...）就是一次对 127.0.0.1:port 的 HTTP 请求。
//! 连不上时提示"请先运行 rdm serve"。

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::manager::StatusSummary;
use crate::server::{AddReq, AddResp, GenericResp, IdReq, RmReq};
use crate::task::TaskView;

pub const DEFAULT_PORT: u16 = 7319;

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("创建 HTTP 客户端失败")
}

/// 把 reqwest 的连接错误翻译成"daemon 没启动"的友好提示
fn hint(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() || err.is_request() {
        anyhow!("无法连接 daemon（127.0.0.1:{}）。请先运行 `rdm serve`。", DEFAULT_PORT)
    } else {
        anyhow!("{err:#}")
    }
}

pub async fn add(port: u16, url: String, file_path: String, segments: usize) -> Result<u64> {
    let body = AddReq {
        url,
        file_path,
        segments: Some(segments),
    };
    let resp: AddResp = client()
        .post(format!("{}/add", base_url(port)))
        .json(&body)
        .send()
        .await
        .map_err(hint)?
        .json()
        .await
        .map_err(hint)?;
    if resp.ok {
        Ok(resp.id)
    } else {
        Err(anyhow!(resp.error.unwrap_or_else(|| "未知错误".into())))
    }
}

pub async fn list(port: u16) -> Result<Vec<TaskView>> {
    client()
        .get(format!("{}/list", base_url(port)))
        .send()
        .await
        .map_err(hint)?
        .json::<Vec<TaskView>>()
        .await
        .map_err(hint)
}

pub async fn pause(port: u16, id: u64) -> Result<()> {
    simple(port, "/pause", &IdReq { id }).await
}

pub async fn resume(port: u16, id: u64) -> Result<()> {
    simple(port, "/resume", &IdReq { id }).await
}

pub async fn retry(port: u16, id: u64) -> Result<()> {
    simple(port, "/retry", &IdReq { id }).await
}

pub async fn rm(port: u16, id: u64, purge: bool) -> Result<()> {
    simple(port, "/rm", &RmReq { id, purge: Some(purge) }).await
}

pub async fn status(port: u16) -> Result<StatusSummary> {
    client()
        .get(format!("{}/status", base_url(port)))
        .send()
        .await
        .map_err(hint)?
        .json::<StatusSummary>()
        .await
        .map_err(hint)
}

/// 通用 POST：发个带 id 的 JSON，期望 GenericResp
async fn simple<B: Serialize>(port: u16, path: &str, body: &B) -> Result<()> {
    let resp: GenericResp = client()
        .post(format!("{}{path}", base_url(port)))
        .json(body)
        .send()
        .await
        .map_err(hint)?
        .json()
        .await
        .map_err(hint)?;
    if resp.ok {
        Ok(())
    } else {
        Err(anyhow!(resp.error.unwrap_or_else(|| "未知错误".into())))
    }
}
