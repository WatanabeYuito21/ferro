<script>
  // 固定行高ウィンドウイング + 無限スクロールページングの自前仮想スクロール実装。
  // CLAUDE.mdの設計原則(「メッセージ一覧は仮想スクロール必須」)どおり、
  // 1000万件規模でもDOMには可視範囲分の行しか描画しない。
  import { invoke } from '@tauri-apps/api/core'
  import { untrack } from 'svelte'
  import { matchColor } from './colorRules.js'
  import { formatDate } from './formatDate.js'

  // labelIdが指定されているときはfolderを無視してラベル一覧を表示する
  // （サイドバーはフォルダかラベルのどちらか一方を選ぶ設計）。
  let { folder = 'inbox', labelId = null, colorRules = [], onSelect = () => {} } = $props()

  const ROW_HEIGHT = 68
  const OVERSCAN_ROWS = 5
  const PAGE_SIZE = 100
  // 末尾からこのピクセル数以内に近づいたら次ページを先読みする。
  const PREFETCH_DISTANCE = ROW_HEIGHT * 10

  let items = $state([])
  let exhausted = $state(false)
  let loading = $state(false)
  let loadError = $state('')
  let scrollTop = $state(0)
  let viewportHeight = $state(0)

  async function loadMore() {
    if (loading || exhausted) return
    loading = true
    try {
      // date_headerの降順キーセットページネーション。ferro_core::db::messages
      // と同じ設計(OFFSETではなくカーソル)なので、何ページ辿ってもO(log n)のまま。
      const before = items.length > 0 ? items[items.length - 1].date_header : null
      const page =
        labelId != null
          ? await invoke('list_messages_by_label', { labelId, before, limit: PAGE_SIZE })
          : await invoke('list_messages_by_folder', {
              folder,
              accountId: null,
              before,
              limit: PAGE_SIZE,
            })
      if (page.length === 0) {
        exhausted = true
      } else {
        items = [...items, ...page]
        if (page.length < PAGE_SIZE) exhausted = true
      }
    } catch (e) {
      loadError = String(e)
    } finally {
      loading = false
    }
  }

  function reset() {
    items = []
    exhausted = false
    loadError = ''
    loadMore()
  }

  // folder/labelIdが変わったら最初から読み直す。マウント時にも一度実行される。
  //
  // `reset()`/`loadMore()`はitems/loading/exhausted($state)を読み書きするが、
  // それをこのeffect自身の依存として拾わせてはいけない。untrackで包まないと、
  // 「読んだ状態をこのeffectの中で書き換える」→Svelteがそれを依存の変化とみなして
  // このeffectを再実行→reset()が再度items=[]で表示中データを消す、という無限
  // ループになる（実際にこれで一覧が常に空に見えるバグを踏んだ）。
  $effect(() => {
    folder
    labelId
    untrack(() => reset())
  })

  function onScroll(event) {
    scrollTop = event.target.scrollTop
    const totalHeight = items.length * ROW_HEIGHT
    if (totalHeight - scrollTop - viewportHeight < PREFETCH_DISTANCE) {
      loadMore()
    }
  }

  let startIndex = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN_ROWS))
  let endIndex = $derived(
    Math.min(items.length, Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN_ROWS)
  )
  let visibleItems = $derived(items.slice(startIndex, endIndex))
  let topOffset = $derived(startIndex * ROW_HEIGHT)
  let totalHeight = $derived(items.length * ROW_HEIGHT)

</script>

<div class="viewport" bind:clientHeight={viewportHeight} onscroll={onScroll}>
  <div class="spacer" style="height: {totalHeight}px">
    <div class="window" style="transform: translateY({topOffset}px)">
      {#each visibleItems as message (message.id)}
        {@const rowColor = matchColor(colorRules, message)}
        <div
          class="row"
          class:unread={!message.is_read}
          style="height: {ROW_HEIGHT}px"
          role="button"
          tabindex="0"
          onclick={() => onSelect(message.id)}
          onkeydown={(e) => e.key === 'Enter' && onSelect(message.id)}
        >
          <div class="line1">
            {#if message.is_flagged}<span class="star">★</span>{/if}
            <span class="from" title={message.from_addr ?? ''} style={rowColor ? `color: ${rowColor}` : ''}>
              {message.from_name ?? message.from_addr ?? '(unknown sender)'}
            </span>
            <span class="date">{formatDate(message.date_header)}</span>
          </div>
          <div class="subject" title={message.subject ?? ''} style={rowColor ? `color: ${rowColor}` : ''}>
            {message.subject ?? '(no subject)'}
          </div>
          {#if message.preview}
            <div class="preview">{message.preview}</div>
          {/if}
          {#if message.labels.length > 0 || message.attachment_count > 0}
            <div class="chips">
              {#each message.labels as label (label.id)}
                <span class="chip label-chip" style="background: {label.color}22; color: {label.color};">
                  {label.name}
                </span>
              {/each}
              {#if message.attachment_count > 0}
                <span class="chip attachment-chip">添付 {message.attachment_count}</span>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </div>

  {#if items.length === 0 && !loading}
    <p class="empty">メッセージはありません</p>
  {/if}
</div>

{#if loadError}
  <p class="error">{loadError}</p>
{/if}

<style>
  .viewport {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    position: relative;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-subtle);
  }
  .spacer {
    position: relative;
  }
  .window {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
  }
  .row {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 3px;
    padding: 0 16px;
    border-bottom: 1px solid #efede5;
    box-sizing: border-box;
    overflow: hidden;
    cursor: pointer;
  }
  .row:hover {
    background: #f4f2eb;
  }
  .line1 {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .star {
    color: #d4a017;
    font-size: 12px;
    flex: none;
  }
  .from {
    font-size: 13px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .date {
    font-size: 11px;
    color: var(--text-faint);
    flex: none;
  }
  .subject {
    font-size: 13px;
    margin-left: 14px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .preview {
    font-size: 12px;
    color: var(--text-muted);
    margin-left: 14px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.unread .from,
  .row.unread .subject {
    font-weight: 700;
  }
  .chips {
    display: flex;
    gap: 6px;
    margin: 2px 0 0 14px;
    overflow: hidden;
  }
  .chip {
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: 5px;
    white-space: nowrap;
    flex: none;
  }
  .attachment-chip {
    background: var(--surface-muted);
    color: var(--text-muted);
  }
  .empty {
    padding: 1rem;
    color: var(--text-muted);
  }
  .error {
    color: var(--danger);
  }
</style>
