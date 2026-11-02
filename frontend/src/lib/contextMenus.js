// 右クリックメニュー（`ContextMenu.svelte`）に出す項目を組み立てる。項目を選んだ時に何をするかは
// `run(command)`で呼び出し側（App.svelte）へ伝えるだけにし、ここは「どの項目を出すか」だけを持つ。
//
// command:
//   { type: 'bulk', action }  … `bulk_update`に渡す操作（形は`BulkActionBar`と同じ）
//   { type: 'open' } / { type: 'searchFrom' } / { type: 'copyFrom' } / { type: 'saveEml' }
// ヒントのキーはApp.svelteの`handleKeydown`・`KeyboardHelp.svelte`と揃えること。

// メニューを出す位置。マウスの位置が無い時（キーボードから開いた時は0,0になる）は
// 要素の左下に出す。
export function menuPosition(event, element) {
  if (event && (event.clientX !== 0 || event.clientY !== 0)) {
    return { x: event.clientX, y: event.clientY }
  }
  const rect = (element ?? event.currentTarget).getBoundingClientRect()
  return { x: rect.left + 16, y: rect.bottom }
}

function isSnoozed(message, nowSeconds) {
  return message.snoozed_until != null && message.snoozed_until > nowSeconds
}

// メッセージの行のメニュー。`messages`は操作の対象（右クリックした行か、選択中の全行）。
export function messageMenuItems({ messages, labels, run, nowSeconds = Date.now() / 1000 }) {
  if (messages.length === 0) return []
  const single = messages.length === 1
  const message = messages[0]
  const bulk = (action) => () => run({ type: 'bulk', action })
  const items = []

  if (single) {
    items.push({ label: '開く', hint: 'Enter', onSelect: () => run({ type: 'open' }) })
  } else {
    items.push({ heading: `${messages.length}件を選択中` })
  }
  items.push({ separator: true })

  // 既読・スターは、1件ならその行の状態を切り替える。複数なら、変える余地がある方を出す。
  const anyUnread = messages.some((m) => !m.is_read)
  const anyRead = messages.some((m) => m.is_read)
  if (anyUnread) items.push({ label: '既読にする', hint: single ? 'u' : undefined, onSelect: bulk({ kind: 'read', value: true }) })
  if (anyRead) items.push({ label: '未読にする', hint: single ? 'u' : undefined, onSelect: bulk({ kind: 'read', value: false }) })

  const anyUnflagged = messages.some((m) => !m.is_flagged)
  const anyFlagged = messages.some((m) => m.is_flagged)
  if (anyUnflagged) items.push({ label: 'スターを付ける', hint: single ? 's' : undefined, onSelect: bulk({ kind: 'flag', value: true }) })
  if (anyFlagged) items.push({ label: 'スターを外す', hint: single ? 's' : undefined, onSelect: bulk({ kind: 'flag', value: false }) })

  // 全部がアーカイブ済みなら「受信箱に戻す」、そうでなければ「アーカイブ」（`e`キーと同じ向き）。
  const allArchived = messages.every((m) => m.is_archived)
  items.push({
    label: allArchived ? '受信箱に戻す' : 'アーカイブ',
    hint: 'e',
    onSelect: bulk({ kind: 'archive', value: !allArchived }),
  })

  const snoozeItems = [
    { label: '1日後', onSelect: bulk({ kind: 'snooze', preset: '1d' }) },
    { label: '3日後', onSelect: bulk({ kind: 'snooze', preset: '3d' }) },
    { label: '1週間後', onSelect: bulk({ kind: 'snooze', preset: '1w' }) },
  ]
  if (messages.some((m) => isSnoozed(m, nowSeconds))) {
    snoozeItems.push({ separator: true }, { label: 'スヌーズ解除', onSelect: bulk({ kind: 'snooze', preset: null }) })
  }
  items.push({ label: 'スヌーズ', hint: single ? 'b' : undefined, submenu: snoozeItems })

  // ラベル: 全部に付いていればチェックを付け、選ぶと外す。そうでなければ選ぶと全部に付ける。
  const labelItems =
    labels.length === 0
      ? [{ label: 'ラベルがありません（サイドバーの＋で作成）', disabled: true }]
      : labels.map((label) => {
          const all = messages.every((m) => m.labels.some((l) => l.id === label.id))
          return {
            label: label.name,
            color: label.color,
            checked: all,
            onSelect: bulk({ kind: 'label', label_id: label.id, assigned: !all }),
          }
        })
  items.push({ label: 'ラベル', hint: single ? 'l' : undefined, submenu: labelItems })

  if (single) {
    items.push({ separator: true })
    if (message.from_addr) {
      items.push({ label: 'この差出人のメールを検索', onSelect: () => run({ type: 'searchFrom' }) })
      items.push({ label: '差出人のアドレスをコピー', onSelect: () => run({ type: 'copyFrom' }) })
    }
    items.push({ label: '.emlとして保存…', onSelect: () => run({ type: 'saveEml' }) })
  }

  // ゴミ箱の中のメッセージ（削除済み）には削除を出さない（戻す操作は今のところ無い）。
  if (messages.some((m) => !m.is_deleted)) {
    items.push({ separator: true })
    items.push({ label: '削除', hint: '#', danger: true, onSelect: bulk({ kind: 'delete' }) })
  }
  return items
}

// サイドバーのフォルダのメニュー。
export function folderMenuItems({ name, onOpen, onMarkAllRead }) {
  return [
    { heading: name },
    { label: '開く', onSelect: onOpen },
    { label: 'すべて既読にする…', hint: 'R', onSelect: onMarkAllRead },
  ]
}

// サイドバーのラベルのメニュー。
export function labelMenuItems({ label, onOpen, onMarkAllRead, onDelete }) {
  return [
    { heading: label.name },
    { label: '開く', onSelect: onOpen },
    { label: 'すべて既読にする…', hint: 'R', onSelect: onMarkAllRead },
    { separator: true },
    { label: 'ラベルを削除', danger: true, onSelect: onDelete },
  ]
}

// サイドバーの保存した検索のメニュー。
export function savedSearchMenuItems({ search, onOpen, onDelete }) {
  return [
    { heading: search.name },
    { label: '開く', onSelect: onOpen },
    { separator: true },
    { label: '保存した検索を削除', danger: true, onSelect: onDelete },
  ]
}
