import { useCallback, useEffect, useRef, useState } from 'react'
import * as api from './api.js'

const MFT_ROOT = 5

function humanSize(size) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  if (!size) return '0 B'
  const exp = Math.min(Math.floor(Math.log(size) / Math.log(1024)), units.length - 1)
  return `${(size / 1024 ** exp).toFixed(2)} ${units[exp]}`
}

function chainToPath(chain) {
  if (!chain.length) return ''
  const first = chain[0].name.replace(/\\+$/, '')
  return first + '\\' + chain.slice(1).map((c) => c.name).join('\\')
}

function timestamp() {
  const d = new Date()
  const p = (n) => String(n).padStart(2, '0')
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`
}

function csvCell(v) {
  const s = String(v)
  return /[",\r\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s
}

function splitPath(p) {
  const idx = p.lastIndexOf('\\')
  if (idx <= 2) return { dir: p.slice(0, 3), name: p.slice(idx + 1) }
  return { dir: p.slice(0, idx), name: p.slice(idx + 1) }
}

function copyToClipboard(text) {
  if (navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(text)
  }
  return new Promise((resolve, reject) => {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    try {
      document.execCommand('copy') ? resolve() : reject(new Error('复制失败'))
    } catch (e) {
      reject(e)
    } finally {
      document.body.removeChild(ta)
    }
  })
}

const FolderIcon = () => (
  <svg className="icon" width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
    <path
      fill="#e8b93e"
      d="M1 3.5A1.5 1.5 0 0 1 2.5 2h3.6a1.5 1.5 0 0 1 1.1.5l1.2 1.4a1.5 1.5 0 0 0 1.1.5h3.9A1.5 1.5 0 0 1 16 6v6a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 0 12V4.5z"
    />
  </svg>
)

const FileIcon = () => (
  <svg className="icon" width="12" height="13" viewBox="0 0 16 16" aria-hidden="true">
    <path
      fill="#9aa0aa"
      d="M3.5 0h6L14 4.5V14a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 14V1.5A1.5 1.5 0 0 1 3.5 0zM9 1v4h4"
    />
  </svg>
)

// ── 多语言 ──
const I18N = {
  zh: {
    driveLabel: '盘符: ',
    limitLabel: 'MFT 读取上限（安全上限）: ',
    gb: 'GB',
    cancel: '取消',
    startScan: '开始扫描',
    refresh: '刷新',
    refreshTitle: '用相同盘符重新扫描 MFT；目录树保持展开状态，仅更新数据',
    scanning: (d) => `正在扫描 ${d}: 盘 MFT ...`,
    refreshScanning: '正在重新扫描（保留目录树展开状态）...',
    invalidDrive: '盘符无效，请输入 A-Z。',
    progress: (mb, rec, valid) => `已读 ${mb} MB，记录 ${rec}，有效 ${valid}`,
    scanDone: (d) =>
      `扫描完成：${d.drive} 盘共 ${d.count.toLocaleString()} 条有效记录。` +
      `簇大小 ${humanSize(d.bytes_per_cluster)}，MFT 记录大小 ${humanSize(d.record_size)}，` +
      `MFT 偏移 0x${d.mft_byte_offset.toString(16).toUpperCase()}` +
      (d.cancelled ? '（已取消，结果不完整）' : ''),
    initStatus: '选择盘符后点击「开始扫描」。需要管理员权限。',
    emptyDesc1: '读取 NTFS 主文件表（MFT），分析磁盘空间占用。',
    emptyDesc2: '请以管理员身份运行，否则无法打开卷设备。',
    drill: '⤵ 以所选为根',
    showFiles: '显示文件',
    showFilesTitle: '在目录树中同时显示文件',
    exportTree: '导出树',
    exportTreeTitle: '导出当前展开的目录树为文本',
    topFiles: (p) => `Top 文件（${p} 下，按大小排序）`,
    exportCsv: '导出 CSV',
    exportCsvTitle: '把当前列表导出为 CSV（UTF-8）',
    pieAnalyze: '饼图分析',
    pieAnalyzeTitle: '按文件名 / 后缀聚合绘制子树文件占用饼图',
    recon: (used, scanned, diff) =>
      `磁盘已用 ${used} · 已统计 ${scanned} · 差额 ${diff}（NTFS 元数据 / MFT 保留区 / 未归属簇）`,
    truncatedWarn: '⚠ 该目录文件数过多，统计已截断，结果可能不完整。',
    computing: '计算中...',
    noFiles: '该目录下没有文件。',
    menuReveal: '打开所在目录',
    menuOpenDir: '打开此目录',
    menuDirPie: '查看空间饼图',
    menuCopyPath: '复制路径',
    copied: (p) => `已复制路径：${p}`,
    copyFailed: (e) => `复制失败: ${e}`,
    csvExported: (p) => `已导出 CSV：${p}`,
    treeExported: (p) => `已导出目录树：${p}`,
    csvHeader: ['排名', '文件名', '所属目录', '路径', '大小(字节)', '大小'],
    dirPieTitle: '空间占用饼图 — ',
    filesPieTitle: '文件饼图分析 — ',
    viewLabel: '视角: ',
    byFile: '按文件（Top-K）',
    byExt: '按后缀聚合',
    kLabel: 'K: ',
    otherNote: '超出 Top-K 的项合并为「其他」',
    noData: '没有可绘制的数据。',
    noItems: '该范围没有占用空间的项目。',
    themeTitle: '切换白天 / 黑夜模式',
    themeBtn: (theme) => (theme === 'dark' ? '☀ 白天' : '🌙 黑夜'),
    langTitle: 'Switch language',
    langBtn: (lang) => (lang === 'zh' ? 'EN' : '中'),
    dirMark: 'D',
    fileMark: 'F',
  },
  en: {
    driveLabel: 'Drive: ',
    limitLabel: 'MFT read limit (safety cap): ',
    gb: 'GB',
    cancel: 'Cancel',
    startScan: 'Start Scan',
    refresh: 'Refresh',
    refreshTitle: 'Re-scan the MFT with the same drive; the tree keeps its expansion, only data updates',
    scanning: (d) => `Scanning MFT of ${d}: ...`,
    refreshScanning: 'Re-scanning (keeping tree expansion)...',
    invalidDrive: 'Invalid drive letter, enter A-Z.',
    progress: (mb, rec, valid) => `Read ${mb} MB, records ${rec}, valid ${valid}`,
    scanDone: (d) =>
      `Scan complete: ${d.count.toLocaleString()} valid records on drive ${d.drive}. ` +
      `Cluster size ${humanSize(d.bytes_per_cluster)}, MFT record size ${humanSize(d.record_size)}, ` +
      `MFT offset 0x${d.mft_byte_offset.toString(16).toUpperCase()}` +
      (d.cancelled ? ' (cancelled, results incomplete)' : ''),
    initStatus: 'Select a drive and click "Start Scan". Administrator rights required.',
    emptyDesc1: 'Reads the NTFS Master File Table (MFT) to analyze disk space usage.',
    emptyDesc2: 'Run as administrator, otherwise the volume device cannot be opened.',
    drill: '⤵ Set as root',
    showFiles: 'Show files',
    showFilesTitle: 'Also show files in the directory tree',
    exportTree: 'Export Tree',
    exportTreeTitle: 'Export the currently expanded tree as text',
    topFiles: (p) => `Top files under ${p}, by size`,
    exportCsv: 'Export CSV',
    exportCsvTitle: 'Export the current list as CSV (UTF-8)',
    pieAnalyze: 'Pie Analysis',
    pieAnalyzeTitle: 'Pie chart of subtree file usage by file name / extension',
    recon: (used, scanned, diff) =>
      `Disk used ${used} · accounted ${scanned} · gap ${diff} (NTFS metadata / MFT reserved / unattributed clusters)`,
    truncatedWarn: '⚠ Too many files in this directory; statistics were truncated and may be incomplete.',
    computing: 'Computing...',
    noFiles: 'No files under this directory.',
    menuReveal: 'Open containing folder',
    menuOpenDir: 'Open this folder',
    menuDirPie: 'Show space pie chart',
    menuCopyPath: 'Copy path',
    copied: (p) => `Path copied: ${p}`,
    copyFailed: (e) => `Copy failed: ${e}`,
    csvExported: (p) => `CSV exported: ${p}`,
    treeExported: (p) => `Tree exported: ${p}`,
    csvHeader: ['Rank', 'File Name', 'Folder', 'Path', 'Size (bytes)', 'Size'],
    dirPieTitle: 'Space Usage Pie — ',
    filesPieTitle: 'File Pie Analysis — ',
    viewLabel: 'View: ',
    byFile: 'By file (Top-K)',
    byExt: 'By extension',
    kLabel: 'K: ',
    otherNote: 'Items beyond Top-K are merged into "Other"',
    noData: 'Nothing to draw.',
    noItems: 'No space-consuming items in this scope.',
    themeTitle: 'Toggle light / dark mode',
    themeBtn: (theme) => (theme === 'dark' ? '☀ Light' : '🌙 Dark'),
    langTitle: '切换语言 / Switch language',
    langBtn: (lang) => (lang === 'zh' ? 'EN' : '中'),
    dirMark: 'D',
    fileMark: 'F',
  },
}

// ── 饼图 ──
const PIE_PALETTE = [
  '#4e7ec2', '#e8b93e', '#5cb87a', '#d16969', '#9a6fd1', '#4ec2b8',
  '#d18a4e', '#7a9e43', '#c25c9e', '#6f8bd1', '#b8b04e', '#8a8f98',
]

function PieSvg({ items, total }) {
  const r = 80
  const c = 100
  if (!total) return null
  let angle = -Math.PI / 2
  const paths = items.map((it, i) => {
    const frac = it.size / total
    if (frac <= 0) return null
    const color = PIE_PALETTE[i % PIE_PALETTE.length]
    if (frac >= 0.99999) {
      return <circle key={i} cx={c} cy={c} r={r} fill={color} />
    }
    const a0 = angle
    const a1 = angle + frac * Math.PI * 2
    angle = a1
    const large = frac > 0.5 ? 1 : 0
    const x0 = c + r * Math.cos(a0)
    const y0 = c + r * Math.sin(a0)
    const x1 = c + r * Math.cos(a1)
    const y1 = c + r * Math.sin(a1)
    return (
      <path
        key={i}
        d={`M ${c} ${c} L ${x0} ${y0} A ${r} ${r} 0 ${large} 1 ${x1} ${y1} Z`}
        fill={color}
        stroke="var(--bg)"
        strokeWidth="1"
      />
    )
  })
  return (
    <svg width="200" height="200" viewBox="0 0 200 200" className="pie-svg">
      {paths}
    </svg>
  )
}

function PieModal({ pie, t, onClose }) {
  const [data, setData] = useState(null)
  const [byExt, setByExt] = useState(false)
  const [k, setK] = useState(10)
  const [err, setErr] = useState('')

  useEffect(() => {
    let dead = false
    setData(null)
    setErr('')
    const req =
      pie.kind === 'dir'
        ? api.getDirPie(pie.record)
        : api.getFilesPie(pie.record, byExt, k)
    req.then((d) => !dead && setData(d)).catch((e) => !dead && setErr(String(e)))
    return () => {
      dead = true
    }
  }, [pie, byExt, k])

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const total = data?.total || 0
  return (
    <div className="pie-overlay" onClick={onClose}>
      <div className="pie-modal" onClick={(e) => e.stopPropagation()}>
        <div className="pie-head">
          <span>
            {pie.kind === 'dir' ? t.dirPieTitle : t.filesPieTitle}
            {data ? data.title : '...'}
          </span>
          <button className="mini" onClick={onClose}>
            ✕
          </button>
        </div>
        {pie.kind === 'files' && (
          <div className="pie-controls">
            <label>
              {t.viewLabel}{' '}
              <select value={byExt ? 'ext' : 'file'} onChange={(e) => setByExt(e.target.value === 'ext')}>
                <option value="file">{t.byFile}</option>
                <option value="ext">{t.byExt}</option>
              </select>
            </label>
            <label>
              {t.kLabel}{' '}
              <input
                type="number"
                min={3}
                max={100}
                value={k}
                style={{ width: 56 }}
                onChange={(e) => setK(Math.max(3, Math.min(100, Number(e.target.value) || 10)))}
              />
            </label>
            <span className="status">{t.otherNote}</span>
          </div>
        )}
        {err && <div className="status error">{err}</div>}
        {!data && !err && <div className="status">{t.computing}</div>}
        {data && (
          <div className="pie-body">
            {total ? <PieSvg items={data.items} total={total} /> : <div className="status">{t.noData}</div>}
            <div className="pie-legend">
              {data.items.map((it, i) => (
                <div className="legend-row" key={i} title={it.name}>
                  <span className="swatch" style={{ background: PIE_PALETTE[i % PIE_PALETTE.length] }} />
                  <span className="legend-name">{it.name}</span>
                  <span className="legend-size">{humanSize(it.size)}</span>
                  <span className="legend-pct">{total ? ((it.size / total) * 100).toFixed(1) : 0}%</span>
                </div>
              ))}
              {data.items.length === 0 && <div className="status">{t.noItems}</div>}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

export default function App() {
  const [drive, setDrive] = useState('C')
  const [drives, setDrives] = useState([])
  const [limitGb, setLimitGb] = useState(8)
  const [scanning, setScanning] = useState(false)
  const [progress, setProgress] = useState(null)
  const [ready, setReady] = useState(false)
  const [isError, setIsError] = useState(false)
  const [status, setStatus] = useState('')
  const [lang, setLang] = useState(() => localStorage.getItem('mft-lang') || 'zh')
  const [theme, setTheme] = useState(() => localStorage.getItem('mft-theme') || 'dark')
  const t = I18N[lang] || I18N.zh

  useEffect(() => {
    localStorage.setItem('mft-lang', lang)
  }, [lang])
  useEffect(() => {
    document.documentElement.dataset.theme = theme
    localStorage.setItem('mft-theme', theme)
  }, [theme])

  const [root, setRoot] = useState(null)
  const [volStats, setVolStats] = useState(null)
  const [viewRoot, setViewRoot] = useState(MFT_ROOT)
  const [selected, setSelected] = useState(null)
  const [expanded, setExpanded] = useState(new Set())
  const [childrenCache, setChildrenCache] = useState({})
  const [crumbs, setCrumbs] = useState([])
  const [selPath, setSelPath] = useState('')
  const [topFiles, setTopFiles] = useState(null)
  const [topRec, setTopRec] = useState(null) // topFiles 对应的目录 record（饼图分析用）
  const [filesLoading, setFilesLoading] = useState(false)
  const [ctxMenu, setCtxMenu] = useState(null) // { x, y, path, isDir, record }
  const [pie, setPie] = useState(null) // { kind: 'dir' | 'files', record }
  const [showFiles, setShowFiles] = useState(false)
  const showFilesRef = useRef(false)
  const selectedIsDirRef = useRef(true)
  showFilesRef.current = showFiles
  const lastScanRef = useRef(null) // { drive, limitGb }，刷新时用
  const keepTreeRef = useRef(false) // 刷新扫描：完成后保留目录树展开状态
  const reloadTreeRef = useRef(() => {})
  const tRef = useRef(I18N.zh)
  tRef.current = t

  const pollRef = useRef(null)
  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }

  const loadChildren = useCallback(async (record) => {
    const dirs = await api.getChildren(record, showFilesRef.current)
    setChildrenCache((c) => ({ ...c, [record]: dirs }))
    return dirs
  }, [])

  // 枚举可用盘符，填充下拉框
  useEffect(() => {
    api
      .listDrives()
      .then((list) => {
        setDrives(list)
        setDrive((d) => (list.includes(d) ? d : list[0] || 'C'))
      })
      .catch(() => {})
  }, [])

  const loadTopFiles = useCallback(async (record) => {
    setFilesLoading(true)
    try {
      setTopRec(record)
      setTopFiles(await api.getTopFiles(record))
    } catch (e) {
      setIsError(true)
      setStatus(String(e))
    } finally {
      setFilesLoading(false)
    }
  }, [])

  const loadTree = useCallback(
    async (rec) => {
      setViewRoot(rec)
      setSelected(null)
      setSelPath('')
      setExpanded(new Set([rec]))
      setChildrenCache({})
      const [chain, rootDto, stats] = await Promise.all([
        api.getChain(rec),
        api.getRoot(),
        api.getVolumeStats(),
      ])
      setCrumbs(chain)
      setRoot(rootDto)
      setVolStats(stats)
      await Promise.all([loadChildren(rec), loadTopFiles(rec)])
    },
    [loadChildren, loadTopFiles]
  )

  useEffect(() => {
    let un1, un2
    api.onScanDone((d) => {
      stopPolling()
      setScanning(false)
      setReady(true)
      setProgress(null)
      setIsError(false)
      setStatus(tRef.current.scanDone(d))
      keepTreeRef.current ? reloadTreeRef.current() : loadTree(MFT_ROOT)
    }).then((u) => (un1 = u))
    api.onScanError((e) => {
      stopPolling()
      setScanning(false)
      keepTreeRef.current = false
      setProgress(null)
      setIsError(true)
      setStatus(String(e))
    }).then((u) => (un2 = u))
    return () => {
      stopPolling()
      un1?.()
      un2?.()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const doScan = async (d, keepTree) => {
    stopPolling()
    keepTreeRef.current = keepTree
    setScanning(true)
    setIsError(false)
    setProgress(null)
    if (!keepTree) {
      setReady(false)
      setRoot(null)
      setSelected(null)
      setSelPath('')
      setExpanded(new Set())
      setChildrenCache({})
      setTopFiles(null)
      setTopRec(null)
      setPie(null)
      setVolStats(null)
    }
    setStatus(t.scanning(d))
    try {
      await api.startScan(d, lastScanRef.current.limitGb)
      pollRef.current = setInterval(async () => {
        try {
          setProgress(await api.getProgress())
        } catch {
          /* 忽略轮询期间的瞬时错误 */
        }
      }, 300)
    } catch (e) {
      setScanning(false)
      keepTreeRef.current = false
      setIsError(true)
      setStatus(String(e))
    }
  }

  const startScan = async () => {
    const d = (drive.trim()[0] || '').toUpperCase()
    if (!/^[A-Z]$/.test(d)) {
      setIsError(true)
      setStatus(t.invalidDrive)
      return
    }
    lastScanRef.current = { drive: d, limitGb }
    await doScan(d, false)
  }

  // 刷新：用相同的盘符/上限重扫，完成后保留目录树的展开状态，
  // 只重载已展开节点的子项 —— 形状不变，数值与新出现的子项会更新。
  const refreshScan = async () => {
    if (!lastScanRef.current || scanning) return
    setStatus(t.refreshScanning)
    await doScan(lastScanRef.current.drive, true)
  }

  const reloadTree = useCallback(async () => {
    try {
      const [rootDto, stats] = await Promise.all([api.getRoot(), api.getVolumeStats()])
      setRoot(rootDto)
      setVolStats(stats)
      // 重载当前视图根与所有已展开节点的子项；已消失的记录自然落空
      const records = [viewRoot, ...expanded]
      await Promise.all(records.map((r) => loadChildren(r).catch(() => {})))
      // 刷新选中项路径；记录已不存在则清除选中
      if (selected != null) {
        try {
          const chain = await api.getChain(selected)
          setSelPath(chainToPath(chain))
        } catch {
          setSelected(null)
          setSelPath('')
        }
      }
      // 右侧 Top 文件列表按原 record 重载（已消失则忽略）
      if (topRec != null) await loadTopFiles(topRec).catch(() => {})
    } catch (e) {
      setIsError(true)
      setStatus(String(e))
    }
  }, [viewRoot, expanded, selected, topRec, loadChildren, loadTopFiles])
  reloadTreeRef.current = reloadTree

  const toggle = async (record) => {
    const next = new Set(expanded)
    if (next.has(record)) {
      next.delete(record)
    } else {
      next.add(record)
      if (!childrenCache[record]) await loadChildren(record).catch(() => {})
    }
    setExpanded(next)
  }

  const select = async (record, isDir = true) => {
    setSelected(record)
    selectedIsDirRef.current = isDir
    try {
      const chain = await api.getChain(record)
      setSelPath(chainToPath(chain))
    } catch {
      setSelPath('')
    }
    if (isDir) await loadTopFiles(record)
  }

  // 切换树中是否显示文件：清空子项缓存并按新开关重载当前视图
  const toggleShowFiles = () => {
    const next = !showFiles
    setShowFiles(next)
    showFilesRef.current = next
    setChildrenCache({})
    if (ready) loadChildren(viewRoot).catch(() => {})
  }

  // 导出右侧当前 Top 文件列表为 CSV
  const exportCsv = async () => {
    if (!topFiles?.files?.length) return
    try {
      const rows = [t.csvHeader]
      topFiles.files.forEach(([p, s], i) => {
        const { dir, name } = splitPath(p)
        rows.push([i + 1, name, dir, p, s, humanSize(s)])
      })
      const csv = rows.map((r) => r.map(csvCell).join(',')).join('\r\n')
      const path = await api.exportText(`mft-top-files-${timestamp()}.csv`, csv)
      setStatus(t.csvExported(path))
      await api.revealInExplorer(path)
    } catch (e) {
      setIsError(true)
      setStatus(String(e))
    }
  }

  // 导出左侧当前树（仅已展开的层级；显示文件开关生效）为文本
  const exportTree = async () => {
    if (!root) return
    try {
      const lines = [`${root.name}  ${humanSize(root.subtree_size)}`]
      const walk = (record, depth) => {
        if (!expanded.has(record)) return
        const kids = childrenCache[record]
        if (!kids) return
        for (const k of kids) {
          const mark = k.is_dir ? t.dirMark : t.fileMark
          lines.push(`${'  '.repeat(depth + 1)}[${mark}] ${k.name}  ${humanSize(k.subtree_size)}`)
          if (k.is_dir && expanded.has(k.record)) walk(k.record, depth + 1)
        }
      }
      walk(viewRoot, 0)
      const path = await api.exportText(`mft-tree-${timestamp()}.txt`, lines.join('\r\n'))
      setStatus(t.treeExported(path))
      await api.revealInExplorer(path)
    } catch (e) {
      setIsError(true)
      setStatus(String(e))
    }
  }

  const drill = () => {
    if (selected != null && selectedIsDirRef.current) loadTree(selected).catch(() => {})
  }

  // ── 右键菜单：打开所在目录 / 打开此目录 ──
  useEffect(() => {
    if (!ctxMenu) return
    const close = () => setCtxMenu(null)
    const onKey = (e) => {
      if (e.key === 'Escape') close()
    }
    // 行内处理器会 stopPropagation，能冒泡到这里的右键一律用于关闭菜单
    window.addEventListener('click', close)
    window.addEventListener('contextmenu', close)
    window.addEventListener('blur', close)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('click', close)
      window.removeEventListener('contextmenu', close)
      window.removeEventListener('blur', close)
      window.removeEventListener('keydown', onKey)
    }
  }, [ctxMenu])

  const showMenuAt = (x, y, path, isDir, record = 0) =>
    setCtxMenu({ x, y, path, isDir, record })

  const showMenu = (e, path, isDir, record = 0) => {
    e.preventDefault()
    e.stopPropagation()
    showMenuAt(e.clientX, e.clientY, path, isDir, record)
  }

  const menuReveal = () => {
    const p = ctxMenu.path
    setCtxMenu(null)
    api.revealInExplorer(p).catch((err) => {
      setIsError(true)
      setStatus(String(err))
    })
  }

  const menuOpenDir = () => {
    const p = ctxMenu.path
    setCtxMenu(null)
    api.openDir(p).catch((err) => {
      setIsError(true)
      setStatus(String(err))
    })
  }

  const menuCopyPath = () => {
    const p = ctxMenu.path
    setCtxMenu(null)
    copyToClipboard(p).then(
      () => setStatus(t.copied(p)),
      (err) => {
        setIsError(true)
        setStatus(t.copyFailed(String(err)))
      }
    )
  }

  // 目录树行：先同步拦住事件（防止 await 期间冒泡到 window 把新菜单关掉），
  // 再异步回溯完整路径弹出菜单
  const treeCtx = (e, record) => {
    e.preventDefault()
    e.stopPropagation()
    const { clientX, clientY } = e
    api
      .getChain(record)
      .then((chain) => showMenuAt(clientX, clientY, chainToPath(chain), true, record))
      .catch(() => {})
  }

  const menuDirPie = () => {
    const r = ctxMenu.record
    setCtxMenu(null)
    setPie({ kind: 'dir', record: r })
  }

  // 空白区域右键：屏蔽 WebView 默认菜单（保留输入框的右键粘贴）
  const blankCtx = (e) => {
    const t = e.target
    if (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement) return
    e.preventDefault()
  }

  const renderNode = (dir, depth) => {
    if (depth > 48) return null
    const isOpen = expanded.has(dir.record)
    const kids = childrenCache[dir.record]
    const base = root?.subtree_size || 0
    const frac = base > 0 ? dir.subtree_size / base : 0
    return (
      <div key={dir.record}>
        <div className="tree-row" onContextMenu={(e) => treeCtx(e, dir.record)}>
          <span className="indent" style={{ width: depth * 14 }} />
          {dir.is_dir ? (
            <button className="arrow" onClick={() => toggle(dir.record)}>
              {isOpen ? '▼' : '▶'}
            </button>
          ) : (
            <span className="arrow placeholder" />
          )}
          <span className="type-icon">{dir.is_dir ? <FolderIcon /> : <FileIcon />}</span>
          <span className="bar" style={{ width: Math.max(2, frac * 140) }} />
          <span
            className={selected === dir.record ? 'name sel' : 'name'}
            onClick={() => select(dir.record, dir.is_dir)}
          >
            {dir.name}
            <span className="size">{humanSize(dir.subtree_size)}</span>
          </span>
        </div>
        {isOpen && kids && kids.map((k) => renderNode(k, depth + 1))}
      </div>
    )
  }

  const focusPath = selected != null ? selPath : chainToPath(crumbs)
  const kids = childrenCache[viewRoot]

  return (
    <div id="root-layout" onContextMenu={blankCtx}>
      <div className="topbar">
        <label>
          {t.driveLabel}{' '}
          <select
            value={drive}
            disabled={scanning}
            onChange={(e) => setDrive(e.target.value)}
            style={{ width: 56 }}
          >
            {drives.map((d) => (
              <option key={d} value={d}>
                {d}:
              </option>
            ))}
          </select>
        </label>
        <label>
          {t.limitLabel}{' '}
          <input
            type="number"
            min={0.5}
            max={64}
            step={0.5}
            value={limitGb}
            disabled={scanning}
            onChange={(e) => setLimitGb(Number(e.target.value) || 2)}
            style={{ width: 64 }}
          />{' '}
          {t.gb}
        </label>
        {scanning ? (
          <button className="act" onClick={() => api.cancelScan()}>
            {t.cancel}
          </button>
        ) : (
          <button className="act" onClick={startScan}>
            {t.startScan}
          </button>
        )}
        {!scanning && ready && (
          <button className="act" onClick={refreshScan} title={t.refreshTitle}>
            {t.refresh}
          </button>
        )}
        <button
          className="mini"
          onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
          title={t.themeTitle}
        >
          {t.themeBtn(theme)}
        </button>
        <button className="mini" onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')} title={t.langTitle}>
          {t.langBtn(lang)}
        </button>
        {progress && (
          <span className="prog">
            <progress value={progress.bytes_scanned} max={progress.total_bytes || 1} />
            {t.progress(
              (progress.bytes_scanned / 1048576).toFixed(0),
              progress.records.toLocaleString(),
              progress.valid.toLocaleString()
            )}
          </span>
        )}
        <div className={isError ? 'status error' : 'status'}>
          {status || (!ready ? t.initStatus : '')}
        </div>
      </div>

      {!ready ? (
        <div className="empty">
          <h2>MFT Analysis</h2>
          <p>{t.emptyDesc1}</p>
          <p>{t.emptyDesc2}</p>
        </div>
      ) : (
        <div className="main">
          <div className="tree">
            <div className="crumbs">
              {crumbs.map((c, i) => (
                <span key={c.record}>
                  {i > 0 && <span className="sep">\</span>}
                  <button onClick={() => loadTree(c.record).catch(() => {})}>{c.name}</button>
                </span>
              ))}
              {selected != null && <button onClick={drill}>{t.drill}</button>}
              <span className="spacer" />
              <button
                className={showFiles ? 'mini on' : 'mini'}
                onClick={toggleShowFiles}
                title={t.showFilesTitle}
              >
                {showFiles ? `☑ ${t.showFiles}` : `☐ ${t.showFiles}`}
              </button>
              <button className="mini" onClick={exportTree} title={t.exportTreeTitle}>
                {t.exportTree}
              </button>
            </div>
            <div className="tree-body">
              {root && (
                <div
                  className="tree-row head"
                  onContextMenu={(e) => showMenu(e, root.name, true, MFT_ROOT)}
                >
                  <span className="name">
                    {root.name}
                    <span className="size">{humanSize(root.subtree_size)}</span>
                  </span>
                </div>
              )}
              {kids && kids.map((k) => renderNode(k, 0))}
            </div>
          </div>
          <div className="files">
            <div className="files-head">
              {t.topFiles(focusPath || '...')}
              <button
                className="mini"
                onClick={exportCsv}
                disabled={!topFiles?.files?.length}
                title={t.exportCsvTitle}
              >
                {t.exportCsv}
              </button>
              <button
                className="mini"
                onClick={() => setPie({ kind: 'files', record: topRec ?? viewRoot })}
                disabled={topRec == null}
                title={t.pieAnalyzeTitle}
              >
                {t.pieAnalyze}
              </button>
            </div>
            {volStats && root && (
              <div className="recon">
                {t.recon(
                  humanSize(volStats.used),
                  humanSize(root.total_scanned),
                  humanSize(Math.max(0, volStats.used - root.total_scanned))
                )}
              </div>
            )}
            {topFiles?.truncated && <div className="warn">{t.truncatedWarn}</div>}
            {filesLoading ? (
              <div className="status">{t.computing}</div>
            ) : (
              <table>
                <tbody>
                  {topFiles?.files.map(([p, s], i) => (
                    <tr key={i} onContextMenu={(e) => showMenu(e, p, false)}>
                      <td className="rank">{i + 1}.</td>
                      <td className="num">{humanSize(s)}</td>
                      <td className="path">{p}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {!filesLoading && topFiles && topFiles.files.length === 0 && (
              <div className="status">{t.noFiles}</div>
            )}
          </div>
        </div>
      )}
      {ctxMenu && (
        <div className="ctxmenu" style={{ left: ctxMenu.x, top: ctxMenu.y }}>
          <button onClick={menuReveal}>{t.menuReveal}</button>
          {ctxMenu.isDir && <button onClick={menuOpenDir}>{t.menuOpenDir}</button>}
          {ctxMenu.isDir && <button onClick={menuDirPie}>{t.menuDirPie}</button>}
          <button onClick={menuCopyPath}>{t.menuCopyPath}</button>
        </div>
      )}
      {pie && <PieModal pie={pie} t={t} onClose={() => setPie(null)} />}
    </div>
  )
}
