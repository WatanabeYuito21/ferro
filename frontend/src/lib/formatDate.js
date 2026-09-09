// メッセージ一覧・検索結果一覧で共通の受信日時表示（MessageList.svelte/App.svelte）。
// 日付だけだと同じ日に届いた大量のアラートメール等の前後関係が分からないため、
// 常に日付と時刻の両方を表示する。
export function formatDate(unixSeconds) {
  const d = new Date(unixSeconds * 1000)
  const date = d.toLocaleDateString()
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  return `${date} ${time}`
}
