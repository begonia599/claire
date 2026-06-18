//! 文件类型分类（IDM 风格的"按扩展名归类到不同目录"）
//!
//! 分类配置存 `categories.json`（数据目录）。每个分类有名字、目录、一组扩展名。
//! 下载确认弹窗根据文件扩展名自动选中分类，并把"保存目录"默认填成该分类的目录。
//! 用户可在设置里增删改分类。"通用"分类没有扩展名，作为兜底。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Category {
    pub name: String,
    /// 该分类的保存目录（绝对路径）
    pub dir: String,
    /// 匹配的扩展名（不带点，小写）。空列表 = 通用兜底分类
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Categories(pub Vec<Category>);

impl Categories {
    fn path() -> Result<PathBuf> {
        Ok(crate::store::Registry::data_dir()?.join("categories.json"))
    }

    /// 读取；不存在则写入默认并返回
    pub fn load() -> Result<Self> {
        let p = Self::path()?;
        if !p.exists() {
            let d = Self::defaults();
            d.save()?;
            return Ok(d);
        }
        let s = std::fs::read_to_string(&p).with_context(|| format!("读取分类失败: {p:?}"))?;
        Ok(serde_json::from_str(&s).context("解析 categories.json 失败")?)
    }

    /// 原子保存
    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).context("序列化分类失败")?)
            .with_context(|| format!("写分类失败: {tmp:?}"))?;
        std::fs::rename(&tmp, &p).with_context(|| format!("重命名分类失败: {p:?}"))?;
        Ok(())
    }

    /// 按扩展名找分类；找不到走第一个空扩展名列表的"通用"分类；都没有就返回 None
    pub fn match_by_ext(&self, ext: &str) -> Option<&Category> {
        let ext = ext.trim_start_matches('.').to_lowercase();
        if !ext.is_empty() {
            for c in &self.0 {
                if c.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                    return Some(c);
                }
            }
        }
        // 兜底：第一个没有扩展名的分类（"通用"）
        self.0.iter().find(|c| c.extensions.is_empty())
    }

    /// 默认分类表。目录基于"下载"目录下的子文件夹。
    pub fn defaults() -> Self {
        let dl = dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."));
        let sub = |name: &str| dl.join(name).to_string_lossy().to_string();
        Categories(vec![
            Category { name: "压缩文件".into(), dir: sub("Compressed"), extensions: split("zip rar 7z gz tar bz2 xz tgz") },
            Category { name: "视频".into(), dir: sub("Video"), extensions: split("mp4 mkv avi mov flv webm m4v ts wmv") },
            Category { name: "音频".into(), dir: sub("Audio"), extensions: split("mp3 flac aac ogg wav m4a opus") },
            Category { name: "文档".into(), dir: sub("Documents"), extensions: split("pdf doc docx xls xlsx ppt pptx txt md epub pages") },
            Category { name: "程序".into(), dir: sub("Programs"), extensions: split("exe msi apk deb rpm dmg pkg appimage") },
            Category { name: "图片".into(), dir: sub("Pictures"), extensions: split("jpg jpeg png gif bmp webp svg tiff ico") },
            Category { name: "通用".into(), dir: dl.to_string_lossy().to_string(), extensions: vec![] },
        ])
    }
}

/// 取扩展名（不带点，小写）；没有返回空串
pub fn ext_of(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn split(s: &str) -> Vec<String> {
    s.split_whitespace().map(|x| x.to_string()).collect()
}
