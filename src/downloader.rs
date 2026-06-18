//! 下载引擎：多线程分段下载 + 断点续传 + 可暂停
//!
//! 核心思路：
//! 1. 先向服务器发一个探针请求，问清楚文件多大、支不支持分段（Range 请求）
//! 2. 把文件按字节区间切成 N 段，每段一个 tokio 任务并行下载
//!    每段用 HTTP 头 `Range: bytes=start-end` 只请求自己那部分
//! 3. 各段把收到的数据写到文件的对应位置（seek + write）
//! 4. 每隔一会儿把进度存进 .rdmmeta，中断后下次运行从断点继续
//! 5. 每写一块检查一下暂停标志，被暂停就优雅退出并保存进度
//!
//! 这就是 IDM「多线程加速」的本质——不是真的更快，而是把一段管道
//! 变成 N 条并行管道，吃满服务器给你的带宽。

use anyhow::{Context, Result};
use futures::StreamExt;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::meta::{DownloadMeta, SegmentState};

/// 一次下载的结局，由管理器据此更新任务状态
#[derive(Debug)]
pub enum Outcome {
    /// 全部下完
    Completed { total_size: u64 },
    /// 被用户暂停（.rdmmeta 已保存，可续传）
    Paused { total_size: u64 },
    /// 出错（.rdmmeta 保留以便重试）
    Failed { total_size: u64, error: String },
}

/// 下载器配置
pub struct Downloader {
    pub url: String,
    pub file_path: PathBuf,
    /// 想用多少个分段（线程）
    pub segments: usize,
}

/// prepare 阶段的产物
struct Prepared {
    client: reqwest::Client,
    meta: Arc<Mutex<DownloadMeta>>,
}

impl Downloader {
    /// 管理器调用的主入口：执行下载，期间检查暂停标志，返回结局
    pub async fn run_managed(self, pause_flag: Arc<AtomicBool>) -> Outcome {
        // 1. 准备：探针 / 续传，得到 meta 和 HTTP 客户端
        let Prepared { client, meta } = match self.prepare().await {
            Ok(p) => p,
            Err(e) => {
                return Outcome::Failed {
                    total_size: 0,
                    error: format!("{e:#}"),
                }
            }
        };
        // 2. 派发分段并行下载
        Self::dispatch(client, meta, pause_flag).await
    }

    /// 准备阶段：全新下载走探针+预分配；已存在 .rdmmeta 走续传
    async fn prepare(self) -> Result<Prepared> {
        let client = reqwest::Client::builder()
            .build()
            .context("创建 HTTP 客户端失败")?;

        let meta = if DownloadMeta::meta_path_for(&self.file_path).exists() {
            // 续传
            let m = DownloadMeta::load(&self.file_path)?;
            if m.url != self.url {
                anyhow::bail!(
                    "已存在的 .rdmmeta 对应另一个 URL，无法续传\n  旧: {}\n  新: {}",
                    m.url,
                    self.url
                );
            }
            m
        } else {
            // 全新：探针
            let (total_size, supports_range) = probe(&client, &self.url).await?;
            if total_size == 0 {
                // 服务器不给大小（chunked 传输、不支持 Range，如 GitHub codeload 源码归档）
                // 走流式顺序下载：单连接、不知大小、不能续传，但能下下来
                let m = DownloadMeta {
                    url: self.url.clone(),
                    file_path: self.file_path.to_string_lossy().to_string(),
                    total_size: 0,
                    supports_range: false,
                    segments: vec![SegmentState {
                        index: 0,
                        start: 0,
                        end: 0,
                        downloaded: 0,
                    }],
                };
                // 创建空文件（不预分配，因为不知道大小）
                {
                    ensure_parent(&self.file_path);
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&self.file_path)
                        .context("创建目标文件失败")?;
                }
                m
            } else if supports_range {
                let segs = build_segments(self.segments, total_size);
                let m = DownloadMeta {
                    url: self.url.clone(),
                    file_path: self.file_path.to_string_lossy().to_string(),
                    total_size,
                    supports_range,
                    segments: segs,
                };
                {
                    ensure_parent(&self.file_path);
                    let f = std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&self.file_path)
                        .context("创建目标文件失败")?;
                    f.set_len(total_size).context("预分配文件大小失败")?;
                }
                m
            } else {
                // 知道大小但不支持 Range：单段顺序下
                let m = DownloadMeta {
                    url: self.url.clone(),
                    file_path: self.file_path.to_string_lossy().to_string(),
                    total_size,
                    supports_range,
                    segments: vec![SegmentState {
                        index: 0,
                        start: 0,
                        end: total_size - 1,
                        downloaded: 0,
                    }],
                };
                {
                    ensure_parent(&self.file_path);
                    let f = std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&self.file_path)
                        .context("创建目标文件失败")?;
                    f.set_len(total_size).context("预分配文件大小失败")?;
                }
                m
            }
        };

        Ok(Prepared {
            client,
            meta: Arc::new(Mutex::new(meta)),
        })
    }

    /// 把分段派发给并行任务，统一管理进度和定期保存
    async fn dispatch(
        client: reqwest::Client,
        meta: Arc<Mutex<DownloadMeta>>,
        pause_flag: Arc<AtomicBool>,
    ) -> Outcome {
        let total_size = meta.lock().await.total_size;
        // 服务器不给大小（chunked）→ 走流式顺序下载
        if total_size == 0 {
            return Self::stream_download(client, meta, pause_flag).await;
        }
        let file_path = {
            let m = meta.lock().await;
            m.file_path.clone()
        };

        // 打开文件，所有分段共享这一个句柄（用 Mutex 保证 seek+write 原子）
        ensure_parent(std::path::Path::new(&file_path));
        let file = match std::fs::OpenOptions::new()
            .write(true)
            .open(&file_path)
            .context("打开文件失败")
        {
            Ok(f) => Arc::new(Mutex::new(f)),
            Err(e) => {
                return Outcome::Failed {
                    total_size,
                    error: format!("{e:#}"),
                }
            }
        };

        // 后台任务：每 1 秒把进度存盘（断点续传的关键，硬中断最多丢 1 秒）
        let saver_meta = meta.clone();
        let saver = tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            tick.tick().await; // 第一次立即返回，跳过
            loop {
                tick.tick().await;
                if let Err(e) = saver_meta.lock().await.save() {
                    eprintln!("保存进度失败: {e}");
                }
            }
        });

        // 为每个分段起一个下载任务
        let mut handles = Vec::new();
        {
            let m = meta.lock().await;
            for seg in &m.segments {
                let seg = seg.clone();
                handles.push(tokio::spawn(download_segment(
                    client.clone(),
                    m.url.clone(),
                    seg,
                    file.clone(),
                    meta.clone(),
                    pause_flag.clone(),
                )));
            }
        }

        // 等所有分段结束（完成、或因暂停提前退出）
        let mut errors = Vec::new();
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => errors.push(format!("分段失败: {e:#}")),
                Err(e) => errors.push(format!("任务崩溃: {e}")),
            }
        }

        saver.abort();
        // 最后再存一次进度
        if let Err(e) = meta.lock().await.save() {
            eprintln!("最终保存进度失败: {e}");
        }

        // 被暂停：保留 .rdmmeta，等下次续传
        if pause_flag.load(Ordering::Relaxed) {
            return Outcome::Paused { total_size };
        }
        if !errors.is_empty() {
            return Outcome::Failed {
                total_size,
                error: errors.join("\n"),
            };
        }

        // 全部成功：删掉 .rdmmeta，文件就是干净的最终产物
        if let Err(e) = DownloadMeta::remove(Path::new(&file_path)) {
            eprintln!("清理 .rdmmeta 失败: {e}");
        }
        Outcome::Completed { total_size }
    }

    /// 流式顺序下载：服务器不给大小（chunked / 不支持 Range）时使用。
    /// 单连接、顺序写盘、不知总大小、不能续传（中断后重来）。
    async fn stream_download(
        client: reqwest::Client,
        meta: Arc<Mutex<DownloadMeta>>,
        pause_flag: Arc<AtomicBool>,
    ) -> Outcome {
        let (url, file_path) = {
            let m = meta.lock().await;
            (m.url.clone(), m.file_path.clone())
        };

        // 创建/清空文件
        ensure_parent(std::path::Path::new(&file_path));
        let mut file = match std::fs::File::create(&file_path) {
            Ok(f) => f,
            Err(e) => {
                return Outcome::Failed {
                    total_size: 0,
                    error: format!("创建文件失败: {e:#}"),
                }
            }
        };

        // 全量 GET（不带 Range）
        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Outcome::Failed {
                    total_size: 0,
                    error: format!("请求失败: {e:#}"),
                }
            }
        };
        if !resp.status().is_success() {
            return Outcome::Failed {
                total_size: 0,
                error: format!("服务器返回 {}", resp.status()),
            };
        }

        // 后台定期存盘（记录已下载字节，供 /list 显示进度）
        let saver_meta = meta.clone();
        let saver = tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            tick.tick().await;
            loop {
                tick.tick().await;
                let _ = saver_meta.lock().await.save();
            }
        });

        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;
        let mut paused = false;
        while let Some(chunk) = stream.next().await {
            if pause_flag.load(Ordering::Relaxed) {
                paused = true;
                break;
            }
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    saver.abort();
                    let _ = meta.lock().await.save();
                    return Outcome::Failed {
                        total_size: downloaded,
                        error: format!("读取数据失败: {e:#}"),
                    };
                }
            };
            if let Err(e) = file.write_all(&chunk) {
                saver.abort();
                let _ = meta.lock().await.save();
                return Outcome::Failed {
                    total_size: downloaded,
                    error: format!("写盘失败: {e:#}"),
                };
            }
            downloaded += chunk.len() as u64;
            {
                let mut m = meta.lock().await;
                m.segments[0].downloaded = downloaded;
            }
        }

        saver.abort();
        let _ = meta.lock().await.save();

        if paused {
            // 流式无法续传，暂停 = 下次重来。保留 .rdmmeta 以便显示已下字节。
            return Outcome::Paused { total_size: downloaded };
        }

        // 完成：实际总大小 = 已下载字节
        if let Err(e) = DownloadMeta::remove(Path::new(&file_path)) {
            eprintln!("清理 .rdmmeta 失败: {e}");
        }
        println!("[stream] ✅ 完成 {} ({})", file_path, format_size(downloaded));
        Outcome::Completed { total_size: downloaded }
    }
}

/// 探针请求：用一个 `Range: bytes=0-0` 请求来探测
/// - 文件总大小（从 Content-Range 或 Content-Length）
/// - 是否支持分段（返回 206 Partial Content 即支持）
async fn probe(client: &reqwest::Client, url: &str) -> Result<(u64, bool)> {
    let resp = client
        .get(url)
        .header("Range", "bytes=0-0")
        .send()
        .await
        .with_context(|| format!("请求失败: {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("服务器返回错误状态: {} (URL: {url})", resp.status());
    }

    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        // 206: 支持 Range。总大小在 Content-Range: bytes 0-0/12345 的最后一部分
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split('/').nth(1))
            .and_then(|s| s.parse::<u64>().ok())
            .context("支持分段但解析 Content-Range 失败")?;
        Ok((total, true))
    } else {
        // 200: 不支持 Range，Content-Length 给出总大小（可能为空）
        let total = resp.content_length().unwrap_or(0);
        Ok((total, false))
    }
}

/// 把 [0, total) 切成 n 段，每段尽量等长，余数塞到前几段
fn build_segments(n: usize, total: u64) -> Vec<SegmentState> {
    let n = n.max(1) as u64;
    let base = total / n;
    let rem = total % n;
    let mut segs = Vec::with_capacity(n as usize);
    let mut offset = 0u64;
    for i in 0..n {
        // 前 rem 段每段多 1 字节，把除不尽的余数分掉
        let len = base + if i < rem { 1 } else { 0 };
        let start = offset;
        let end = offset + len - 1; // 闭区间
        segs.push(SegmentState {
            index: i as usize,
            start,
            end,
            downloaded: 0,
        });
        offset = end + 1;
    }
    segs
}

/// 下载单个分段
async fn download_segment(
    client: reqwest::Client,
    url: String,
    mut seg: SegmentState,
    file: Arc<Mutex<std::fs::File>>,
    meta: Arc<Mutex<DownloadMeta>>,
    pause_flag: Arc<AtomicBool>,
) -> Result<()> {
    // 如果这段已经下完就跳过
    if seg.remaining() == 0 {
        return Ok(());
    }

    // Range 头：从当前断点请求到段尾
    let range = format!("bytes={}-{}", seg.current_offset(), seg.end);
    let resp = client
        .get(&url)
        .header("Range", &range)
        .send()
        .await
        .context("分段请求失败")?;

    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT
        && resp.status() != reqwest::StatusCode::OK
    {
        anyhow::bail!("分段请求返回异常状态: {}", resp.status());
    }

    // 以字节流方式读取，逐块写盘
    let mut stream = resp.bytes_stream();
    let mut paused = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取响应数据失败")?;
        let n = chunk.len() as u64;

        // 写到文件对应位置：加锁后 seek+write，保证多段不互相覆盖
        {
            let mut f = file.lock().await;
            f.seek(SeekFrom::Start(seg.current_offset()))
                .context("文件定位失败")?;
            f.write_all(&chunk).context("写入文件失败")?;
        }

        seg.downloaded += n;

        // 把本段进度回写到共享 meta，后台存盘任务才能存到真实进度
        // 这是断点续传能正确跳过已下字节的关键
        {
            let mut m = meta.lock().await;
            m.segments[seg.index].downloaded = seg.downloaded;
        }

        // 检查暂停：被要求暂停就提前退出（不算错误，进度已保存）
        if pause_flag.load(Ordering::Relaxed) {
            paused = true;
            break;
        }
    }

    if paused {
        return Ok(()); // 暂停退出，不做完整性校验
    }

    // 校验这段是否下完整了
    if seg.remaining() != 0 {
        anyhow::bail!(
            "分段 {} 未下完，缺 {} 字节",
            seg.index,
            seg.remaining()
        );
    }
    Ok(())
}

/// 确保目标文件的父目录存在（分类目录可能还没建）
fn ensure_parent(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

/// 把字节数格式化成人类可读的尺寸，例如 1234567 -> "1.18 MiB"
pub fn format_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if b >= GB {
        format!("{:.2} GiB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.2} MiB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.2} KiB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}
