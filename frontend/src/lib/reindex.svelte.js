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

// 設定画面の「インデックスの状態」。全件のCOUNTを取るので常時ポーリングはせず、
// 設定画面を開いた時・「再確認」を押した時・再構築が終わった時にだけ取り直す。
export const indexStatus = $state({
  // null = まだ取得していない
  data: null,
  loading: false,
  error: '',
})

export async function refreshIndexStatus() {
  if (indexStatus.loading) return
  indexStatus.loading = true
  try {
    indexStatus.data = await invoke('get_search_index_status')
    indexStatus.error = ''
  } catch (e) {
    indexStatus.error = String(e)
  } finally {
    indexStatus.loading = false
  }
}

// 状態から、利用者向けの一言を作る（`kind`は表示の色分け用）。
export function describeIndexStatus(status) {
  if (status.rebuilding) {
    return { kind: 'info', text: '再構築中です。' }
  }
  if (status.index_docs < status.indexed) {
    return {
      kind: 'warn',
      text:
        '索引の文書数がDBの記録より少なくなっています。アプリを再起動すると自動的に' +
        '全件の再投入が始まります（すぐ直したい場合は再構築してください）。',
    }
  }
  const missing = status.total - status.indexed
  if (missing > 0) {
    return {
      kind: 'warn',
      text:
        `${missing.toLocaleString()}件がまだ検索できません。バックグラウンドで自動的に拾い直します` +
        '（しばらく経っても減らない場合は再構築してください）。',
    }
  }
  return { kind: 'ok', text: 'すべてのメッセージを検索できます。' }
}

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
    refreshIndexStatus()
  }
}
