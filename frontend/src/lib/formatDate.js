// メッセージ一覧・検索結果一覧で共通の受信日時表示（MessageList.svelte/App.svelte）。
// 日付だけだと同じ日に届いた大量のアラートメール等の前後関係が分からないため、
// 常に日付と時刻の両方を表示する。
export function formatDate(unixSeconds) {
  const d = new Date(unixSeconds * 1000)
  const date = d.toLocaleDateString()
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  return `${date} ${time}`
}

// 「5分前」のような相対表示（アカウントの最終同期等）。1日以上前は日時で出す。
export function formatRelative(unixSeconds, nowMs = Date.now()) {
  const elapsed = Math.max(0, Math.floor(nowMs / 1000) - unixSeconds)
  if (elapsed < 60) return 'たった今'
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}分前`
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}時間前`
  return formatDate(unixSeconds)
}
