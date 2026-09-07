// メッセージ一覧・検索結果一覧で共通の受信日時表示（MessageList.svelte/App.svelte）。
// 今日なら時刻のみ、それ以外は日付のみ表示する。
export function formatDate(unixSeconds) {
  const d = new Date(unixSeconds * 1000)
  const now = new Date()
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }
  return d.toLocaleDateString()
}
