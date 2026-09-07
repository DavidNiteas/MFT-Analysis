//! MFT Reader —— 基于 MFT 的 Windows 磁盘占用分析工具（Tauri 后端）。
//! 需要以管理员身份运行（打开裸卷需要备份语义权限）。

#![windows_subsystem = "windows"]

mod mft;

use mft::Node;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter, State};

const MFT_ROOT: u64 = 5; // $MFT 记录 5 = 根目录
const TOP_FILES_WALK_CAP: usize = 4_000_000; // 子树遍历文件数上限，防止极端情况卡死

#[derive(Serialize, Clone)]
struct DirDto {
    record: u64,
    name: String,
    subtree_size: u64,
    /// 仅根目录有意义：全部已扫描记录的大小之和（含父链断裂、不在目录树中的文件），
    /// 用于和卷已用空间对账。
    total_scanned: u64,
}

#[derive(Serialize)]
struct TopFilesDto {
    files: Vec<(String, u64)>,
    truncated: bool,
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
    })
}

/// 某目录下的子目录（按子树占用降序）。
#[tauri::command]
fn get_children(state: State<'_, AppState>, record: u64) -> Result<Vec<DirDto>, String> {
    let nodes = current_nodes(&state)?;
    let node = nodes
        .get(&record)
        .ok_or_else(|| format!("记录 {} 不存在", record))?;
    let mut dirs: Vec<DirDto> = node
        .children
        .iter()
        .filter_map(|&c| nodes.get(&c).map(|n| (c, n)))
        .filter(|(_, n)| n.is_dir)
        .map(|(c, n)| DirDto {
            record: c,
            name: n.name.clone(),
            subtree_size: n.subtree_size,
            total_scanned: 0,
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
            get_volume_stats
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
