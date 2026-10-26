import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

// 検索インデックス再構築の実行状態と進捗。大きなメールボックスでは数分かかるため、
// 途中で設定画面を離れて戻ってきても進捗表示が続くよう、画面(SettingsView)の外の
// モジュールに持たせる（`updater.svelte.js`と同じ理由）。
export const reindexer = $state({
  running: false,
  processed: 0,
  total: 0,
  // 完了・失敗時のメッセージ。
  status: '',
})

// src-tauri側の`reindex_all`が開始直後と1000件ごとに発火する。
// アプリ全体で1つあればよいので、モジュールの読み込み時に一度だけ購読する。
listen('reindex-progress', (event) => {
  reindexer.processed = event.payload.processed
  reindexer.total = event.payload.total
})

export function reindexPercent() {
  if (reindexer.total === 0) return 0
  return Math.min(100, Math.floor((reindexer.processed / reindexer.total) * 100))
}

export async function runReindex() {
  if (reindexer.running) return
  reindexer.running = true
  reindexer.processed = 0
  reindexer.total = 0
  reindexer.status = ''
  try {
    const count = await invoke('reindex_all')
    reindexer.status = `${count.toLocaleString()}件を再構築しました`
  } catch (e) {
    reindexer.status = `エラー: ${e}`
  } finally {
    reindexer.running = false
  }
}
