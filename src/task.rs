//! 任务类型与状态机
//!
//! 一个 Task 描述"要下什么、下到哪、现在什么状态"。
//! 字节级的进度（每段下了多少）不存这里，而是存在各任务自己的 .rdmmeta 里，
//! 这里只存任务级的信息：状态、错误信息、总大小（探测后才知道）。

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 任务状态机
///   Awaiting    待用户确认（弹窗确认文件名/目录/分段）
///   Queued      已确认入队，等并发空位
///   Downloading 正在下
///   Paused      用户暂停
///   Completed   完成
///   Failed      出错（可 retry 回 Queued）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Awaiting,
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
}

impl TaskState {
    /// 中文显示名
    pub fn label(&self) -> &'static str {
        match self {
            TaskState::Awaiting => "待确认",
            TaskState::Queued => "排队中",
            TaskState::Downloading => "下载中",
            TaskState::Paused => "已暂停",
            TaskState::Completed => "已完成",
            TaskState::Failed => "失败",
        }
    }
}

/// 一个下载任务（会序列化进 tasks.json）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Task {
    pub id: u64,
    pub url: String,
    pub file_path: String,
    pub segments: usize,
    pub state: TaskState,
    /// 失败时的错误信息
    #[serde(default)]
    pub error: Option<String>,
    /// 文件总大小，探测后才填；未探测为 0
    #[serde(default)]
    pub total_size: u64,
    /// 入队时间（unix 秒）
    #[serde(default)]
    pub added_at: u64,
    /// 当前运行代号：每次被调度器启动时 +1。
    /// 用来防止"旧一次运行的收尾回调"覆盖"新一次运行/续传"设的状态。
    #[serde(default)]
    pub run_id: u64,
}

impl Task {
    pub fn new(id: u64, url: String, file_path: String, segments: usize) -> Self {
        Self {
            id,
            url,
            file_path,
            segments,
            state: TaskState::Queued,
            error: None,
            total_size: 0,
            added_at: now_secs(),
            run_id: 0,
        }
    }
}

/// 单个分段的进度视图（供前端画 IDM 那种分段进度条）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SegView {
    /// 这段一共有多少字节
    pub size: u64,
    /// 这段已经下了多少字节
    pub downloaded: u64,
}

/// /list 接口返回的视图：在 Task 基础上附带当前进度
#[derive(Serialize, Deserialize, Debug)]
pub struct TaskView {
    pub id: u64,
    pub url: String,
    pub file: String,
    pub state: String,
    pub total: u64,
    pub downloaded: u64,
    pub error: Option<String>,
    /// 各分段进度（下载中/暂停时从 .rdmmeta 读出；其余状态为空）
    /// 前端拿到后画"一条进度条分 N 格、每格独立填充"的效果
    #[serde(default)]
    pub segments: Vec<SegView>,
}

/// 当前 unix 时间戳（秒）
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
