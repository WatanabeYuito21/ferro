// 一覧に表示中のメッセージへ操作（スター・アーカイブ・削除・スヌーズ・ラベル・既読）を
// 反映するための共通処理。以前は操作のたびに一覧を最初のページから作り直していたため、
// スクロール位置が失われ、キーボードで次々に処理していく使い方ができなかった。
// 今は変更内容(patch)を一覧の該当行へ当てるだけにし、表示中のフォルダ/ラベルの条件から
// 外れた行だけを取り除く。

// フォルダ/ラベルの絞り込み条件。`ferro_core::db::messages`の`list_by_folder`/
// `list_by_label`のWHERE句と同じ判定をフロント側で行う（ずれると、操作した行が
// 一覧に残る/消えるの判断が再読み込み後の表示と食い違う）。
export function belongsToView(message, view, nowSeconds = Date.now() / 1000) {
  if (view.kind === 'search') {
    // 検索結果は削除されたものだけ消す（検索はフォルダで絞り込んでいない）。
    return !message.is_deleted
  }
  if (view.kind === 'label') {
    return !message.is_deleted && message.labels.some((label) => label.id === view.labelId)
  }
  const snoozed = message.snoozed_until != null && message.snoozed_until > nowSeconds
  switch (view.folder) {
    case 'inbox':
      return !message.is_deleted && !message.is_archived && !snoozed
    case 'starred':
      return !message.is_deleted && message.is_flagged
    case 'snoozed':
      return !message.is_deleted && snoozed
    case 'archive':
      return !message.is_deleted && message.is_archived
    case 'trash':
      return message.is_deleted
    default:
      return true
  }
}

// `items`の中で`id`の行を`patch`で更新する。表示条件から外れたら取り除き、
// 代わりに選ぶべき行のid（直後の行、末尾なら直前の行、無ければnull）を返す。
// `patch`は変更する項目のオブジェクトか、行を受け取ってそれを返す関数
// （一括でラベルを付け外しする時のように、行ごとに値が違う場合）。
// 戻り値: { items, removed, nextId }
export function applyPatch(items, id, patch, view) {
  const index = items.findIndex((m) => m.id === id)
  if (index === -1) return { items, removed: false, nextId: id }
  const changes = typeof patch === 'function' ? patch(items[index]) : patch
  const updated = { ...items[index], ...changes }
  if (belongsToView(updated, view)) {
    const next = items.slice()
    next[index] = updated
    return { items: next, removed: false, nextId: id }
  }
  const next = items.slice(0, index).concat(items.slice(index + 1))
  const neighbor = next[index] ?? next[index - 1] ?? null
  return { items: next, removed: true, nextId: neighbor ? neighbor.id : null }
}

// 行のクリックを複数選択として解釈する（Ctrl/⌘+クリックで1件ずつ、Shift+クリックで
// 範囲を追加。修飾キー無しのクリックは選択を解除してその行を開く）。
// `checked`はチェック済みidのSet、`anchor`は範囲選択の起点（直前にクリックした行）。
// 戻り値: { checked, anchor, open }（`open`がtrueならその行を開く）
export function clickSelection(items, checked, anchor, id, event) {
  if (event.shiftKey && anchor != null) {
    const from = items.findIndex((m) => m.id === anchor)
    const to = items.findIndex((m) => m.id === id)
    if (from !== -1 && to !== -1) {
      const next = new Set(checked)
      for (let i = Math.min(from, to); i <= Math.max(from, to); i++) next.add(items[i].id)
      return { checked: next, anchor, open: false }
    }
  }
  if (event.shiftKey || event.ctrlKey || event.metaKey) {
    return { checked: toggleChecked(checked, id), anchor: id, open: false }
  }
  return { checked: new Set(), anchor: id, open: true }
}

export function toggleChecked(checked, id) {
  const next = new Set(checked)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  return next
}

// `id`の行から`delta`行動いた先の行のid。`id`が一覧に無ければ先頭の行。
export function neighborId(items, id, delta) {
  if (items.length === 0) return null
  const index = items.findIndex((m) => m.id === id)
  if (index === -1) return items[0].id
  const target = Math.min(items.length - 1, Math.max(0, index + delta))
  return items[target].id
}
