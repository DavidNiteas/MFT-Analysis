//! MFT Analysis —— 基于 MFT 的 Windows 磁盘占用分析工具（Tauri 后端）。
//! 需要以管理员身份运行（打开裸卷需要备份语义权限）。

#![windows_subsystem = "windows"]

use mft_analysis_core as mft;
use mft::Node;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, State};

const MFT_ROOT: u64 = 5; // $MFT 记录 5 = 根目录
const TOP_FILES_WALK_CAP: usize = 4_000_000; // 子树遍历文件数上限，防止极端情况卡死

#[derive(Serialize, Clone)]
struct DirDto {
    record: u64,
    name: String,
    /// 目录为子树占用，文件为自身大小
    subtree_size: u64,
    /// 仅根目录有意义：全部已扫描记录的大小之和（含父链断裂、不在目录树中的文件），
    /// 用于和卷已用空间对账。
    total_scanned: u64,
    is_dir: bool,
}

#[derive(Serialize)]
struct TopFilesDto {
    files: Vec<(String, u64)>,
    truncated: bool,
}

#[derive(Serialize, Clone)]
struct PieItem {
    name: String,
    size: u64,
}

#[derive(Serialize, Clone)]
struct PieDto {
    title: String,
    total: u64,
    items: Vec<PieItem>,
}

#[derive(Serialize, Clone)]
struct CrumbDto {
    record: u64,
    name: String,
}

#[derive(Serialize, Clone)]
struct ScanDoneDto {
    count: usize,
    drive: char,
    bytes_per_cluster: u64,
    record_size: u64,
    mft_byte_offset: u64,
    cancelled: bool,
}

#[derive(Serialize, Clone)]
struct VolumeStatsDto {
    total: u64,
    free: u64,
    used: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetDiskFreeSpaceExW(
        lpdir: *const u16,
        lpfree_avail: *mut u64,
        lptotal: *mut u64,
        lpfree: *mut u64,
    ) -> i32;
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
}

/// 已知文件夹 ID：文档（FOLDERID_Documents）
const FOLDERID_DOCUMENTS: GUID = GUID {
    data1: 0xFDD39AD0,
    data2: 0x238F,
    data3: 0x46AF,
    data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
};

#[repr(C)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(
        rfid: *const GUID,
        dw_flags: u32,
        h_token: isize,
        ppsz_path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(pv: *mut core::ffi::c_void);
}

struct AppState {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<mft::ScanProgress>>,
    /// 最近一次扫描结果；None 表示未扫描/已清空。
    nodes: Arc<RwLock<Option<Arc<HashMap<u64, Node>>>>>,
    drive: Arc<RwLock<char>>,
    scanning: Arc<AtomicBool>,
}

#[tauri::command]
fn scan_start(
    state: State<'_, AppState>,
    app: AppHandle,
    drive: char,
    limit_gb: f64,
) -> Result<(), String> {
    if state.scanning.load(Ordering::Relaxed) {
        return Err("已有扫描正在进行".into());
    }

    // 同步打开卷：权限不足 / 非 NTFS 在这里立即报错
    let handle = mft::open_volume(drive)?;
    let info = handle.info.clone();
    let limit = (limit_gb * 1024.0 * 1024.0 * 1024.0) as u64;

    state.cancel.store(false, Ordering::Relaxed);
    state.scanning.store(true, Ordering::Relaxed);
    *state.progress.lock().unwrap() = mft::ScanProgress {
        total_bytes: limit,
        ..Default::default()
    };
    *state.nodes.write().unwrap() = None;
    *state.drive.write().unwrap() = info.drive;

    let cancel = state.cancel.clone();
    let progress = state.progress.clone();
    let nodes_slot = state.nodes.clone();
    let scanning = state.scanning.clone();

    std::thread::spawn(move || {
        let result = mft::scan_mft(handle, limit, &cancel, &progress);
        scanning.store(false, Ordering::Relaxed);
        let cancelled = progress.lock().map(|p| p.cancelled).unwrap_or(false);
        match result {
            Ok(nodes) => {
                let count = nodes.len();
                *nodes_slot.write().unwrap() = Some(Arc::new(nodes));
                let dto = ScanDoneDto {
                    count,
                    drive: info.drive,
                    bytes_per_cluster: info.bytes_per_cluster,
                    record_size: info.record_size,
                    mft_byte_offset: info.mft_byte_offset,
                    cancelled,
                };
                let _ = app.emit("scan-done", dto);
            }
            Err(e) => {
                let _ = app.emit("scan-error", e);
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn scan_cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn scan_progress(state: State<'_, AppState>) -> mft::ScanProgress {
    state.progress.lock().unwrap().clone()
}

/// 卷的总量/空闲/已用（与资源管理器磁盘属性同一口径），用于和扫描结果对账。
#[tauri::command]
fn get_volume_stats(state: State<'_, AppState>) -> Result<VolumeStatsDto, String> {
    let drive = *state.drive.read().unwrap();
    let mut path: Vec<u16> = format!("{}:\\", drive).encode_utf16().collect();
    path.push(0);
    let mut free_avail = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free_avail,
            &mut total,
            &mut free,
        )
    };
    if ok == 0 {
        return Err("获取卷空间信息失败".into());
    }
    Ok(VolumeStatsDto {
        total,
        free,
        used: total.saturating_sub(free),
    })
}

#[tauri::command]
fn get_root(state: State<'_, AppState>) -> Result<DirDto, String> {
    let nodes = current_nodes(&state)?;
    let drive = *state.drive.read().unwrap();
    let size = nodes.get(&MFT_ROOT).map(|n| n.subtree_size).unwrap_or(0);
    let total_scanned: u64 = nodes.values().map(|n| n.self_size).sum();
    Ok(DirDto {
        record: MFT_ROOT,
        name: format!("{}:\\", drive),
        subtree_size: size,
        total_scanned,
        is_dir: true,
    })
}

/// 枚举本机可用盘符（固定/可移动/网络驱动器），供前端下拉选择。
#[tauri::command]
fn list_drives() -> Vec<String> {
    let mut out = vec![];
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{}:\\", letter).encode_utf16().chain(Some(0)).collect();
        let t = unsafe { GetDriveTypeW(root.as_ptr()) };
        // 2=可移动 3=固定 4=网络；跳过不存在的/光驱
        if (2..=4).contains(&t) {
            out.push(letter.to_string());
        }
    }
    out
}

/// 某目录下的子项（按大小降序）。`include_files` 为 true 时同时返回文件。
#[tauri::command]
fn get_children(
    state: State<'_, AppState>,
    record: u64,
    include_files: bool,
) -> Result<Vec<DirDto>, String> {
    let nodes = current_nodes(&state)?;
    let node = nodes
        .get(&record)
        .ok_or_else(|| format!("记录 {} 不存在", record))?;
    let mut dirs: Vec<DirDto> = node
        .children
        .iter()
        .filter_map(|&c| nodes.get(&c).map(|n| (c, n)))
        .filter(|(_, n)| n.is_dir || include_files)
        .map(|(c, n)| DirDto {
            record: c,
            name: n.name.clone(),
            subtree_size: if n.is_dir { n.subtree_size } else { n.self_size },
            total_scanned: 0,
            is_dir: n.is_dir,
        })
        .collect();
    dirs.sort_by(|a, b| b.subtree_size.cmp(&a.subtree_size));
    Ok(dirs)
}

#[tauri::command]
fn get_top_files(state: State<'_, AppState>, record: u64) -> Result<TopFilesDto, String> {
    let nodes = current_nodes(&state)?;
    let drive = *state.drive.read().unwrap();
    if !nodes.contains_key(&record) {
        return Err(format!("记录 {} 不存在", record));
    }

    let mut visited: HashSet<u64> = HashSet::new();
    let mut stack = vec![record];
    let mut files: Vec<(u64, u64)> = Vec::new();
    let mut truncated = false;

    while let Some(r) = stack.pop() {
        if !visited.insert(r) {
            continue;
        }
        if let Some(n) = nodes.get(&r) {
            if !n.is_dir && n.self_size > 0 {
                files.push((r, n.self_size));
                if files.len() >= TOP_FILES_WALK_CAP {
                    truncated = true;
                    break;
                }
            }
            for &c in &n.children {
                stack.push(c);
            }
        }
    }

    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(500);
    let list: Vec<(String, u64)> = files
        .iter()
        .map(|&(r, s)| (get_path(&nodes, r, drive), s))
        .collect();
    Ok(TopFilesDto {
        files: list,
        truncated,
    })
}

const PIE_MAX_SLICES: usize = 25;

/// 目录当前层级的空间占用饼图：每个直接子目录一项，直属文件聚合为一项，
/// 其余小项并入「其他」。仅返回 size > 0 的项。
#[tauri::command]
fn get_dir_pie(state: State<'_, AppState>, record: u64) -> Result<PieDto, String> {
    let nodes = current_nodes(&state)?;
    let drive = *state.drive.read().unwrap();
    let node = nodes
        .get(&record)
        .ok_or_else(|| format!("记录 {} 不存在", record))?;

    let mut items: Vec<PieItem> = Vec::new();
    let mut files_total = 0u64;
    for &c in &node.children {
        if let Some(n) = nodes.get(&c) {
            if n.is_dir {
                if n.subtree_size > 0 {
                    items.push(PieItem {
                        name: n.name.clone(),
                        size: n.subtree_size,
                    });
                }
            } else {
                files_total += n.self_size;
            }
        }
    }
    if files_total > 0 {
        items.push(PieItem {
            name: "[本目录内文件]".into(),
            size: files_total,
        });
    }
    Ok(PieDto {
        title: get_path(&nodes, record, drive),
        total: items.iter().map(|i| i.size).sum(),
        items: top_k_other(items, PIE_MAX_SLICES),
    })
}

/// 子树文件饼图：`by_ext` 时按小写后缀聚合（无后缀归入「[无后缀]」），
/// 否则按单个文件；均只保留 top-k，其余并入「其他」。k 限制在 3..=100。
#[tauri::command]
fn get_files_pie(
    state: State<'_, AppState>,
    record: u64,
    by_ext: bool,
    k: usize,
) -> Result<PieDto, String> {
    let nodes = current_nodes(&state)?;
    let drive = *state.drive.read().unwrap();
    if !nodes.contains_key(&record) {
        return Err(format!("记录 {} 不存在", record));
    }
    let k = k.clamp(3, 100);

    let mut visited: HashSet<u64> = HashSet::new();
    let mut stack = vec![record];
    let mut items: Vec<PieItem> = Vec::new();
    // by_ext 时用 HashMap 聚合，键为后缀
    let mut ext_agg: HashMap<String, u64> = HashMap::new();

    while let Some(r) = stack.pop() {
        if !visited.insert(r) {
            continue;
        }
        if let Some(n) = nodes.get(&r) {
            if !n.is_dir && n.self_size > 0 {
                if by_ext {
                    let ext = match n.name.rfind('.') {
                        Some(i) if i > 0 && i + 1 < n.name.len() => {
                            n.name[i + 1..].to_lowercase()
                        }
                        _ => String::new(),
                    };
                    *ext_agg.entry(ext).or_insert(0) += n.self_size;
                } else {
                    items.push(PieItem {
                        name: n.name.clone(),
                        size: n.self_size,
                    });
                }
                if items.len() >= TOP_FILES_WALK_CAP {
                    break;
                }
            }
            for &c in &n.children {
                stack.push(c);
            }
        }
    }
    if by_ext {
        items = ext_agg
            .into_iter()
            .map(|(ext, size)| PieItem {
                name: if ext.is_empty() {
                    "[无后缀]".into()
                } else {
                    format!(".{}", ext)
                },
                size,
            })
            .collect();
    }
    Ok(PieDto {
        title: get_path(&nodes, record, drive),
        total: items.iter().map(|i| i.size).sum(),
        items: top_k_other(items, k),
    })
}

/// 降序排列并裁剪：保留 top-k，其余合并为「其他」项；全零时返回空。
fn top_k_other(mut items: Vec<PieItem>, k: usize) -> Vec<PieItem> {
    items.sort_by(|a, b| b.size.cmp(&a.size));
    items.retain(|i| i.size > 0);
    if items.len() > k {
        let rest: u64 = items[k..].iter().map(|i| i.size).sum();
        items.truncate(k);
        items.push(PieItem {
            name: "其他".into(),
            size: rest,
        });
    }
    items
}

/// 从根目录到指定记录的面包屑链。
#[tauri::command]
fn get_chain(state: State<'_, AppState>, record: u64) -> Result<Vec<CrumbDto>, String> {
    let nodes = current_nodes(&state)?;
    let drive = *state.drive.read().unwrap();
    let mut chain = vec![];
    let mut cur = record;
    for _ in 0..64 {
        if cur == MFT_ROOT {
            chain.push(CrumbDto {
                record: MFT_ROOT,
                name: format!("{}:\\", drive),
            });
            break;
        }
        match nodes.get(&cur) {
            Some(n) if n.parent != cur => {
                chain.push(CrumbDto {
                    record: cur,
                    name: n.name.clone(),
                });
                cur = n.parent;
            }
            _ => break,
        }
    }
    chain.reverse();
    Ok(chain)
}

fn current_nodes(state: &State<'_, AppState>) -> Result<Arc<HashMap<u64, Node>>, String> {
    state
        .nodes
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| "尚未完成扫描".to_string())
}

/// 导出文本内容到「文档\MFT-Analysis\」下（按 UTF-8 带 BOM，Excel/记事本均友好），
/// 返回写出的完整路径，供调用方在资源管理器中定位。
#[tauri::command]
fn export_text(suggested_name: String, content: String) -> Result<String, String> {
    let dir = documents_dir()?.join("MFT-Analysis");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {}", e))?;
    // 防止路径分隔符逃逸导出目录
    let safe_name: String = suggested_name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' { '_' } else { c })
        .collect();
    let path = dir.join(safe_name);
    let mut data = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
    data.extend_from_slice(content.as_bytes());
    std::fs::write(&path, &data).map_err(|e| format!("写入导出文件失败: {}", e))?;
    Ok(path.to_string_lossy().into_owned())
}

fn documents_dir() -> Result<std::path::PathBuf, String> {
    unsafe {
        let mut raw: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_DOCUMENTS, 0, 0, &mut raw);
        if hr == 0 && !raw.is_null() {
            let mut len = 0usize;
            while *raw.add(len) != 0 {
                len += 1;
            }
            let path = std::ffi::OsString::from_wide(std::slice::from_raw_parts(raw, len));
            CoTaskMemFree(raw as *mut core::ffi::c_void);
            return Ok(path.into());
        }
        if !raw.is_null() {
            CoTaskMemFree(raw as *mut core::ffi::c_void);
        }
    }
    // 兜底：%USERPROFILE%\Documents
    let home =
        std::env::var_os("USERPROFILE").ok_or_else(|| "无法确定文档目录".to_string())?;
    Ok(std::path::PathBuf::from(home).join("Documents"))
}

/// 在资源管理器中打开所在目录并选中该项。
#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    validate_explorer_path(&path)?;
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn()
        .map_err(|e| format!("打开资源管理器失败: {}", e))?;
    Ok(())
}

/// 在资源管理器中打开该目录本身。
#[tauri::command]
fn open_dir(path: String) -> Result<(), String> {
    validate_explorer_path(&path)?;
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("打开资源管理器失败: {}", e))?;
    Ok(())
}

/// 仅允许从扫描结果得到的真实路径；孤儿记录（<orphan-N>）没有对应文件系统项。
fn validate_explorer_path(path: &str) -> Result<(), String> {
    if path.starts_with('<') {
        return Err("该记录没有有效路径（父链断裂的孤儿记录）".into());
    }
    Ok(())
}

/// 回溯生成完整路径。
fn get_path(nodes: &HashMap<u64, Node>, record: u64, drive: char) -> String {
    if record == MFT_ROOT {
        return format!("{}:\\", drive);
    }
    let mut parts = vec![];
    let mut cur = record;
    for _ in 0..64 {
        match nodes.get(&cur) {
            Some(node) if cur != MFT_ROOT && !node.name.is_empty() => {
                parts.push(node.name.clone());
                if node.parent == cur {
                    break;
                }
                cur = node.parent;
            }
            _ => break,
        }
    }
    parts.reverse();
    if parts.is_empty() {
        format!("<orphan-{}>", record)
    } else {
        format!("{}:\\{}", drive, parts.join("\\"))
    }
}

fn main() {
    let state = AppState {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(Mutex::new(mft::ScanProgress::default())),
        nodes: Arc::new(RwLock::new(None)),
        drive: Arc::new(RwLock::new('C')),
        scanning: Arc::new(AtomicBool::new(false)),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            scan_start,
            scan_cancel,
            scan_progress,
            get_root,
            get_children,
            get_top_files,
            get_chain,
            get_volume_stats,
            get_dir_pie,
            get_files_pie,
            reveal_in_explorer,
            open_dir,
            list_drives,
            export_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
