//! 任务管理器：任务表 + 并发调度 + 状态机驱动 + 持久化
//!
//! 它是 daemon 的"大脑"：
//!   - 持有所有任务（Registry）和正在跑的任务（running map）
//!   - 一个调度循环每 500ms 看一眼：有空位就把 Queued 的任务启动
//!   - 下载任务的结局（Outcome）回写到这里，更新状态并存盘
//!   - 暂停/继续/删除/重试 都在这里改状态
//!
//! 暂停靠 Arc<AtomicBool>：manager 持有每个运行任务的暂停标志，
//! `pause(id)` 把它置 true，下载循环下个 chunk 检查到就优雅退出。

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};

use crate::downloader::{Downloader, Outcome};
use crate::meta::DownloadMeta;
use crate::store::Registry;
use crate::task::{SegView, Task, TaskState, TaskView};

/// 一个正在运行的任务：暂停标志 + 句柄 + 运行代号
struct RunningTask {
    pause_flag: Arc<AtomicBool>,
    join: JoinHandle<()>,
    run_id: u64,
}

struct Inner {
    registry: Registry,
    running: HashMap<u64, RunningTask>,
}

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Mutex<Inner>>,
    max_concurrent: usize,
}

impl TaskManager {
    /// 加载已有注册表，创建管理器。
    /// daemon 重启时，原本 Downloading 的任务降级为 Queued（续传会接着下）。
    pub fn new(max_concurrent: usize) -> Result<Self> {
        let mut registry = Registry::load()?;
        for t in &mut registry.tasks {
            if t.state == TaskState::Downloading {
                t.state = TaskState::Queued;
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                registry,
                running: HashMap::new(),
            })),
            max_concurrent,
        })
    }

    /// 添加任务，返回新 id
    pub async fn add(
        &self,
        url: String,
        file_path: String,
        segments: usize,
    ) -> Result<u64> {
        let mut inner = self.inner.lock().await;
        let id = inner.registry.next_id;
        inner.registry.next_id += 1;
        let task = Task::new(id, url, file_path, segments);
        inner.registry.tasks.push(task);
        inner.registry.save()?;
        Ok(id)
    }

    /// 添加"待确认"任务（扩展拦截后走这里，等 GUI 弹窗确认再 confirm）
    pub async fn add_awaiting(&self, url: String, filename: String) -> Result<u64> {
        let mut inner = self.inner.lock().await;
        let id = inner.registry.next_id;
        inner.registry.next_id += 1;
        let mut task = Task::new(id, url, String::new(), 8);
        task.state = TaskState::Awaiting;
        // file_path 暂存建议文件名（非绝对路径），确认时才落定
        task.file_path = filename;
        inner.registry.tasks.push(task);
        inner.registry.save()?;
        Ok(id)
    }

    /// 确认待确认任务：落定保存路径 + 分段，转为 Queued
    pub async fn confirm(&self, id: u64, file_path: String, segments: usize) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let t = inner
            .registry
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("任务 #{id} 不存在"))?;
        if t.state != TaskState::Awaiting {
            return Err(anyhow!("任务 #{id} 不是待确认状态，无法确认"));
        }
        t.file_path = file_path;
        t.segments = segments;
        t.state = TaskState::Queued;
        inner.registry.save()?;
        Ok(())
    }

    /// 列出所有任务的视图（带当前进度 + 各分段进度）
    pub async fn list(&self) -> Vec<TaskView> {
        let inner = self.inner.lock().await;
        let mut views = Vec::with_capacity(inner.registry.tasks.len());
        for t in &inner.registry.tasks {
            let meta = load_meta(t);
            let (downloaded, total, segs) = match t.state {
                // 已完成的 .rdmmeta 已清理，按满进度显示，不分段
                TaskState::Completed => (t.total_size, t.total_size, Vec::new()),
                // 运行中/暂停：用 meta 里的实时进度 + 各段进度
                TaskState::Downloading | TaskState::Paused => {
                    if let Some(m) = &meta {
                        let segs: Vec<SegView> = m
                            .segments
                            .iter()
                            .map(|s| SegView {
                                size: s.end - s.start + 1,
                                downloaded: s.downloaded,
                            })
                            .collect();
                        (m.downloaded_total(), m.total_size.max(t.total_size), segs)
                    } else {
                        (0, t.total_size, Vec::new())
                    }
                }
                // 排队/失败：还没下或下不了，进度 0
                _ => (0, t.total_size, Vec::new()),
            };
            views.push(TaskView {
                id: t.id,
                url: t.url.clone(),
                file: t.file_path.clone(),
                state: t.state.label().to_string(),
                total,
                downloaded,
                error: t.error.clone(),
                segments: segs,
            });
        }
        views
    }

    /// 暂停任务
    pub async fn pause(&self, id: u64) -> Result<()> {
        // 锁内：先从 running 取出 join（置暂停标志），再借 tasks 改状态
        let join_to_wait = {
            let mut inner = self.inner.lock().await;
            // 1. 处理 running：置标志 + 取 join（只借 running，不碰 tasks）
            let join_opt = if let Some(r) = inner.running.get(&id) {
                r.pause_flag.store(true, Ordering::Relaxed);
                inner.running.remove(&id).map(|r| r.join)
            } else {
                None
            };
            // 2. 借 tasks 改状态
            let t = inner
                .registry
                .tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| anyhow!("任务 #{id} 不存在"))?;
            match t.state {
                TaskState::Downloading | TaskState::Queued => {
                    t.state = TaskState::Paused;
                }
                other => return Err(anyhow!("任务 #{id} 当前状态 {other:?}，无法暂停")),
            }
            let _ = inner.registry.save();
            join_opt
        };
        // 锁外等待旧 run 真正结束，避免立即续传时两个 run 同时写同一文件
        if let Some(join) = join_to_wait {
            let _ = join.await;
        }
        Ok(())
    }

    /// 继续（暂停/失败 → 排队，等调度器拾起）
    pub async fn resume(&self, id: u64) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let t = inner
            .registry
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("任务 #{id} 不存在"))?;
        match t.state {
            TaskState::Paused | TaskState::Failed => {
                t.state = TaskState::Queued;
                t.error = None;
                inner.registry.save()?;
                Ok(())
            }
            _ => Err(anyhow!("任务 #{id} 当前状态 {:?}，无法继续", t.state)),
        }
    }

    /// 重试失败的任务（语义同 resume）
    pub async fn retry(&self, id: u64) -> Result<()> {
        self.resume(id).await
    }

    /// 删除任务；purge=true 时连文件和 .rdmmeta 一起删
    pub async fn rm(&self, id: u64, purge: bool) -> Result<()> {
        let mut inner = self.inner.lock().await;
        // 先停掉运行中的任务
        if let Some(r) = inner.running.remove(&id) {
            r.join.abort();
        }
        let pos = inner
            .registry
            .index_of(id)
            .ok_or_else(|| anyhow!("任务 #{id} 不存在"))?;
        let task = inner.registry.tasks.remove(pos);

        if purge {
            let p = PathBuf::from(&task.file_path);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
            let _ = DownloadMeta::remove(&p);
        }
        inner.registry.save()?;
        Ok(())
    }

    /// 总览状态
    pub async fn status(&self) -> StatusSummary {
        let inner = self.inner.lock().await;
        let mut s = StatusSummary::default();
        s.running = inner.running.len() as u64;
        for t in &inner.registry.tasks {
            match t.state {
                TaskState::Awaiting => s.awaiting += 1,
                TaskState::Queued => s.queued += 1,
                TaskState::Downloading => {}
                TaskState::Paused => s.paused += 1,
                TaskState::Completed => s.completed += 1,
                TaskState::Failed => s.failed += 1,
            }
        }
        s
    }

    /// 启动调度循环（在 daemon 主流程里 spawn）
    pub fn spawn_scheduler(self) {
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_millis(500));
            tick.tick().await; // 跳过立即返回的那次
            loop {
                tick.tick().await;
                self.schedule_once().await;
            }
        });
    }

    /// 一轮调度：有空位就启动排队中的任务
    async fn schedule_once(&self) {
        // 在锁内挑出可启动任务（带上新 run_id），锁外再 spawn
        let to_start: Vec<(u64, u64, Downloader)> = {
            let mut inner = self.inner.lock().await;
            let max = self.max_concurrent;
            let running_len = inner.running.len();
            if running_len >= max {
                return;
            }
            let mut picked = Vec::new();
            let mut picked_count = 0usize;
            for t in inner.registry.tasks.iter_mut() {
                if running_len + picked_count >= max {
                    break;
                }
                if t.state == TaskState::Queued {
                    t.state = TaskState::Downloading;
                    t.run_id += 1; // 新一次运行
                    let run_id = t.run_id;
                    picked_count += 1;
                    picked.push((
                        t.id,
                        run_id,
                        Downloader {
                            url: t.url.clone(),
                            file_path: PathBuf::from(&t.file_path),
                            segments: t.segments,
                        },
                    ));
                }
            }
            if picked.is_empty() {
                return;
            }
            let _ = inner.registry.save();
            picked
        };

        for (id, run_id, dl) in to_start {
            let pause_flag = Arc::new(AtomicBool::new(false));
            let manager = self.clone();
            let pf = pause_flag.clone();
            let join = tokio::spawn(async move {
                let outcome = dl.run_managed(pf.clone()).await;
                manager.handle_outcome(id, run_id, outcome).await;
            });
            let mut inner = self.inner.lock().await;
            inner.running.insert(
                id,
                RunningTask {
                    pause_flag,
                    join,
                    run_id,
                },
            );
        }
    }

    /// 下载任务结束后回调：更新状态、移出 running、存盘。
    /// 带 run_id 守卫：只有当代号匹配当前 run 时才改状态/移 running，
    /// 避免"暂停后立即续传"时旧 run 的收尾把新 run 的状态覆盖回去。
    async fn handle_outcome(&self, id: u64, run_id: u64, outcome: Outcome) {
        let mut inner = self.inner.lock().await;
        // 只在 running 里仍是这一次 run 时才移除（别误删新 run 的条目）
        if inner.running.get(&id).map(|r| r.run_id) == Some(run_id) {
            inner.running.remove(&id);
        }
        let t = match inner.registry.tasks.iter_mut().find(|t| t.id == id) {
            Some(t) => t,
            None => return, // 任务可能已被 rm，忽略
        };
        // 旧 run 的收尾：当前 run_id 已不同（被续传/重试覆盖），不动状态
        if t.run_id != run_id {
            return;
        }
        match outcome {
            Outcome::Completed { total_size } => {
                t.state = TaskState::Completed;
                t.total_size = total_size;
                t.error = None;
                println!("[#{}] ✅ 完成 ({} )", id, crate::downloader::format_size(total_size));
            }
            Outcome::Paused { total_size } => {
                t.state = TaskState::Paused;
                t.total_size = total_size;
                println!("[#{}] ⏸ 已暂停", id);
            }
            Outcome::Failed { total_size, error } => {
                t.state = TaskState::Failed;
                t.total_size = total_size;
                t.error = Some(error.clone());
                println!("[#{}] ❌ 失败: {}", id, error);
            }
        }
        let _ = inner.registry.save();
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default, Debug)]
pub struct StatusSummary {
    pub running: u64,
    pub queued: u64,
    pub paused: u64,
    pub completed: u64,
    pub failed: u64,
    pub awaiting: u64,
}

/// 读取任务的 .rdmmeta（仅下载中/暂停时有意义；其余状态返回 None）
fn load_meta(t: &Task) -> Option<DownloadMeta> {
    if matches!(t.state, TaskState::Downloading | TaskState::Paused) {
        let p = PathBuf::from(&t.file_path);
        DownloadMeta::load(&p).ok()
    } else {
        None
    }
}
