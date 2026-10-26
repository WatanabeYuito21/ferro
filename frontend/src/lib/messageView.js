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
// 戻り値: { items, removed, nextId }
export function applyPatch(items, id, patch, view) {
  const index = items.findIndex((m) => m.id === id)
  if (index === -1) return { items, removed: false, nextId: id }
  const updated = { ...items[index], ...patch }
  if (belongsToView(updated, view)) {
    const next = items.slice()
    next[index] = updated
    return { items: next, removed: false, nextId: id }
  }
  const next = items.slice(0, index).concat(items.slice(index + 1))
  const neighbor = next[index] ?? next[index - 1] ?? null
  return { items: next, removed: true, nextId: neighbor ? neighbor.id : null }
}

// `id`の行から`delta`行動いた先の行のid。`id`が一覧に無ければ先頭の行。
export function neighborId(items, id, delta) {
  if (items.length === 0) return null
  const index = items.findIndex((m) => m.id === id)
  if (index === -1) return items[0].id
  const target = Math.min(items.length - 1, Math.max(0, index + delta))
  return items[target].id
}
