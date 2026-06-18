//! 任务注册表持久化（tasks.json）
//!
//! 和 .rdmmeta 的分工：
//!   .rdmmeta  —— 单个任务的字节级进度（每段下了多少），下载中途每 1 秒存
//!   tasks.json —— 所有任务的清单（id、url、状态、错误），状态一变就存
//!
//! 数据目录：%APPDATA%\rdm\（Windows）或 ~/.config/rdm/（Linux），用 dirs crate 取。
//! 这样无论 daemon 从哪个工作目录启动，都读写同一份清单。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::task::Task;

/// 任务注册表（tasks.json 的内容）
#[derive(serde::Serialize, serde::Deserialize, Debug, Default)]
pub struct Registry {
    /// 下一个要分配的 id
    #[serde(default)]
    pub next_id: u64,
    /// 所有任务（含已完成，直到被 rm）
    #[serde(default)]
    pub tasks: Vec<Task>,
}

impl Registry {
    /// 返回数据目录路径，不存在则创建
    pub fn data_dir() -> Result<PathBuf> {
        let base = dirs::data_dir()
            .or_else(|| dirs::config_dir())
            .context("找不到系统数据目录")?;
        let dir = base.join("rdm");
        if !dir.exists() {
            std::fs::create_dir_all(&dir).with_context(|| format!("创建数据目录失败: {:?}", dir))?;
        }
        Ok(dir)
    }

    fn registry_path() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("tasks.json"))
    }

    /// 从磁盘加载注册表；文件不存在时返回空表
    pub fn load() -> Result<Self> {
        let path = Self::registry_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("读取注册表失败: {:?}", path))?;
        let reg: Registry = serde_json::from_str(&content)
            .context("解析 tasks.json 失败，文件可能损坏")?;
        Ok(reg)
    }

    /// 原子写入：先写临时文件再 rename，避免写到一半崩溃损坏清单
    pub fn save(&self) -> Result<()> {
        let path = Self::registry_path()?;
        let json = serde_json::to_string_pretty(self).context("序列化注册表失败")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("写入临时注册表失败: {:?}", tmp))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("重命名注册表失败: {:?}", path))?;
        Ok(())
    }

    /// 按 id 查找任务的索引
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tasks.iter().position(|t| t.id == id)
    }
}

/// 给定输出文件名，返回它在数据目录下的 .rdmmeta 路径
/// （下载进度文件始终跟目标文件放在一起，由 meta 模块管理；这里只是辅助）
#[allow(dead_code)]
pub fn meta_path_for_file(file: &Path) -> PathBuf {
    crate::meta::DownloadMeta::meta_path_for(file)
}

/// 把文件名解析成绝对路径：绝对路径原样用，相对路径落到用户"下载"目录。
/// 浏览器扩展只给得出文件名（不知道系统下载目录），由 daemon 这里统一解析。
pub fn resolve_download_path(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() {
        return p;
    }
    let base = dirs::download_dir().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    });
    base.join(p)
}

/// 从 URL 末段推断文件名；取不到就用 download.bin
pub fn infer_filename(url: &str) -> String {
    url.split('?')
        .next()
        .and_then(|s| s.rsplit('/').next())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "download.bin".to_string())
}
