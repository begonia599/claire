//! 断点续传的元数据持久化
//!
//! 每个下载任务对应一个 `.rdmmeta` 文件，记录：
//! - 下载地址、目标文件路径、总大小
//! - 是否支持 Range（分段）请求
//! - 每个分段的起止字节和已下载字节
//!
//! 中断后重新运行时，读取这个文件就能从断点继续，不用重头来。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 单个分段的状态
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SegmentState {
    /// 分段序号
    pub index: usize,
    /// 分段起始字节（含）
    pub start: u64,
    /// 分段结束字节（含）
    pub end: u64,
    /// 这个分段已经下载了多少字节
    pub downloaded: u64,
}

impl SegmentState {
    /// 这个分段还剩多少字节没下
    pub fn remaining(&self) -> u64 {
        // 总长度 = end - start + 1（字节是闭区间）
        let total = self.end - self.start + 1;
        total.saturating_sub(self.downloaded)
    }

    /// 当前应该从哪个字节继续下载
    pub fn current_offset(&self) -> u64 {
        self.start + self.downloaded
    }
}

/// 整个下载任务的元数据
#[derive(Serialize, Deserialize, Debug)]
pub struct DownloadMeta {
    pub url: String,
    pub file_path: String,
    /// 文件总大小；服务器不支持 content-length 时为 0
    pub total_size: u64,
    /// 服务器是否支持 Range 请求（决定能否分段）
    pub supports_range: bool,
    /// 所有分段的状态
    pub segments: Vec<SegmentState>,
}

impl DownloadMeta {
    /// 已下载的总字节数（所有分段相加）
    pub fn downloaded_total(&self) -> u64 {
        self.segments.iter().map(|s| s.downloaded).sum()
    }

    /// 是否已经全部下载完成
    pub fn is_complete(&self) -> bool {
        if self.total_size == 0 {
            return false; // 大小未知，没法判断
        }
        self.downloaded_total() >= self.total_size
    }

    /// 根据目标文件路径推导出对应的 .rdmmeta 文件路径
    pub fn meta_path_for(file_path: &Path) -> PathBuf {
        // 比如 file.zip -> file.zip.rdmmeta
        let mut p = file_path.as_os_str().to_os_string();
        p.push(".rdmmeta");
        PathBuf::from(p)
    }

    /// 把元数据写入 .rdmmeta 文件
    pub fn save(&self) -> Result<()> {
        let path = Self::meta_path_for(Path::new(&self.file_path));
        let json = serde_json::to_string_pretty(self)
            .context("序列化元数据失败")?;
        // 先写到临时文件再原子重命名，避免写到一半崩溃损坏元数据
        let tmp = path.with_extension("rdmmeta.tmp");
        std::fs::write(&tmp, json)
            .with_context(|| format!("写入临时元数据失败: {:?}", tmp))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("重命名元数据文件失败: {:?}", path))?;
        Ok(())
    }

    /// 从 .rdmmeta 文件读取元数据
    pub fn load(file_path: &Path) -> Result<Self> {
        let path = Self::meta_path_for(file_path);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("读取元数据失败: {:?}", path))?;
        let meta: DownloadMeta = serde_json::from_str(&content)
            .context("解析元数据失败，文件可能损坏")?;
        Ok(meta)
    }

    /// 删除元数据文件（下载完成后清理）
    pub fn remove(file_path: &Path) -> Result<()> {
        let path = Self::meta_path_for(file_path);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}
