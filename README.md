# MFT Reader — 磁盘占用分析

直接读取 NTFS 的 MFT（Master File Table），几秒内统计出整个卷的磁盘占用分布。不递归遍历文件系统，因此**快得多**，也不受文件占用、权限（对目录树的）影响。

## 特点

- **快**：扫描约 150 万条 MFT 记录仅需数秒（SSD），没有逐文件的 API 遍历
- **准**：按 runlist 实际分配的簇计算物理占用，对 NTFS 压缩、稀疏文件、容器镜像层均准确；全卷总量与 `fsutil` 对账偏差 < 1%
- **全**：包含资源管理器看不到或进不去的部分（`System Volume Information`、`$Extend`、休眠文件等系统元数据）
- **GUI**：Tauri 2 + React，目录树 + Top 文件排行，中文界面

## 工作原理

1. 读取 Boot Sector 获取卷参数，解析 `$MFT` 自身的 `$DATA` runlist，分段读完整个 MFT（不依赖"连续 N 条空记录即结束"的猜测）
2. 逐记录解析 `$FILE_NAME` / `$DATA` / `$INDEX_ALLOCATION`，应用 USA fixup，并把扩展记录（`$ATTRIBUTE_LIST` 指向的）的大小合并回基础记录
3. 重建目录树，后序遍历累加每个目录的子树大小

### 大小口径（重要）

本工具统计的是**物理磁盘占用**（实际分配的簇），不是文件的逻辑大小：

- NTFS 压缩、稀疏文件按真实占用计算
- 容器镜像层占位文件（WCIFS，未在本卷分配簇）计 0
- 硬链接文件只计一次

因此与资源管理器属性页（"大小 / 占用空间"）对比时，典型差异 ±1~2%，来源包括：硬链接 Explorer 按路径重复统计、扫描期间的磁盘活动、NTFS 元数据本身的归属口径。界面顶部提供「磁盘已用 / 已统计 / 差额」对账行，差额即未归属的元数据与保留区。

## 使用要求

- Windows 10 / 11，NTFS 卷
- **必须以管理员身份运行**（读取裸卷 `\\.\X:` 需要相应权限）
- WebView2 运行时（Windows 10 1803+ / Windows 11 自带）

## 构建

前置依赖：Rust toolchain、[Node.js](https://nodejs.org)。

```bash
# 1. 构建前端（产物输出到仓库根目录 dist/）
cd ui
npm install
npm run build

# 2. 构建后端（单文件可执行程序，无需安装）
cargo build --release
# 产物：target/release/mft-reader.exe
```

开发模式（热重载）：

```bash
cargo install tauri-cli --version "^2"   # 仅需一次
cd ui && npm run dev                     # 终端 1：vite dev server
cargo tauri dev                          # 终端 2
```

## 使用

启动后选择盘符，点击「开始扫描」。默认 MFT 读取上限 8 GB（约 800 万条记录，远超实际需求），可在界面调整；扫描完成后左侧为目录树（按占用排序，可展开），右侧为全卷 Top 文件排行。

## 项目结构

```
src/mft.rs        MFT 扫描与解析核心（纯 Rust，无 GUI 依赖）
src/main.rs       Tauri 入口与命令
ui/               React 前端（Vite 构建）
scripts/          辅助脚本（图标生成、GUI 自动化验证）
```

## 安全说明

程序以只读方式打开卷，**不会写入任何数据**；但 MFT 是活动卷的内部结构，扫描结果只是某一时刻的快照，且运行期间文件增删会带来轻微误差。

## License

MIT
