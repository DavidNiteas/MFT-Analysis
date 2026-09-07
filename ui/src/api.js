import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export const startScan = (drive, limitGb) => invoke('scan_start', { drive, limitGb })
export const cancelScan = () => invoke('scan_cancel')
export const getProgress = () => invoke('scan_progress')
export const getRoot = () => invoke('get_root')
export const getChildren = (record, includeFiles) =>
  invoke('get_children', { record: Number(record), includeFiles: !!includeFiles })
export const getTopFiles = (record) => invoke('get_top_files', { record: Number(record) })
export const getChain = (record) => invoke('get_chain', { record: Number(record) })
export const getVolumeStats = () => invoke('get_volume_stats')
export const revealInExplorer = (path) => invoke('reveal_in_explorer', { path })
export const openDir = (path) => invoke('open_dir', { path })
export const listDrives = () => invoke('list_drives')
export const exportText = (suggestedName, content) =>
  invoke('export_text', { suggestedName, content })
export const getDirPie = (record) => invoke('get_dir_pie', { record: Number(record) })
export const getFilesPie = (record, byExt, k) =>
  invoke('get_files_pie', { record: Number(record), byExt: !!byExt, k: Number(k) || 10 })

export const onScanDone = (cb) => listen('scan-done', (e) => cb(e.payload))
export const onScanError = (cb) => listen('scan-error', (e) => cb(e.payload))
