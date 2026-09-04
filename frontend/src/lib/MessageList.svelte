<script>
  // 固定行高ウィンドウイング + 無限スクロールページングの自前仮想スクロール実装。
  // CLAUDE.mdの設計原則(「メッセージ一覧は仮想スクロール必須」)どおり、
  // 1000万件規模でもDOMには可視範囲分の行しか描画しない。
  import { invoke } from '@tauri-apps/api/core'

  let { accountId = null } = $props()

  const ROW_HEIGHT = 28
  const OVERSCAN_ROWS = 5
  const PAGE_SIZE = 100
  // 末尾からこのピクセル数以内に近づいたら次ページを先読みする。
  const PREFETCH_DISTANCE = ROW_HEIGHT * 20

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
      // date_headerの降順キーセットページネーション。ferro_core::db::messages::list_recent
      // と同じ設計(OFFSETではなくカーソル)なので、何ページ辿ってもO(log n)のまま。
      const before = items.length > 0 ? items[items.length - 1].date_header : null
      const page = await invoke('list_messages', { accountId, before, limit: PAGE_SIZE })
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

  // accountIdが変わったら(将来のアカウント別フィルタ等)最初から読み直す。
  // マウント時にも一度実行される。
  $effect(() => {
    accountId
    reset()
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

  function formatDate(unixSeconds) {
    return new Date(unixSeconds * 1000).toLocaleString()
  }
</script>

<div
  class="viewport"
  bind:clientHeight={viewportHeight}
  onscroll={onScroll}
>
  <div class="spacer" style="height: {totalHeight}px">
    <div class="window" style="transform: translateY({topOffset}px)">
      {#each visibleItems as message (message.id)}
        <div class="row" style="height: {ROW_HEIGHT}px">
          <span class="from" title={message.from_addr ?? ''}>
            {message.from_name ?? message.from_addr ?? '(unknown sender)'}
          </span>
          <span class="subject" title={message.subject ?? ''}>
            {message.subject ?? '(no subject)'}
          </span>
          <span class="date">{formatDate(message.date_header)}</span>
        </div>
      {/each}
    </div>
  </div>

  {#if items.length === 0 && !loading}
    <p class="empty">No messages yet.</p>
  {/if}
</div>

{#if loadError}
  <p class="error">{loadError}</p>
{/if}

<style>
  .viewport {
    height: 420px;
    overflow-y: auto;
    position: relative;
    border: 1px solid #ddd;
    border-radius: 4px;
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
    gap: 0.75rem;
    align-items: center;
    padding: 0 0.5rem;
    border-bottom: 1px solid #eee;
    box-sizing: border-box;
    overflow: hidden;
  }
  .from {
    flex: 0 0 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .subject {
    flex: 1 1 auto;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .date {
    flex: 0 0 auto;
    color: #666;
    font-size: 0.85em;
    white-space: nowrap;
  }
  .empty {
    padding: 1rem;
    color: #666;
  }
  .error {
    color: #b00020;
  }
</style>
