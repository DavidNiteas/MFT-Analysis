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

export default function App() {
  const [drive, setDrive] = useState('C')
  const [limitGb, setLimitGb] = useState(8)
  const [scanning, setScanning] = useState(false)
  const [progress, setProgress] = useState(null)
  const [ready, setReady] = useState(false)
  const [isError, setIsError] = useState(false)
  const [status, setStatus] = useState('选择盘符后点击「开始扫描」。需要管理员权限。')

  const [root, setRoot] = useState(null)
  const [volStats, setVolStats] = useState(null)
  const [viewRoot, setViewRoot] = useState(MFT_ROOT)
  const [selected, setSelected] = useState(null)
  const [expanded, setExpanded] = useState(new Set())
  const [childrenCache, setChildrenCache] = useState({})
  const [crumbs, setCrumbs] = useState([])
  const [selPath, setSelPath] = useState('')
  const [topFiles, setTopFiles] = useState(null)
  const [filesLoading, setFilesLoading] = useState(false)

  const pollRef = useRef(null)
  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }

  const loadChildren = useCallback(async (record) => {
    const dirs = await api.getChildren(record)
    setChildrenCache((c) => ({ ...c, [record]: dirs }))
    return dirs
  }, [])

  const loadTopFiles = useCallback(async (record) => {
    setFilesLoading(true)
    try {
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
      setStatus(
        `扫描完成：${d.drive} 盘共 ${d.count.toLocaleString()} 条有效记录。` +
          `簇大小 ${humanSize(d.bytes_per_cluster)}，MFT 记录大小 ${humanSize(d.record_size)}，` +
          `MFT 偏移 0x${d.mft_byte_offset.toString(16).toUpperCase()}` +
          (d.cancelled ? '（已取消，结果不完整）' : '')
      )
      loadTree(MFT_ROOT)
    }).then((u) => (un1 = u))
    api.onScanError((e) => {
      stopPolling()
      setScanning(false)
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

  const startScan = async () => {
    const d = (drive.trim()[0] || '').toUpperCase()
    if (!/^[A-Z]$/.test(d)) {
      setIsError(true)
      setStatus('盘符无效，请输入 A-Z。')
      return
    }
    stopPolling()
    setScanning(true)
    setReady(false)
    setIsError(false)
    setProgress(null)
    setRoot(null)
    setSelected(null)
    setSelPath('')
    setExpanded(new Set())
    setChildrenCache({})
    setTopFiles(null)
    setStatus(`正在扫描 ${d}: 盘 MFT ...`)
    try {
      await api.startScan(d, limitGb)
      pollRef.current = setInterval(async () => {
        try {
          setProgress(await api.getProgress())
        } catch {
          /* 忽略轮询期间的瞬时错误 */
        }
      }, 300)
    } catch (e) {
      setScanning(false)
      setIsError(true)
      setStatus(String(e))
    }
  }

  const toggle = async (record) => {
    const next = new Set(expanded)
    if (next.has(record)) {
      next.delete(record)
    } else {
      next.add(record)
      if (!childrenCache[record]) await loadChildren(record)
    }
    setExpanded(next)
  }

  const select = async (record) => {
    setSelected(record)
    try {
      const chain = await api.getChain(record)
      setSelPath(chainToPath(chain))
    } catch {
      setSelPath('')
    }
    await loadTopFiles(record)
  }

  const drill = () => {
    if (selected != null) loadTree(selected)
  }

  const renderNode = (dir, depth) => {
    if (depth > 48) return null
    const isOpen = expanded.has(dir.record)
    const kids = childrenCache[dir.record]
    const base = root?.subtree_size || 0
    const frac = base > 0 ? dir.subtree_size / base : 0
    return (
      <div key={dir.record}>
        <div className="tree-row">
          <span className="indent" style={{ width: depth * 14 }} />
          <button className="arrow" onClick={() => toggle(dir.record)}>
            {isOpen ? '▼' : '▶'}
          </button>
          <span className="bar" style={{ width: Math.max(2, frac * 140) }} />
          <span
            className={selected === dir.record ? 'name sel' : 'name'}
            onClick={() => select(dir.record)}
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
    <div id="root-layout">
      <div className="topbar">
        <label>
          盘符:{' '}
          <input
            value={drive}
            maxLength={1}
            disabled={scanning}
            onChange={(e) => setDrive(e.target.value.toUpperCase())}
            style={{ width: 34 }}
          />
        </label>
        <label>
          MFT 读取上限（安全上限）:{' '}
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
          GB
        </label>
        {scanning ? (
          <button className="act" onClick={() => api.cancelScan()}>
            取消
          </button>
        ) : (
          <button className="act" onClick={startScan}>
            开始扫描
          </button>
        )}
        {progress && (
          <span className="prog">
            <progress value={progress.bytes_scanned} max={progress.total_bytes || 1} />
            已读 {(progress.bytes_scanned / 1048576).toFixed(0)} MB，记录{' '}
            {progress.records.toLocaleString()}，有效 {progress.valid.toLocaleString()}
          </span>
        )}
        <div className={isError ? 'status error' : 'status'}>{status}</div>
      </div>

      {!ready ? (
        <div className="empty">
          <h2>MFT Reader</h2>
          <p>读取 NTFS 主文件表（MFT），分析磁盘空间占用。</p>
          <p>请以管理员身份运行，否则无法打开卷设备。</p>
        </div>
      ) : (
        <div className="main">
          <div className="tree">
            <div className="crumbs">
              {crumbs.map((c, i) => (
                <span key={c.record}>
                  {i > 0 && <span className="sep">\</span>}
                  <button onClick={() => loadTree(c.record)}>{c.name}</button>
                </span>
              ))}
              {selected != null && <button onClick={drill}>⤵ 以所选为根</button>}
            </div>
            <div className="tree-body">
              {root && (
                <div className="tree-row head">
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
              Top 文件（{focusPath || '...'} 下，按大小排序）
            </div>
            {volStats && root && (
              <div className="recon">
                磁盘已用 {humanSize(volStats.used)} · 已统计{' '}
                {humanSize(root.total_scanned)} · 差额{' '}
                {humanSize(Math.max(0, volStats.used - root.total_scanned))}
                （NTFS 元数据 / MFT 保留区 / 未归属簇）
              </div>
            )}
            {topFiles?.truncated && (
              <div className="warn">⚠ 该目录文件数过多，统计已截断，结果可能不完整。</div>
            )}
            {filesLoading ? (
              <div className="status">计算中...</div>
            ) : (
              <table>
                <tbody>
                  {topFiles?.files.map(([p, s], i) => (
                    <tr key={i}>
                      <td className="rank">{i + 1}.</td>
                      <td className="num">{humanSize(s)}</td>
                      <td className="path">{p}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {!filesLoading && topFiles && topFiles.files.length === 0 && (
              <div className="status">该目录下没有文件。</div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
