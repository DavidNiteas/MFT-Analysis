# MFT Analysis — 磁盘占用分析 / Disk Usage Analyzer

[English](#english) | [中文](#中文)

<a id="中文"></a>
## 中文

直接读取 NTFS 的 MFT（Master File Table），几秒内统计出整个卷的磁盘占用分布。不递归遍历文件系统，因此**快得多**，也不受文件占用、权限（对目录树的）影响。

### 特点

- **快**：扫描约 150 万条 MFT 记录仅需数秒（SSD），没有逐文件的 API 遍历
- **准**：按 runlist 实际分配的簇计算物理占用，对 NTFS 压缩、稀疏文件、容器镜像层均准确；全卷总量与 `fsutil` 对账偏差 < 1%
- **全**：包含资源管理器看不到或进不去的部分（`System Volume Information`、`$Extend`、休眠文件等系统元数据）
- **GUI**：Tauri 2 + React，目录树 + Top 文件排行 + 空间饼图；界面支持**中英切换**与**白天/黑夜模式**
- **双许可**：MIT OR Apache-2.0（与 Rust 相同）

### 功能

- 目录树按占用排序展开，可切换显示/隐藏文件，目录与文件以不同图标区分并均显示大小
- 右键菜单：打开所在目录 / 打开此目录 / 复制路径 / 查看空间饼图（目录）
- Top 文件排行（按文件名或后缀聚合的饼图分析，支持 Top-K，其余归「其他」）
- 「刷新」按钮：磁盘变动后一键重扫，保留目录树展开状态，仅更新数据
- 导出：Top 文件列表 CSV、当前展开层级的目录树文本（UTF-8 带 BOM，写到「文档\MFT-Analysis\」）
- 对账行：磁盘已用 / 已统计 / 差额（NTFS 元数据与保留区）

### 工作原理

1. 读取 Boot Sector 获取卷参数，解析 `$MFT` 自身的 `$DATA` runlist，分段读完整个 MFT（不依赖"连续 N 条空记录即结束"的猜测）
2. 逐记录解析 `$FILE_NAME` / `$DATA` / `$INDEX_ALLOCATION`，应用 USA fixup，并把扩展记录（`$ATTRIBUTE_LIST` 指向的）的大小合并回基础记录
3. 重建目录树，后序遍历累加每个目录的子树大小

### 大小口径（重要）

本工具统计的是**物理磁盘占用**（实际分配的簇），不是文件的逻辑大小：

- NTFS 压缩、稀疏文件按真实占用计算
- 容器镜像层占位文件（WCIFS，未在本卷分配簇）计 0
- 硬链接文件只计一次

因此与资源管理器属性页（"大小 / 占用空间"）对比时，典型差异 ±1~2%，来源包括：硬链接 Explorer 按路径重复统计、扫描期间的磁盘活动、NTFS 元数据本身的归属口径。界面顶部提供「磁盘已用 / 已统计 / 差额」对账行，差额即未归属的元数据与保留区。

### 使用要求

- Windows 10 / 11，NTFS 卷
- **必须以管理员身份运行**（读取裸卷 `\\.\X:` 需要相应权限）
- WebView2 运行时（Windows 10 1803+ / Windows 11 自带）

### 直接使用（无需构建）

仓库根目录的 [`mft-analysis-studio.exe`](./mft-analysis-studio.exe) 就是编译好的完整程序（单文件，无需安装）：

1. 下载该文件（GitHub 页面右上角 Code → Download ZIP，或单独下载此文件）
2. **右键 → 以管理员身份运行**
3. 选择盘符，点击「开始扫描」

> 程序未做代码签名，首次运行 Windows SmartScreen 可能提示"未知发布者"，选择"仍要运行"即可。
> 根目录的 exe 随版本发布更新；如需最新构建，可按下方说明自行编译。

### 构建

前置依赖：Rust toolchain、[Node.js](https://nodejs.org)。

```bash
# 1. 构建前端（产物输出到仓库根目录 dist/）
cd ui
npm install
npm run build

# 2. 构建后端（单文件可执行程序，无需安装）
cargo build --release
# 产物：target/release/mft-analysis-studio.exe
```

开发模式（热重载）：

```bash
cargo install tauri-cli --version "^2"   # 仅需一次
cd ui && npm run dev                     # 终端 1：vite dev server
cargo tauri dev                          # 终端 2（在仓库根目录执行）
```

### 使用

启动后选择盘符，点击「开始扫描」。默认 MFT 读取上限 8 GB（约 800 万条记录，远超实际需求），可在界面调整；扫描完成后左侧为目录树（按占用排序，可展开），右侧为 Top 文件排行。界面右上角可切换中英文与白天/黑夜模式，选择会本地保存。

### 项目结构

```
core/             mft-analysis-core：MFT 扫描与解析算法（纯 Rust，无 GUI 依赖）
studio/           mft-analysis-studio：Tauri GUI（Rust 后端 + 窗口配置 + 图标）
ui/               React 前端（Vite 构建，产物输出到根目录 dist/）
```

### 安全说明

程序以只读方式打开卷，**不会写入任何数据**；但 MFT 是活动卷的内部结构，扫描结果只是某一时刻的快照，且运行期间文件增删会带来轻微误差。

---

<a id="english"></a>
## English

Reads the NTFS Master File Table (MFT) directly and computes the disk usage distribution of an entire volume within seconds. It never walks the file system recursively, so it is **much faster** and unaffected by locked files or directory permissions.

### Features

- **Fast**: scanning ~1.5 million MFT records takes only a few seconds (SSD) — no per-file API enumeration
- **Accurate**: physical usage is computed from runlist-allocated clusters, correct for NTFS compression, sparse files and container image layers; total volume figures reconcile with `fsutil` within < 1%
- **Complete**: includes what Explorer cannot see or enter (`System Volume Information`, `$Extend`, hibernation files and other system metadata)
- **GUI**: Tauri 2 + React, with directory tree, top-files ranking and space pie charts; the UI supports **Chinese/English switching** and **light/dark mode**
- **Dual licensed**: MIT OR Apache-2.0 (same as Rust)

### Capabilities

- Directory tree sorted by usage, expandable; optional show/hide of files; folders and files use different icons, both with sizes
- Context menu: open containing folder / open this folder / copy path / show space pie chart (folders)
- Top files ranking (pie analysis by file name or aggregated by extension, with Top-K and an "Other" bucket)
- "Refresh" button: re-scan after disk changes while keeping the tree expansion — only data updates
- Export: top-files list as CSV, currently expanded tree levels as text (UTF-8 with BOM, written to `Documents\MFT-Analysis\`)
- Reconciliation line: disk used / accounted / gap (NTFS metadata and reserved areas)

### How it works

1. Reads the boot sector for volume parameters, parses the `$DATA` runlist of `$MFT` itself, and reads the entire MFT in chunks (no "N consecutive empty records means end" guessing)
2. Parses `$FILE_NAME` / `$DATA` / `$INDEX_ALLOCATION` per record, applies the USA fixup, and merges sizes from extension records (pointed to by `$ATTRIBUTE_LIST`) back into the base record
3. Rebuilds the directory tree and post-order accumulates subtree sizes

### Size semantics (important)

The tool reports **physical disk usage** (actually allocated clusters), not the logical size of files:

- NTFS-compressed and sparse files count their real allocation
- Container layer placeholder files (WCIFS, no clusters allocated on this volume) count as 0
- Hard-linked files are counted once

Expect a typical ±1–2% difference versus the Explorer properties page ("size / size on disk"), caused by Explorer counting hard links once per path, disk activity during scanning, and how NTFS metadata ownership is attributed. The reconciliation line at the top shows "disk used / accounted / gap", where the gap is unattributed metadata and reserved areas.

### Requirements

- Windows 10 / 11, NTFS volume
- **Must run as administrator** (opening the raw volume `\\.\X:` requires that privilege)
- WebView2 runtime (built into Windows 10 1803+ / Windows 11)

### Use a prebuilt binary (no build needed)

[`mft-analysis-studio.exe`](./mft-analysis-studio.exe) in the repository root is the fully compiled program (single file, no installation):

1. Download it (Code → Download ZIP on the GitHub page, or fetch the file directly)
2. **Right-click → Run as administrator**
3. Pick a drive and click "Start Scan"

> The binary is not code-signed; Windows SmartScreen may warn about an "unknown publisher" on first run — choose "Run anyway".
> The exe in the repository root is updated with releases; build from source below if you need the latest.

### Building

Prerequisites: Rust toolchain, [Node.js](https://nodejs.org).

```bash
# 1. Build the frontend (output goes to dist/ in the repository root)
cd ui
npm install
npm run build

# 2. Build the backend (single portable executable)
cargo build --release
# Output: target/release/mft-analysis-studio.exe
```

Dev mode (hot reload):

```bash
cargo install tauri-cli --version "^2"   # once
cd ui && npm run dev                     # terminal 1: vite dev server
cargo tauri dev                          # terminal 2 (repository root)
```

### Usage

Pick a drive and click "Start Scan". The default MFT read limit is 8 GB (~8 million records, far beyond typical needs) and can be adjusted in the UI. After scanning, the left side shows the directory tree (sorted by usage, expandable) and the right side shows the top-files ranking. Use the buttons in the top-right corner to switch between Chinese/English and light/dark mode; the choices are persisted locally.

### Project layout

```
core/             mft-analysis-core: MFT scanning and parsing (pure Rust, no GUI deps)
studio/           mft-analysis-studio: Tauri GUI (Rust backend + window config + icons)
ui/               React frontend (Vite build, output to dist/ in the repository root)
```

### Safety

The program opens the volume **read-only and never writes any data**; however, the MFT is a live structure, so results are only a point-in-time snapshot and file churn during scanning introduces minor drift.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache License, Version 2.0](LICENSE-APACHE), at your option.

Copyright (c) 2026 David Niteas
