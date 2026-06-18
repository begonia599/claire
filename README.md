# rdm — Rust Download Manager

一个用 Rust 写的多线程下载管理器，效仿 IDM 的核心体验：多线程分段加速、断点续传、下载队列、浏览器接管、下载前确认弹窗、按文件类型分类。

```
┌────────────┐   invoke/HTTP   ┌──────────────┐
│  GUI (Tauri)│ ──────────────▶ │ TaskManager  │
│  + 扩展(MV3) │                │  调度器/状态机 │
└────────────┘                  └──────┬───────┘
                                       │
                          ┌────────────┴────────────┐
                          │ Downloader (多段+续传)    │
                          │ axum :7319 (供扩展/CLI)   │
                          └─────────────────────────┘
```

## 功能

- **多线程分段下载**：HTTP Range 请求，N 段并行写同一文件（seek+write，无需合并）
- **断点续传**：每段进度存 `.rdmmeta`，中断后从断点继续，文件逐字节一致
- **流式回退**：服务器不支持 Range / chunked（如 GitHub codeload 源码归档）时自动走单连接顺序下载
- **下载队列**：daemon 模式，并发限制，暂停/继续/重试/删除，状态持久化（重启恢复）
- **浏览器接管**：Chrome/Edge 扩展（MV3）拦截下载 + 右键菜单，转交 rdm
- **下载前确认**：弹窗确认文件名/保存目录/分段，支持原生目录选择
- **文件分类**：按扩展名自动归类到不同目录（压缩/视频/音频/文档/程序/图片/通用），可自定义
- **GUI**：Tauri 桌面应用，分段进度条（IDM 风格分格）、不确定式流式进度

## 目录结构

```
rdm/
├── src/            # 核心库（lib）：downloader/manager/server/task/store/meta/categories
├── src/main.rs     # CLI 二进制：rdm serve / add / list / pause / ...
├── src-tauri/      # Tauri GUI 后端（内嵌 daemon + 命令）
├── frontend/       # GUI 前端（HTML/CSS/JS，Geist 字体）
├── extension/      # 浏览器扩展（MV3：manifest/background/popup/icons）
└── Cargo.toml      # workspace
```

## 构建

需要 Rust 1.75+ 和 Windows 上 WebView2（Win11 自带）。

```bash
# CLI
cargo build --release -p rdm        # → target/release/rdm.exe

# GUI
cargo build --release -p rdm-gui    # → target/release/rdm-gui.exe
```

## 使用

### GUI
双击 `rdm-gui.exe`。粘贴链接 → 添加下载 → 确认弹窗 → 开始。
齿轮按钮配置分类。下载默认到"下载"目录，按分类落到子目录。

### CLI（daemon 模式）
```bash
rdm serve --max-concurrent 3        # 启动 daemon
rdm add <URL> [文件名] --segments 8  # 添加任务
rdm list / status                    # 查看
rdm pause <id> | resume <id> | retry <id>
rdm rm <id> --purge                  # 删除（含文件）
```

### 浏览器扩展
Chrome/Edge → `edge://extensions` → 开发人员模式 → 加载解压缩的扩展 → 选 `extension/` 目录。
扩展 popup 里可开关"接管下载"和"下载前确认"。

## 状态

- 阶段 1-4 完成：分段下载、续传、队列、GUI、浏览器接管、确认弹窗、分类
- 已知限制：独立确认小窗在 WebView2 下白屏，暂用主窗口内模态 + 抢焦点替代；下一阶段考虑原生 GUI（egui）

## 许可

MIT
