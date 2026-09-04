<script>
  // 本文はプレーンテキストのみ表示する。HTML本文はバックエンド側
  // (ferro_core::mail::parse::extract_plain_text_body)でタグを剥がした
  // テキストに変換済みなので、ここは<pre>にテキスト補間で描画するだけで済み、
  // HTMLとして解釈されることはない。
  import { invoke } from '@tauri-apps/api/core'
  import { save } from '@tauri-apps/plugin-dialog'

  let { messageId, onClose = () => {} } = $props()

  let detail = $state(null)
  let error = $state('')
  let saveStatus = $state({})

  $effect(() => {
    const id = messageId
    detail = null
    error = ''
    saveStatus = {}
    invoke('get_message_detail', { messageId: id })
      .then((d) => {
        detail = d
      })
      .catch((e) => {
        error = String(e)
      })
  })

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
  <button type="button" class="close" onclick={onClose}>Close</button>

  {#if error}
    <p class="error">{error}</p>
  {:else if detail === null}
    <p>Loading…</p>
  {:else}
    <h3>{detail.subject ?? '(no subject)'}</h3>
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
      <h4>Attachments ({detail.attachments.length})</h4>
      <ul class="attachments">
        {#each detail.attachments as attachment (attachment.index)}
          <li>
            <span class="filename">{attachment.filename ?? '(unnamed)'}</span>
            <span class="meta">{attachment.content_type ?? ''} · {formatSize(attachment.size)}</span>
            <button type="button" onclick={() => saveAttachment(attachment)}>Save…</button>
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
    border: 1px solid #ddd;
    border-radius: 4px;
    padding: 1rem;
    margin-top: 0.75rem;
  }
  .close {
    float: right;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.15rem 0.75rem;
    margin: 0.5rem 0;
  }
  dt {
    font-weight: 600;
    color: #555;
  }
  dd {
    margin: 0;
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
    padding: 0.25rem 0;
  }
  .filename {
    font-weight: 600;
  }
  .meta {
    color: #666;
    font-size: 0.85em;
  }
  .status {
    color: #555;
    font-size: 0.85em;
  }
  .body {
    white-space: pre-wrap;
    word-break: break-word;
    background: #fafafa;
    border: 1px solid #eee;
    border-radius: 4px;
    padding: 0.75rem;
    max-height: 320px;
    overflow-y: auto;
  }
  .error {
    color: #b00020;
  }
</style>
