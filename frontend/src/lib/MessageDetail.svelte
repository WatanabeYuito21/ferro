<script>
  // 本文はプレーンテキストのみ表示する。HTML本文はバックエンド側
  // (ferro_core::mail::parse::extract_plain_text_body)でタグを剥がした
  // テキストに変換済みなので、ここは<pre>にテキスト補間で描画するだけで済み、
  // HTMLとして解釈されることはない。
  import { invoke } from '@tauri-apps/api/core'
  import { save } from '@tauri-apps/plugin-dialog'

  let {
    messageId,
    allLabels = [],
    markReadDelay = true,
    onClose = () => {},
    onChanged = () => {},
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

  $effect(() => {
    const id = messageId
    detail = null
    error = ''
    saveStatus = {}
    showLabelPicker = false
    showSnoozeMenu = false
    let cancelled = false
    let readTimer = null

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
            try {
              await invoke('set_read', { messageId: id, isRead: true })
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
    }
  })

  async function toggleFlag() {
    if (!detail) return
    flagging = true
    try {
      const next = !detail.is_flagged
      await invoke('set_flagged', { messageId, isFlagged: next })
      detail = { ...detail, is_flagged: next }
      onChanged()
    } catch (e) {
      error = String(e)
    } finally {
      flagging = false
    }
  }

  async function toggleArchive() {
    if (!detail) return
    archiving = true
    try {
      const next = !detail.is_archived
      await invoke('set_archived', { messageId, isArchived: next })
      detail = { ...detail, is_archived: next }
      onChanged()
    } catch (e) {
      error = String(e)
    } finally {
      archiving = false
    }
  }

  async function snooze(preset) {
    showSnoozeMenu = false
    snoozing = true
    try {
      await invoke('set_snoozed', { messageId, preset })
      onChanged()
      onClose()
    } catch (e) {
      error = String(e)
      snoozing = false
    }
  }

  async function toggleLabel(label) {
    if (!detail) return
    const has = detail.labels.some((l) => l.id === label.id)
    try {
      await invoke('set_message_label', { messageId, labelId: label.id, assigned: !has })
      detail = {
        ...detail,
        labels: has
          ? detail.labels.filter((l) => l.id !== label.id)
          : [...detail.labels, label],
      }
      onChanged()
    } catch (e) {
      error = String(e)
    }
  }

  async function deleteMessage() {
    deleting = true
    try {
      await invoke('set_deleted', { messageId, isDeleted: true })
      onChanged()
      onClose()
    } catch (e) {
      error = String(e)
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
      <button type="button" class="danger" onclick={deleteMessage} disabled={deleting}>削除</button>
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

    <pre class="body">{detail.body ?? '(no body)'}</pre>
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
</style>
