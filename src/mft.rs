//! MFT 扫描核心：打开卷、读取并解析 MFT 记录、重建目录树、计算子树大小。
//! 与原命令行版本逻辑一致，但改为返回数据，并支持进度上报与取消。

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub parent: u64,
    pub name: String,
    pub self_size: u64,
    pub subtree_size: u64,
    pub is_dir: bool,
    pub children: Vec<u64>,
    /// 诊断：$FILE_NAME 上报的分配大小（0 = 未记录）
    pub dbg_fn_alloc: u64,
    /// 诊断：$FILE_NAME 上报的逻辑大小
    pub dbg_fn_real: u64,
    /// 诊断：$DATA 头上报的分配大小合计（0 = 无非驻留 $DATA）
    pub dbg_data_alloc: u64,
    /// 诊断：记录中 $FILE_NAME 属性个数（≥2 表示硬链接，Explorer 会按路径重复计）
    pub dbg_fn_count: u64,
    /// 诊断：扩展记录合并进来的 runlist 字节数
    pub dbg_ext_runs: u64,
    /// 诊断：基础+扩展记录 runlist 实际分配字节数（0 = 走了大小字段回退）
    pub dbg_runs: u64,
    /// 诊断：是否见过非驻留 $DATA
    pub dbg_saw_nonresident: bool,
    /// 诊断：runlist 是否截断/损坏
    pub dbg_truncated: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ScanProgress {
    pub records: u64,
    pub valid: u64,
    pub bytes_scanned: u64,
    pub total_bytes: u64,
    pub finished: bool,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct VolumeInfo {
    pub drive: char,
    pub bytes_per_cluster: u64,
    pub record_size: u64,
    pub mft_byte_offset: u64,
}

/// 已打开的卷句柄（含解析出的卷参数），用于在扫描线程间传递。
pub struct VolumeHandle {
    pub file: File,
    pub info: VolumeInfo,
}

/// 打开卷并解析 Boot Sector。同步返回错误（权限不足/非 NTFS 等），
/// 便于调用方在发起扫描前立即提示。
pub fn open_volume(drive: char) -> Result<VolumeHandle, String> {
    let drive = drive.to_ascii_uppercase();
    let volume_path = format!(r"\\.\{}:", drive);

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&volume_path)
        .map_err(|e| format!("无法打开卷 {}（请以管理员身份运行）: {}", volume_path, e))?;

    // ── 1. Boot Sector ──
    let mut boot = [0u8; 512];
    file.read_exact(&mut boot)
        .map_err(|e| format!("读取 Boot Sector 失败: {}", e))?;
    if &boot[0x03..0x07] != b"NTFS" {
        return Err("不是 NTFS 卷".into());
    }

    let bytes_per_sector = u16::from_le_bytes([boot[0x0B], boot[0x0C]]);
    let sectors_per_cluster = boot[0x0D];
    let mft_start_lcn = u64::from_le_bytes(boot[0x30..0x38].try_into().unwrap());
    let mft_record_clusters = boot[0x40] as i8;

    let bytes_per_cluster = bytes_per_sector as u64 * sectors_per_cluster as u64;
    let record_size: u64 = if mft_record_clusters > 0 {
        mft_record_clusters as u64 * bytes_per_cluster
    } else {
        1u64 << (-mft_record_clusters as u64)
    };
    let mft_byte_offset = mft_start_lcn * bytes_per_cluster;

    Ok(VolumeHandle {
        file,
        info: VolumeInfo {
            drive,
            bytes_per_cluster,
            record_size,
            mft_byte_offset,
        },
    })
}

/// 扫描指定卷的 MFT。`cancel` 置位时提前返回（返回已扫描部分）。
/// 进度通过 `progress` 定期上报。
pub fn scan_mft(
    handle: VolumeHandle,
    limit: u64,
    cancel: &AtomicBool,
    progress: &Mutex<ScanProgress>,
) -> Result<HashMap<u64, Node>, String> {
    let VolumeHandle {
        mut file,
        info,
    } = handle;
    let mft_byte_offset = info.mft_byte_offset;
    let record_size = info.record_size;
    let bytes_per_cluster = info.bytes_per_cluster;

    // ── 2. 读取 $MFT 记录 0，解析 $DATA 运行列表 ──
    // $MFT 可能是分段存储的，只有按运行列表逐段读取才能覆盖全部记录。
    let mut rec0 = vec![0u8; record_size as usize];
    file.seek(SeekFrom::Start(mft_byte_offset))
        .map_err(|e| format!("定位 MFT 失败: {}", e))?;
    file.read_exact(&mut rec0)
        .map_err(|e| format!("读取 $MFT 记录 0 失败: {}", e))?;
    let runs = parse_mft_data_runs(&mut rec0, record_size as usize)
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| {
            // 兜底：解析失败则按 Boot Sector 起始 LCN 顺序读 limit 大小
            let start_lcn = mft_byte_offset / bytes_per_cluster;
            let clusters = limit.div_ceil(bytes_per_cluster).max(1);
            vec![(start_lcn, clusters)]
        });

    // ── 3. 按段读取 MFT ──
    const CHUNK: usize = 1024 * 1024;
    let mut chunk_buf = vec![0u8; CHUNK];
    let mut nodes: HashMap<u64, Node> = HashMap::new();
    let mut sizes_map: HashMap<u64, SizeParts> = HashMap::new();
    let mut ext_map: HashMap<u64, SizeParts> = HashMap::new();
    let mut parent_map: HashMap<u64, Vec<u64>> = HashMap::new();

    let total_run_bytes: u64 = runs.iter().map(|(_, c)| c * bytes_per_cluster).sum();
    let total_plan = total_run_bytes.min(limit);

    let mut mft_data_pos: u64 = 0; // MFT 数据流内的全局偏移（跨段连续，用于记录编号）
    let mut bytes_scanned = 0u64;
    let mut stopped = false;
    let mut record_num: u64 = 0;

    'outer: for &(lcn, clusters) in &runs {
        if bytes_scanned >= limit {
            break;
        }
        let run_bytes = clusters * bytes_per_cluster;
        let mut run_pos = 0u64;
        if file
            .seek(SeekFrom::Start(lcn * bytes_per_cluster))
            .is_err()
        {
            continue;
        }
        while run_pos < run_bytes {
            if cancel.load(Ordering::Relaxed) {
                stopped = true;
                break 'outer;
            }
            let to_read = std::cmp::min(
                std::cmp::min(CHUNK as u64, run_bytes - run_pos),
                limit - bytes_scanned,
            ) as usize;
            if to_read == 0 {
                break 'outer;
            }
            if let Err(e) = file.read_exact(&mut chunk_buf[..to_read]) {
                eprintln!("读取 MFT 分段失败 (LCN {}): {}", lcn, e);
                break; // 放弃本段，继续下一段
            }

            let mut offset = 0usize;
            while offset + record_size as usize <= to_read {
                record_num = (mft_data_pos + offset as u64) / record_size;
                let rec_slice =
                    &mut chunk_buf[offset..offset + record_size as usize];
                match parse_record(rec_slice, record_size as usize, record_num, bytes_per_cluster) {
                    RecOutcome::Node(node, sizes) => {
                        // 根目录记录（5）没有上级，parent 置为自身；
                        // 跳过父引用自环的记录（根目录的 "." / ".." 名，parent 指向自身），
                        // 否则根目录会把自己挂为自己的子节点，形成无限链。
                        let mut node = node;
                        if record_num == 5 {
                            node.parent = 5;
                        }
                        if node.parent != record_num {
                            parent_map.entry(node.parent).or_default().push(record_num);
                        }
                        nodes.insert(record_num, node);
                        sizes_map.insert(record_num, sizes);
                    }
                    RecOutcome::Ext { base, sizes } => {
                        // 扩展记录：把携带的 $DATA/$INDEX_ALLOCATION 大小合并回基础记录
                        let e = ext_map.entry(base).or_default();
                        e.runs = e.runs.saturating_add(sizes.runs);
                        e.dalloc = e.dalloc.saturating_add(sizes.dalloc);
                        e.dreal = e.dreal.saturating_add(sizes.dreal);
                        if e.fn_alloc == 0 {
                            e.fn_alloc = sizes.fn_alloc;
                            e.fn_real = sizes.fn_real;
                        }
                    }
                    RecOutcome::None => {}
                }
                offset += record_size as usize;
            }

            mft_data_pos += to_read as u64;
            run_pos += to_read as u64;
            bytes_scanned += to_read as u64;

            if let Ok(mut p) = progress.lock() {
                p.records = record_num + 1;
                p.valid = nodes.len() as u64;
                p.bytes_scanned = bytes_scanned;
                p.total_bytes = total_plan;
                p.cancelled = stopped;
            }
        }
    }

    // ── 3.5 合并扩展记录大小，计算每条记录的 self_size ──
    for (rec, node) in nodes.iter_mut() {
        let mut sizes = sizes_map.remove(rec).unwrap_or_default();
        if let Some(e) = ext_map.get(rec) {
            sizes.runs = sizes.runs.saturating_add(e.runs);
            sizes.dalloc = sizes.dalloc.saturating_add(e.dalloc);
            sizes.dreal = sizes.dreal.saturating_add(e.dreal);
            sizes.saw_nonresident |= e.saw_nonresident;
            sizes.runs_truncated |= e.runs_truncated;
            node.dbg_ext_runs = e.runs;
            if sizes.fn_alloc == 0 {
                sizes.fn_alloc = e.fn_alloc;
                sizes.fn_real = e.fn_real;
            }
        }
        node.dbg_runs = sizes.runs;
        node.dbg_saw_nonresident = sizes.saw_nonresident;
        node.dbg_truncated = sizes.runs_truncated;
        node.self_size = finalize_size(&sizes);
        node.dbg_data_alloc = sizes.dalloc;
        // $BadClus（固定记录 8）的 $Bad 流是覆盖全卷的稀疏占位，
        // 其大小字段等于卷容量但并未真实占用簇，必须排除。
        if *rec == 8 {
            node.self_size = 0;
        }
    }

    // ── 4. 重建父子关系 ──
    let updates: Vec<(u64, Vec<u64>)> = parent_map
        .iter()
        .filter(|(k, _)| nodes.contains_key(k))
        .map(|(&k, v)| (k, v.clone()))
        .collect();
    for (rec, kids) in updates {
        nodes.get_mut(&rec).unwrap().children = kids;
    }

    // ── 5. 计算子树大小（迭代后序，防深层递归溢出）──
    compute_subtree_sizes(&mut nodes);

    if let Ok(mut p) = progress.lock() {
        p.finished = true;
        p.records = record_num + 1;
        p.valid = nodes.len() as u64;
        p.bytes_scanned = bytes_scanned;
        p.total_bytes = total_plan;
        p.cancelled = stopped;
    }

    Ok(nodes)
}

fn read_uint_le(buf: &[u8], pos: usize, size: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..size.min(8) {
        v |= (buf[pos + i] as u64) << (8 * i);
    }
    v
}

/// 读取 1-8 字节的小端有符号整数（按最高位符号扩展）。
fn read_int_le(buf: &[u8], pos: usize, size: usize) -> i64 {
    let v = read_uint_le(buf, pos, size);
    if size > 0 && size < 8 && (buf[pos + size - 1] & 0x80) != 0 {
        (v | (u64::MAX << (8 * size))) as i64
    } else {
        v as i64
    }
}

/// 解析 $MFT 记录 0 的非驻留 $DATA 运行列表，返回 (LCN, 簇数) 列表（VCN 顺序）。
/// 失败返回 None，调用方回退到顺序读取。
pub(crate) fn parse_mft_data_runs(buf: &mut [u8], record_size: usize) -> Option<Vec<(u64, u64)>> {
    if !apply_fixups(buf, record_size) {
        return None;
    }
    let mut attr_offset = u16::from_le_bytes([buf[0x14], buf[0x15]]) as usize;
    loop {
        if attr_offset + 0x22 > record_size {
            break;
        }
        let attr_type = u32::from_le_bytes(buf[attr_offset..attr_offset + 4].try_into().ok()?);
        if attr_type == 0xFFFFFFFF || attr_type == 0 {
            break;
        }
        let attr_len =
            u32::from_le_bytes(buf[attr_offset + 4..attr_offset + 8].try_into().ok()?) as usize;
        if attr_len == 0 || attr_offset + attr_len > record_size {
            break;
        }
        if attr_type == 0x80 && buf[attr_offset + 0x08] == 1 {
            // 非驻留 $DATA：运行列表偏移在属性头 0x20 处
            let runlist_rel =
                u16::from_le_bytes([buf[attr_offset + 0x20], buf[attr_offset + 0x21]]) as usize;
            let r = attr_offset + runlist_rel;
            let end = attr_offset + attr_len;
            if r < end {
                return Some(parse_runlist(&buf[r..end]).0);
            }
            return None;
        }
        attr_offset += attr_len;
    }
    None
}

/// 解码 NTFS 运行列表（runlist）。
/// 返回 (段列表, 是否以 0 终止符正常结束)。未正常结束说明数据被截断或损坏，
/// 调用方不应轻信"没有段"的结论。
fn parse_runlist(buf: &[u8]) -> (Vec<(u64, u64)>, bool) {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    let mut complete = false;
    while pos < buf.len() {
        let hdr = buf[pos];
        if hdr == 0 {
            complete = true;
            break;
        }
        let len_size = (hdr & 0x0F) as usize;
        let off_size = (hdr >> 4) as usize;
        pos += 1;
        if len_size == 0 || off_size > 8 || pos + len_size + off_size > buf.len() {
            break;
        }
        let len = read_uint_le(buf, pos, len_size);
        let off = read_int_le(buf, pos + len_size, off_size);
        pos += len_size + off_size;
        lcn += off;
        // off == 0 是稀疏段（压缩文件的压缩单元洞、稀疏文件的空洞），不占簇，跳过
        if len > 0 && off != 0 && lcn > 0 {
            runs.push((lcn as u64, len));
        }
    }
    (runs, complete)
}

/// 对单条 MFT 记录应用 USA Fixup，恢复被 NTFS 替换的扇区尾。
/// 返回记录签名是否有效。
fn apply_fixups(buf: &mut [u8], record_size: usize) -> bool {
    if buf.len() < 4 || (&buf[0..4] != b"FILE" && &buf[0..4] != b"BAAD") {
        return false;
    }
    let usa_offset = u16::from_le_bytes([buf[0x04], buf[0x05]]) as usize;
    let usa_size = u16::from_le_bytes([buf[0x06], buf[0x07]]) as usize;
    if usa_offset >= 0x30 && usa_size > 1 && usa_offset + usa_size * 2 <= record_size {
        let usn = u16::from_le_bytes([buf[usa_offset], buf[usa_offset + 1]]);
        let sectors = record_size / 512;
        for i in 0..sectors.min(usa_size - 1) {
            let pos = (i + 1) * 512 - 2;
            if u16::from_le_bytes([buf[pos], buf[pos + 1]]) == usn {
                let fix = usa_offset + 2 + i * 2;
                buf[pos] = buf[fix];
                buf[pos + 1] = buf[fix + 1];
            }
        }
    }
    true
}

fn compute_subtree_sizes(nodes: &mut HashMap<u64, Node>) {
    let keys: Vec<u64> = nodes.keys().copied().collect();
    let mut memo: HashMap<u64, u64> = HashMap::new();

    for &k in &keys {
        if memo.contains_key(&k) {
            continue;
        }
        let mut on_path: HashSet<u64> = HashSet::new();
        let mut stack = vec![(k, false)];
        while let Some((rec, processed)) = stack.pop() {
            if memo.contains_key(&rec) {
                continue;
            }
            if processed {
                on_path.remove(&rec);
                let node = &nodes[&rec];
                // 目录的 self_size 是其索引分配（$INDEX_ALLOCATION 的 runlist），一并计入
                let mut total = node.self_size;
                for &c in &node.children {
                    if c != rec {
                        total += memo.get(&c).copied().unwrap_or(0);
                    }
                }
                memo.insert(rec, total);
            } else {
                on_path.insert(rec);
                stack.push((rec, true));
                for &c in &nodes[&rec].children {
                    if c != rec && !memo.contains_key(&c) && !on_path.contains(&c) {
                        stack.push((c, false));
                    }
                }
            }
        }
    }

    for (rec, n) in nodes.iter_mut() {
        n.subtree_size = memo.get(rec).copied().unwrap_or(0);
    }
}

/// 记录中解析出的大小信息（基础记录 + 扩展记录合并后得出 self_size）。
#[derive(Default, Clone)]
struct SizeParts {
    /// runlist 实际分配的簇换算成字节（唯一对压缩/稀疏文件也准确的口径）
    runs: u64,
    /// $DATA 头分配大小合计（回退用）
    dalloc: u64,
    /// $DATA 头逻辑大小合计（回退用）
    dreal: u64,
    /// $FILE_NAME 分配大小（回退用）
    fn_alloc: u64,
    /// $FILE_NAME 逻辑大小（回退用）
    fn_real: u64,
    /// 见过非驻留 $DATA（runlist 是权威的分配描述）
    saw_nonresident: bool,
    /// 存在未能正常终止（截断/损坏）的 runlist，此时"无分配段"不可信
    runs_truncated: bool,
}

enum RecOutcome {
    /// 基础记录：节点 + 大小信息
    Node(Node, SizeParts),
    /// 扩展记录：基础记录号 + 其携带的 $DATA/$INDEX_ALLOCATION 大小
    Ext { base: u64, sizes: SizeParts },
    /// 空闲/损坏记录
    None,
}

fn read_u32le(buf: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap_or([0u8; 4]))
}

/// 遍历记录中的 $DATA（0x80）与 $INDEX_ALLOCATION（0xA0）属性，
/// 累加 runlist 实际分配簇数和大小字段。供基础记录与扩展记录共用。
fn accumulate_data_sizes(buf: &[u8], record_size: usize, bytes_per_cluster: u64) -> SizeParts {
    let mut s = SizeParts::default();
    let mut attr_offset = u16::from_le_bytes([buf[0x14], buf[0x15]]) as usize;
    loop {
        if attr_offset + 8 > record_size {
            break;
        }
        let t = read_u32le(buf, attr_offset);
        if t == 0xFFFFFFFF || t == 0 {
            break;
        }
        let alen = read_u32le(buf, attr_offset + 4) as usize;
        if alen == 0 || attr_offset + alen > record_size {
            break;
        }
        if (t == 0x80 || t == 0xA0) && attr_offset + 0x22 <= record_size {
            if buf[attr_offset + 0x08] == 1 {
                // 非驻留头：0x28 = 分配大小，0x30 = 逻辑大小，0x20 = runlist 偏移
                s.saw_nonresident = true;
                if attr_offset + 0x38 <= record_size {
                    s.dalloc = s.dalloc.saturating_add(u64::from_le_bytes(
                        buf[attr_offset + 0x28..attr_offset + 0x30]
                            .try_into()
                            .unwrap_or([0u8; 8]),
                    ));
                    s.dreal = s.dreal.saturating_add(u64::from_le_bytes(
                        buf[attr_offset + 0x30..attr_offset + 0x38]
                            .try_into()
                            .unwrap_or([0u8; 8]),
                    ));
                    let runlist_rel =
                        u16::from_le_bytes([buf[attr_offset + 0x20], buf[attr_offset + 0x21]])
                            as usize;
                    let r = attr_offset + runlist_rel;
                    let end = attr_offset + alen;
                    if r < end && r < record_size {
                        let (parsed, complete) = parse_runlist(&buf[r..end]);
                        if !complete {
                            s.runs_truncated = true;
                        }
                        for (_, len) in parsed {
                            s.runs = s.runs.saturating_add(len * bytes_per_cluster);
                        }
                    }
                }
            } else if attr_offset + 0x14 <= record_size {
                // 驻留：0x10(u32) = 内容长度
                let len = u32::from_le_bytes(
                    buf[attr_offset + 0x10..attr_offset + 0x14]
                        .try_into()
                        .unwrap(),
                ) as u64;
                // 内容必须装得进属性体。WCIFS 容器层占位文件的 $DATA 会把
                // 整个文件大小填进内容长度字段（远超属性实际容量），信了会虚增——
                // 这类文件实际未分配簇（queryextents 显示 LCN=-1），应计 0。
                if len <= (alen as u64).saturating_sub(0x18) {
                    s.dreal = s.dreal.saturating_add(len);
                    s.dalloc = s.dalloc.saturating_add(len);
                }
            }
        }
        attr_offset += alen;
    }
    s
}

/// self_size 语义 = 磁盘占用（分配大小）。
/// 优先 runlist 实际分配的簇；runlist 完整但无分配段（全稀疏/容器层未分配占位）
/// 即为 0——大小字段在此不可信（容器 Layers 文件曾因此虚增 15GB）。
/// runlist 缺失或截断时退回大小字段，兼容 hiberfil.sys / pagefile.sys 等特殊情况。
fn finalize_size(s: &SizeParts) -> u64 {
    if s.runs > 0 {
        return s.runs;
    }
    if s.saw_nonresident && !s.runs_truncated {
        return 0;
    }
    // $FILE_NAME 分配大小为 0（且 runlist 无分配）：真实文件必有非零的
    // $FILE_NAME 大小；为 0 说明是 WCIFS 容器层占位记录，其 $DATA 大小字段
    // 指向未分配簇（queryextents 显示 LCN=-1），按 0 计。
    if s.fn_alloc == 0 && !s.runs_truncated {
        return 0;
    }
    s.fn_alloc.max(s.dalloc).max(if s.fn_alloc == 0 && s.dalloc == 0 {
        s.fn_real.max(s.dreal)
    } else {
        0
    })
}

/// 解析单条 MFT 记录。
/// 基础记录返回节点 + 大小信息；扩展记录（Base Record != 0）返回其携带的
/// $DATA/$INDEX_ALLOCATION 大小，由调用方合并回基础记录——这类文件的
/// 数据可能完全不在基础记录里（$ATTRIBUTE_LIST），不合并会整文件丢失。
fn parse_record(
    buf: &mut [u8],
    record_size: usize,
    record_num: u64,
    bytes_per_cluster: u64,
) -> RecOutcome {
    if !apply_fixups(buf, record_size) {
        return RecOutcome::None;
    }

    let flags = u16::from_le_bytes([buf[0x16], buf[0x17]]);
    let is_used = (flags & 0x01) != 0;
    if !is_used {
        return RecOutcome::None;
    }

    let base_ref = u64::from_le_bytes(buf[0x20..0x28].try_into().unwrap_or([0u8; 8]));
    let base = base_ref & 0xFFFFFFFFFFFF;
    if base != 0 {
        let mut sizes = accumulate_data_sizes(buf, record_size, bytes_per_cluster);
        // 扩展记录自身的 $FILE_NAME 只复制基础记录的名字信息，取其大小做回退
        let mut attr_offset = u16::from_le_bytes([buf[0x14], buf[0x15]]) as usize;
        for _ in 0..64 {
            if attr_offset + 8 > record_size {
                break;
            }
            let t = read_u32le(buf, attr_offset);
            if t == 0xFFFFFFFF || t == 0 {
                break;
            }
            let alen = read_u32le(buf, attr_offset + 4) as usize;
            if alen == 0 || attr_offset + alen > record_size {
                break;
            }
            if t == 0x30 && attr_offset + 0x40 <= record_size {
                let crel =
                    u16::from_le_bytes([buf[attr_offset + 0x14], buf[attr_offset + 0x15]]) as usize;
                let c = attr_offset + crel;
                if c + 0x38 <= record_size {
                    sizes.fn_alloc = u64::from_le_bytes(buf[c + 0x28..c + 0x30].try_into().unwrap());
                    sizes.fn_real = u64::from_le_bytes(buf[c + 0x30..c + 0x38].try_into().unwrap());
                }
                break;
            }
            attr_offset += alen;
        }
        return RecOutcome::Ext { base, sizes };
    }

    let is_dir = (flags & 0x02) != 0;
    let mut node = Node {
        is_dir,
        ..Default::default()
    };
    let mut got_name = false;
    // 部分记录只有 DOS 8.3 短文件名（长名含 Win32 非法字符，如 AMD 驱动解压目录），
    // 不能把整条记录丢弃——记下作为兜底。
    let mut dos_fallback: Option<(u64, String, u64, u64)> = None;

    let mut attr_offset = u16::from_le_bytes([buf[0x14], buf[0x15]]) as usize;
    loop {
        if attr_offset + 8 > record_size {
            break;
        }
        let attr_type = read_u32le(buf, attr_offset);
        if attr_type == 0xFFFFFFFF || attr_type == 0 {
            break;
        }
        let attr_len = read_u32le(buf, attr_offset + 4) as usize;
        if attr_len == 0 || attr_offset + attr_len > record_size {
            break;
        }

        if attr_type == 0x30 {
            // $FILE_NAME
            node.dbg_fn_count += 1;
            let content_rel =
                u16::from_le_bytes([buf[attr_offset + 0x14], buf[attr_offset + 0x15]]) as usize;
            let c = attr_offset + content_rel;
            if c + 0x42 > record_size {
                attr_offset += attr_len;
                continue;
            }

            let parent_ref = u64::from_le_bytes(buf[c..c + 8].try_into().unwrap_or([0u8; 8]));
            let name_len = buf[c + 0x40] as usize;
            let name_type = buf[c + 0x41];

            // 跳过 DOS 8.3 短文件名（type 2），优先 Win32/Posix 名；
            // 同时跳过 "." / ".."（根目录自引用名，parent 指向自身）。
            // 若整条记录只有 DOS 名，保留作兜底，避免整条记录被丢弃。
            if !got_name {
                let name_utf16: Vec<u16> = (0..name_len)
                    .map(|i| {
                        u16::from_le_bytes([buf[c + 0x42 + i * 2], buf[c + 0x42 + i * 2 + 1]])
                    })
                    .collect();
                let name = OsString::from_wide(&name_utf16)
                    .to_string_lossy()
                    .into_owned();
                if name == "." || name == ".." {
                    attr_offset += attr_len;
                    continue;
                }

                let parent = parent_ref & 0xFFFFFFFFFFFF; // 低48位
                let alloc = u64::from_le_bytes(buf[c + 0x28..c + 0x30].try_into().unwrap());
                let real = u64::from_le_bytes(buf[c + 0x30..c + 0x38].try_into().unwrap());
                if name_type != 2 {
                    node.parent = parent;
                    node.name = name;
                    node.dbg_fn_alloc = alloc;
                    node.dbg_fn_real = real;
                    got_name = true;
                } else if dos_fallback.is_none() {
                    dos_fallback = Some((parent, name, alloc, real));
                }
            }
        }

        attr_offset += attr_len;
    }

    // 只有 DOS 8.3 名的记录：启用兜底名，避免整条记录丢失
    if !got_name {
        if let Some((parent, name, alloc, real)) = dos_fallback.take() {
            node.parent = parent;
            node.name = name;
            node.dbg_fn_alloc = alloc;
            node.dbg_fn_real = real;
            got_name = true;
        }
    }

    if !(got_name || record_num == 5) {
        return RecOutcome::None;
    }

    let mut sizes = accumulate_data_sizes(buf, record_size, bytes_per_cluster);
    sizes.fn_alloc = node.dbg_fn_alloc;
    sizes.fn_real = node.dbg_fn_real;
    node.dbg_data_alloc = sizes.dalloc;
    RecOutcome::Node(node, sizes)
}
