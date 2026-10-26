<script>
  // 本文はプレーンテキストのみ表示する。HTML本文はバックエンド側
  // (ferro_core::mail::parse::extract_plain_text_body)でタグを剥がした
  // テキストに変換済みなので、ここは<pre>にテキスト補間で描画するだけで済み、
  // HTMLとして解釈されることはない。
  import { invoke } from '@tauri-apps/api/core'
  import { save } from '@tauri-apps/plugin-dialog'

  // `onChanged(id, patch)`: 操作でメッセージの状態が変わったら、変わった項目だけを渡す
  // （一覧側はその行に当てるだけで済む。`messageView.js`参照）。削除・スヌーズ等で
  // 表示中のフォルダから外れた場合に次のメッセージへ進むか閉じるかは呼び出し側が決める。
  let {
    messageId,
    allLabels = [],
    markReadDelay = true,
    onClose = () => {},
    onChanged = () => {},
    // スレッドの別のメッセージを開く（一覧の選択もそのメッセージに移す）。
    onOpenMessage = () => {},
  } = $props()

  let detail = $state(null)
  let error = $state('')
  let saveStatus = $state({})
  let flagging = $state(false)
  let deleting = $state(false)
  let archiving = $state(false)
  let snoozing = $state(false)
  let showLabelPicker = $state(false)
  let showSnoozeMenu = $state(false)
  let emlStatus = $state('')
  // 本文の代わりに何を出すか（'body' | 'headers' | 'source'）。別のメッセージに移っても保つ
  // （同じ種類のアラートのヘッダーを続けて見比べられるように）。
  let viewMode = $state('body')
  let source = $state(null)
  let sourceError = $state('')
  // 「既読にするまでの時間」の待ち中なら、そのタイマー（`u`で未読に戻した時に止める）。
  let readTimer = null

  $effect(() => {
    const id = messageId
    detail = null
    error = ''
    saveStatus = {}
    emlStatus = ''
    source = null
    sourceError = ''
    showLabelPicker = false
    showSnoozeMenu = false
    flagging = false
    deleting = false
    archiving = false
    snoozing = false
    let cancelled = false

    invoke('get_message_detail', { messageId: id })
      .then((d) => {
        if (cancelled) return
        detail = d
        // 開いたら既読にする（一般的なメールクライアントの挙動に合わせる）。
        // 「既読にするまでの時間」設定がONなら2秒表示してから既読化する
        // （その前に閉じる/別メールへ切り替えたらタイマーはキャンセルする）。
        // 一覧側の表示更新はここでは強制しない（開くたびに一覧を作り直すと
        // スクロール位置が毎回リセットされて煩わしいため。次の自然な再読み込み
        // ―― 同期完了時など ―― で追いつく）。
        if (!d.is_read) {
          const markRead = async () => {
            readTimer = null
            try {
              await invoke('set_read', { messageId: id, isRead: true })
              onChanged(id, { is_read: true })
              if (!cancelled) detail = { ...detail, is_read: true }
            } catch {
              // 既読化の失敗は表示自体を妨げるものではないので無視する。
            }
          }
          if (markReadDelay) {
            readTimer = setTimeout(markRead, 2000)
          } else {
            markRead()
          }
        }
      })
      .catch((e) => {
        if (!cancelled) error = String(e)
      })

    return () => {
      cancelled = true
      if (readTimer) clearTimeout(readTimer)
      readTimer = null
    }
  })

  // ヘッダー・ソースは開いた時に取りに行く（本文だけ見る大多数の場合に余計な読み込みをしない）。
  $effect(() => {
    const id = messageId
    const mode = viewMode
    if (mode === 'body') return
    let cancelled = false
    source = null
    sourceError = ''
    invoke('get_message_source', { messageId: id, headersOnly: mode === 'headers' })
      .then((s) => {
        if (!cancelled) source = s
      })
      .catch((e) => {
        if (!cancelled) sourceError = String(e)
      })
    return () => {
      cancelled = true
    }
  })

  const VIEW_MODES = [
    ['body', '本文'],
    ['headers', 'ヘッダー'],
    ['source', 'ソース'],
  ]

  // 以下の`export`した操作は、ツールバーのボタンとApp.svelteのキーボード操作の両方から呼ぶ。

  // 本文 → ヘッダー → ソース → 本文 の順に切り替える。
  export function cycleView() {
    if (!detail) return
    const index = VIEW_MODES.findIndex(([mode]) => mode === viewMode)
    viewMode = VIEW_MODES[(index + 1) % VIEW_MODES.length][0]
  }

  export async function toggleFlag() {
    if (!detail || flagging) return
    flagging = true
    try {
      const next = !detail.is_flagged
      await invoke('set_flagged', { messageId, isFlagged: next })
      detail = { ...detail, is_flagged: next }
      onChanged(messageId, { is_flagged: next })
    } catch (e) {
      error = String(e)
    } finally {
      flagging = false
    }
  }

  export async function toggleArchive() {
    if (!detail || archiving) return
    archiving = true
    try {
      const next = !detail.is_archived
      await invoke('set_archived', { messageId, isArchived: next })
      detail = { ...detail, is_archived: next }
      onChanged(messageId, { is_archived: next })
    } catch (e) {
      error = String(e)
    } finally {
      archiving = false
    }
  }

  export async function toggleRead() {
    if (!detail) return
    if (readTimer) {
      clearTimeout(readTimer)
      readTimer = null
    }
    const next = !detail.is_read
    try {
      await invoke('set_read', { messageId, isRead: next })
      detail = { ...detail, is_read: next }
      onChanged(messageId, { is_read: next })
    } catch (e) {
      error = String(e)
    }
  }

  export function toggleSnoozeMenu() {
    if (!detail) return
    showLabelPicker = false
    showSnoozeMenu = !showSnoozeMenu
  }

  export function toggleLabelPicker() {
    if (!detail) return
    showSnoozeMenu = false
    showLabelPicker = !showLabelPicker
  }

  // スヌーズの期限そのものはRust側(`set_snoozed`)が計算する。一覧の行に当てる値は
  // 「今より未来」でさえあればよい（受信箱/スヌーズのどちらに属するかの判定だけに使う）。
  const SNOOZE_SECONDS = { '1d': 24 * 60 * 60, '3d': 3 * 24 * 60 * 60, '1w': 7 * 24 * 60 * 60 }

  async function snooze(preset) {
    showSnoozeMenu = false
    snoozing = true
    try {
      await invoke('set_snoozed', { messageId, preset })
      onChanged(messageId, { snoozed_until: Math.floor(Date.now() / 1000) + SNOOZE_SECONDS[preset] })
    } catch (e) {
      error = String(e)
    } finally {
      snoozing = false
    }
  }

  async function toggleLabel(label) {
    if (!detail) return
    const has = detail.labels.some((l) => l.id === label.id)
    try {
      await invoke('set_message_label', { messageId, labelId: label.id, assigned: !has })
      const labels = has
        ? detail.labels.filter((l) => l.id !== label.id)
        : [...detail.labels, { id: label.id, name: label.name, color: label.color }]
      detail = { ...detail, labels }
      onChanged(messageId, { labels })
    } catch (e) {
      error = String(e)
    }
  }

  export async function deleteMessage() {
    if (!detail || deleting) return
    deleting = true
    try {
      await invoke('set_deleted', { messageId, isDeleted: true })
      onChanged(messageId, { is_deleted: true })
    } catch (e) {
      error = String(e)
    } finally {
      deleting = false
    }
  }

  function formatDate(unixSeconds) {
    return new Date(unixSeconds * 1000).toLocaleString()
  }

  function formatSize(bytes) {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  // 生メールそのもの（Maildirに保存しているバイト列）を.emlとして書き出す。
  // 他のメーラーで開いたり、ヘッダーを確認したりするのに使う。
  async function saveEml() {
    if (!detail) return
    emlStatus = ''
    try {
      const destination = await save({
        defaultPath: detail.eml_filename,
        filters: [{ name: 'メール (.eml)', extensions: ['eml'] }],
      })
      if (!destination) return
      emlStatus = '保存中…'
      await invoke('save_message_eml', { messageId, destinationPath: destination })
      emlStatus = '保存しました'
    } catch (e) {
      emlStatus = `エラー: ${e}`
    }
  }

  async function saveAttachment(attachment) {
    saveStatus = { ...saveStatus, [attachment.index]: 'saving…' }
    try {
      const destination = await save({ defaultPath: attachment.filename ?? 'attachment' })
      if (!destination) {
        saveStatus = { ...saveStatus, [attachment.index]: '' }
        return
      }
      await invoke('save_attachment', {
        messageId,
        attachmentIndex: attachment.index,
        destinationPath: destination,
      })
      saveStatus = { ...saveStatus, [attachment.index]: 'saved' }
    } catch (e) {
      saveStatus = { ...saveStatus, [attachment.index]: `error: ${e}` }
    }
  }
</script>

<div class="detail">
  <button type="button" class="close" onclick={onClose}>閉じる</button>

  {#if error}
    <p class="error">{error}</p>
  {:else if detail === null}
    <p>読み込み中…</p>
  {:else}
    <div class="toolbar">
      <button type="button" onclick={toggleArchive} disabled={archiving}>
        {detail.is_archived ? 'アーカイブ解除' : 'アーカイブ'}
      </button>
      <div class="menu-wrap">
        <button type="button" onclick={() => (showSnoozeMenu = !showSnoozeMenu)} disabled={snoozing}>
          スヌーズ
        </button>
        {#if showSnoozeMenu}
          <div class="menu">
            <button type="button" onclick={() => snooze('1d')}>1日後</button>
            <button type="button" onclick={() => snooze('3d')}>3日後</button>
            <button type="button" onclick={() => snooze('1w')}>1週間後</button>
          </div>
        {/if}
      </div>
      <div class="menu-wrap">
        <button type="button" onclick={() => (showLabelPicker = !showLabelPicker)}>ラベル</button>
        {#if showLabelPicker}
          <div class="menu">
            {#if allLabels.length === 0}
              <span class="menu-empty">ラベルがありません</span>
            {/if}
            {#each allLabels as label (label.id)}
              <button type="button" class="label-option" onclick={() => toggleLabel(label)}>
                <span class="dot" style="background: {label.color};"></span>
                <span class="flex">{label.name}</span>
                {#if detail.labels.some((l) => l.id === label.id)}✓{/if}
              </button>
            {/each}
          </div>
        {/if}
      </div>
      <button type="button" onclick={toggleFlag} disabled={flagging}>
        {detail.is_flagged ? 'スター解除' : 'スター'}
      </button>
      <button type="button" onclick={toggleRead}>
        {detail.is_read ? '未読にする' : '既読にする'}
      </button>
      <button type="button" onclick={saveEml}>.emlとして保存…</button>
      <button type="button" class="danger" onclick={deleteMessage} disabled={deleting}>削除</button>
      {#if emlStatus}
        <span class="status toolbar-status">{emlStatus}</span>
      {/if}
    </div>

    <h3>{detail.subject ?? '(no subject)'}</h3>
    {#if detail.labels.length > 0}
      <div class="chips">
        {#each detail.labels as label (label.id)}
          <span class="chip" style="background: {label.color}22; color: {label.color};">{label.name}</span>
        {/each}
      </div>
    {/if}
    <dl>
      <dt>From</dt>
      <dd>{detail.from_name ?? ''} &lt;{detail.from_addr ?? 'unknown'}&gt;</dd>
      {#if detail.to_addr}
        <dt>To</dt>
        <dd>{detail.to_addr}</dd>
      {/if}
      <dt>Date</dt>
      <dd>{formatDate(detail.date_header)}</dd>
    </dl>

    {#if detail.attachments.length > 0}
      <h4>添付ファイル ({detail.attachments.length})</h4>
      <ul class="attachments">
        {#each detail.attachments as attachment (attachment.index)}
          <li>
            <span class="filename">{attachment.filename ?? '(unnamed)'}</span>
            <span class="meta">{attachment.content_type ?? ''} · {formatSize(attachment.size)}</span>
            <button type="button" onclick={() => saveAttachment(attachment)}>保存…</button>
            {#if saveStatus[attachment.index]}
              <span class="status">{saveStatus[attachment.index]}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if detail.thread.length > 1}
      <details class="thread" open>
        <summary>このスレッドのメッセージ（{detail.thread.length}件）</summary>
        <ol>
          {#each detail.thread as item (item.id)}
            <li>
              <button
                type="button"
                class="thread-item"
                class:current={item.id === detail.id}
                class:unread={!item.is_read}
                disabled={item.id === detail.id}
                onclick={() => onOpenMessage(item.id)}
              >
                <span class="thread-from">{item.from_name ?? item.from_addr ?? '(unknown sender)'}</span>
                <span class="thread-date">{formatDate(item.date_header)}</span>
                <span class="thread-preview">{item.preview ?? item.subject ?? ''}</span>
              </button>
            </li>
          {/each}
        </ol>
      </details>
    {/if}

    <div class="view-tabs" role="tablist">
      {#each VIEW_MODES as [mode, label] (mode)}
        <button
          type="button"
          role="tab"
          aria-selected={viewMode === mode}
          class:active={viewMode === mode}
          onclick={() => (viewMode = mode)}>{label}</button
        >
      {/each}
    </div>
    {#if viewMode === 'body'}
      <pre class="body">{detail.body ?? '(no body)'}</pre>
    {:else if sourceError}
      <p class="error">{sourceError}</p>
    {:else if source === null}
      <p class="status">読み込み中…</p>
    {:else}
      {#if source.truncated}
        <p class="status">
          メール全体が{formatSize(source.total_bytes)}あるため、先頭だけを表示しています。全体は「.emlとして保存…」で確認できます。
        </p>
      {/if}
      <pre class="body source">{source.text}</pre>
    {/if}
  {/if}
</div>

<style>
  .detail {
    height: 100%;
    box-sizing: border-box;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.25rem;
    background: var(--surface);
  }
  .close {
    float: right;
    border: none;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
  }
  .close:hover {
    color: var(--text);
  }
  .toolbar {
    display: flex;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
  }
  .toolbar button {
    font-size: 12px;
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 6px 12px;
    background: var(--surface-subtle);
    cursor: pointer;
  }
  .toolbar button:hover {
    background: var(--surface-muted);
  }
  .toolbar button.danger {
    color: var(--danger);
  }
  .menu-wrap {
    position: relative;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 20px -8px rgba(28, 27, 24, 0.35);
    display: flex;
    flex-direction: column;
    min-width: 160px;
    padding: 4px;
  }
  .menu button,
  .label-option {
    text-align: left;
    border: none;
    background: none;
    padding: 7px 10px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--text-secondary);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .menu button:hover,
  .label-option:hover {
    background: var(--surface-muted);
  }
  .menu-empty {
    padding: 7px 10px;
    color: var(--text-faint);
    font-size: 12px;
  }
  .label-option .dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
  }
  .label-option .flex {
    flex: 1;
  }
  h3 {
    font-size: calc(22px * var(--content-font-scale, 1));
  }
  .chips {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }
  .chip {
    font-size: 11px;
    padding: 3px 9px;
    border-radius: 5px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.15rem 0.75rem;
    margin: 0.75rem 0;
    font-size: calc(13px * var(--content-font-scale, 1));
  }
  dt {
    font-weight: 600;
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    color: var(--text-secondary);
  }
  .attachments {
    list-style: none;
    padding: 0;
    margin: 0.5rem 0;
  }
  .attachments li {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    padding: 0.4rem 0;
    border-bottom: 1px solid var(--border);
  }
  .filename {
    font-weight: 500;
  }
  .meta {
    color: var(--text-muted);
    font-size: 0.85em;
  }
  .status {
    color: var(--text-muted);
    font-size: 0.85em;
  }
  .toolbar-status {
    align-self: center;
  }
  .thread {
    margin: 0.5rem 0 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-subtle);
    font-size: 12.5px;
  }
  .thread summary {
    padding: 6px 10px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .thread ol {
    list-style: none;
    margin: 0;
    padding: 0 4px 4px;
    max-height: 240px;
    overflow-y: auto;
  }
  .thread-item {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 0 12px;
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text-secondary);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .thread-item:hover:not(:disabled) {
    background: var(--surface-muted);
  }
  .thread-item.current {
    background: var(--accent-soft-bg);
    cursor: default;
  }
  .thread-item.unread .thread-from {
    font-weight: 700;
  }
  .thread-from,
  .thread-preview {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .thread-date {
    color: var(--text-faint);
    font-size: 11px;
  }
  .thread-preview {
    grid-column: 1 / -1;
    color: var(--text-muted);
    font-size: 12px;
  }
  .body {
    white-space: pre-wrap;
    word-break: break-word;
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.9rem;
    font-size: calc(13.5px * var(--content-font-scale, 1));
    line-height: 1.8;
    color: var(--text);
  }
  .error {
    color: var(--danger);
  }
  .view-tabs {
    display: flex;
    gap: 4px;
    margin: 0.75rem 0 0.5rem;
  }
  .view-tabs button {
    font: inherit;
    font-size: 12px;
    padding: 4px 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
  }
  .view-tabs button:hover {
    background: var(--surface-muted);
  }
  .view-tabs button.active {
    background: var(--accent-soft-bg);
    border-color: var(--accent);
    color: var(--text);
  }
  .body.source {
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    font-size: calc(12px * var(--content-font-scale, 1));
    line-height: 1.5;
    word-break: break-all;
  }
</style>
