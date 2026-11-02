<script>
  // フォルダ(受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱)とラベルの一覧。
  // 状態(件数・選択中フォルダ等)はApp.svelte側で持ち、ここは表示とラベル作成
  // フォームのローカル状態(未送信の入力内容)だけを持つ。
  import { LABEL_COLORS } from './labelColors.js'
  import {
    SIDEBAR_DEFAULT_WIDTH,
    SIDEBAR_MIN_WIDTH,
    SIDEBAR_MAX_WIDTH,
    clampSidebarWidth,
  } from './sidebarWidth.js'

  let {
    folderCounts = { inbox: 0, starred: 0, snoozed: 0, archive: 0, trash: 0 },
    labels = [],
    selectedFolder = 'inbox',
    selectedLabelId = null,
    // 保存した検索（{ name, query, unread }）。`activeSavedQuery`は今表示中の検索の文字列
    // （検索していなければnull）で、一致する行を選択中として出す。
    savedSearches = [],
    activeSavedQuery = null,
    onSelectSavedSearch = () => {},
    onDeleteSavedSearch = async () => {},
    onSelectFolder = () => {},
    onSelectLabel = () => {},
    onCreateLabel = async () => {},
    onDeleteLabel = async () => {},
    onOpenSettings = () => {},
    // 右クリック（メニューはApp.svelteが出す）。フォルダはキー、ラベルは行、保存した検索は添字を渡す。
    onFolderContextMenu = () => {},
    onLabelContextMenu = () => {},
    onSavedSearchContextMenu = () => {},
    // 折りたたみ中は、開く・設定のボタンだけの細い帯にする（状態はApp.svelteが持つ）。
    collapsed = false,
    onToggleCollapsed = () => {},
    // 幅（App.svelteが持ち、localStorageに覚える）。右端のつまみのドラッグ・矢印キーで変え、
    // `onResize(幅, 確定したか)`で伝える。ドラッグ中はfalse、離した時にtrue。
    width = SIDEBAR_DEFAULT_WIDTH,
    onResize = () => {},
  } = $props()

  const RESIZE_STEP = 16

  // ドラッグ開始時のポインタ位置と幅（ドラッグしていなければnull）。
  let dragStart = $state(null)

  function startResize(event) {
    if (event.button !== 0) return
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    dragStart = { x: event.clientX, width }
    // ドラッグ中に一覧や本文の文字が選択されないように。
    document.body.classList.add('resizing-sidebar')
  }

  function moveResize(event) {
    if (!dragStart) return
    onResize(clampSidebarWidth(dragStart.width + event.clientX - dragStart.x), false)
  }

  function endResize() {
    if (!dragStart) return
    dragStart = null
    document.body.classList.remove('resizing-sidebar')
    onResize(width, true)
  }

  function resizeByKey(event) {
    const next = {
      ArrowLeft: width - RESIZE_STEP,
      ArrowRight: width + RESIZE_STEP,
      Home: SIDEBAR_MIN_WIDTH,
      End: SIDEBAR_MAX_WIDTH,
    }[event.key]
    if (next === undefined) return
    // アプリのショートカット（windowで受けている）へ渡さない。
    event.preventDefault()
    event.stopPropagation()
    onResize(clampSidebarWidth(next), true)
  }

  const FOLDERS = [
    { key: 'inbox', name: '受信箱' },
    { key: 'starred', name: 'スター付き' },
    { key: 'snoozed', name: 'スヌーズ' },
    { key: 'archive', name: 'アーカイブ' },
    { key: 'trash', name: 'ゴミ箱' },
  ]

  let creatingLabel = $state(false)
  let newLabelName = $state('')
  let newLabelColor = $state(LABEL_COLORS[0])
  let createError = $state('')

  function folderCount(key) {
    return folderCounts[key] ?? 0
  }

  // 検索中は通常のフォルダ/ラベルを選択中として出さない（保存した検索の方を出す）。
  let searching = $derived(activeSavedQuery !== null)

  // ブラウザ標準のメニューの代わりにApp.svelte側のメニューを出す。
  function contextMenu(handler, ...args) {
    return (event) => {
      event.preventDefault()
      handler(event, ...args)
    }
  }

  function unreadLabel(count) {
    return count >= 1000 ? '999+' : String(count)
  }

  async function submitNewLabel() {
    if (!newLabelName.trim()) return
    createError = ''
    try {
      await onCreateLabel({ name: newLabelName.trim(), color: newLabelColor })
      newLabelName = ''
      newLabelColor = LABEL_COLORS[0]
      creatingLabel = false
    } catch (e) {
      createError = String(e)
    }
  }
</script>

{#if collapsed}
<aside class="sidebar collapsed">
  <button
    type="button"
    class="collapse-toggle"
    title="サイドバーを開く（[）"
    aria-label="サイドバーを開く"
    onclick={onToggleCollapsed}>»</button
  >
  <button
    type="button"
    class="settings-link rail-settings"
    title="設定"
    aria-label="設定"
    onclick={onOpenSettings}>⚙</button
  >
</aside>
{:else}
<aside class="sidebar" style="width: {width}px;">
  <div class="sidebar-top">
    <button
      type="button"
      class="collapse-toggle"
      title="サイドバーを折りたたむ（[）"
      aria-label="サイドバーを折りたたむ"
      onclick={onToggleCollapsed}>«</button
    >
  </div>
  <div class="folders">
    {#each FOLDERS as folder (folder.key)}
      <button
        type="button"
        class="row"
        class:active={!searching && selectedFolder === folder.key && selectedLabelId === null}
        onclick={() => onSelectFolder(folder.key)}
        oncontextmenu={contextMenu(onFolderContextMenu, folder.key)}
      >
        <span class="dot" class:dot-visible={!searching && selectedFolder === folder.key && selectedLabelId === null}></span>
        <span class="name">{folder.name}</span>
        <span class="count">{folderCount(folder.key)}</span>
      </button>
    {/each}
  </div>

  {#if savedSearches.length > 0}
    <div class="section-header">
      <span>保存した検索</span>
    </div>
    <div class="labels">
      {#each savedSearches as search, index (search.name)}
        <div
          class="row label-row"
          class:active={activeSavedQuery === search.query}
          role="group"
          oncontextmenu={contextMenu(onSavedSearchContextMenu, index)}
        >
          <button type="button" class="row-main" title={search.query} onclick={() => onSelectSavedSearch(index)}>
            <span class="search-icon" aria-hidden="true">⌕</span>
            <span class="name">{search.name}</span>
            {#if search.unread > 0}
              <span class="count">{unreadLabel(search.unread)}</span>
            {/if}
          </button>
          <button
            type="button"
            class="delete-label"
            title="保存した検索を削除"
            onclick={() => onDeleteSavedSearch(index)}
          >×</button>
        </div>
      {/each}
    </div>
  {/if}

  <div class="section-header">
    <span>ラベル</span>
    <button type="button" class="add-label" onclick={() => (creatingLabel = !creatingLabel)}>+</button>
  </div>

  {#if creatingLabel}
    <form class="new-label-form" onsubmit={(e) => { e.preventDefault(); submitNewLabel() }}>
      <input placeholder="ラベル名" bind:value={newLabelName} />
      <div class="swatches">
        {#each LABEL_COLORS as color (color)}
          <button
            type="button"
            class="swatch"
            class:selected={newLabelColor === color}
            style="background: {color};"
            onclick={() => (newLabelColor = color)}
            aria-label={color}
          ></button>
        {/each}
      </div>
      <button type="submit" class="create-button">作成</button>
      {#if createError}
        <p class="error">{createError}</p>
      {/if}
    </form>
  {/if}

  <div class="labels">
    {#each labels as label (label.id)}
      <div
        class="row label-row"
        class:active={!searching && selectedLabelId === label.id}
        role="group"
        oncontextmenu={contextMenu(onLabelContextMenu, label)}
      >
        <button type="button" class="row-main" onclick={() => onSelectLabel(label.id)}>
          <span class="dot dot-visible" style="background: {label.color};"></span>
          <span class="name">{label.name}</span>
          <span class="count">{label.count}</span>
        </button>
        <button
          type="button"
          class="delete-label"
          title="ラベルを削除"
          onclick={() => onDeleteLabel(label.id)}
        >×</button>
      </div>
    {/each}
  </div>

  <button type="button" class="settings-link" onclick={onOpenSettings}>設定</button>
</aside>
<!-- 右端の境界線に重ねた幅変更のつまみ（場所は取らない）。ダブルクリックで既定の幅へ戻す。 -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="resize-handle"
  class:dragging={dragStart !== null}
  role="separator"
  aria-orientation="vertical"
  aria-label="サイドバーの幅"
  aria-valuenow={width}
  aria-valuemin={SIDEBAR_MIN_WIDTH}
  aria-valuemax={SIDEBAR_MAX_WIDTH}
  tabindex="0"
  title="ドラッグで幅を変更（ダブルクリックで元の幅に戻す）"
  onpointerdown={startResize}
  onpointermove={moveResize}
  onpointerup={endResize}
  onpointercancel={endResize}
  onlostpointercapture={endResize}
  ondblclick={() => onResize(SIDEBAR_DEFAULT_WIDTH, true)}
  onkeydown={resizeByKey}
></div>
{/if}

<style>
  .sidebar {
    width: 220px;
    flex: none;
    background: var(--surface-muted);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    padding: 16px 10px;
    gap: 2px;
    box-sizing: border-box;
    overflow-y: auto;
  }
  .sidebar.collapsed {
    width: 44px;
    padding: 12px 6px;
    align-items: center;
    overflow: hidden;
  }
  /* 幅0の位置に置き、左右3pxずつ境界線にかぶせる（レイアウト上は場所を取らない）。 */
  .resize-handle {
    flex: none;
    position: relative;
    z-index: 2;
    width: 7px;
    margin: 0 -4px 0 -3px;
    cursor: col-resize;
    touch-action: none;
    outline: none;
  }
  .resize-handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 2px;
    width: 3px;
    background: transparent;
    transition: background 0.12s;
  }
  .resize-handle:hover::after,
  .resize-handle:focus-visible::after,
  .resize-handle.dragging::after {
    background: var(--accent);
  }
  :global(body.resizing-sidebar) {
    cursor: col-resize;
    user-select: none;
  }
  .sidebar-top {
    display: flex;
    justify-content: flex-end;
    margin: -8px -2px 2px;
  }
  .collapse-toggle {
    border: none;
    background: none;
    color: var(--text-faint);
    font: inherit;
    font-size: 15px;
    line-height: 1;
    padding: 4px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .collapse-toggle:hover {
    background: var(--surface-subtle);
    color: var(--text);
  }
  .rail-settings {
    text-align: center;
    font-size: 15px;
  }
  .folders,
  .labels {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border-radius: 7px;
    font-size: 13px;
    color: var(--text-secondary);
    border: none;
    background: none;
    cursor: pointer;
    text-align: left;
    font: inherit;
    width: 100%;
    /* ラベル・保存した検索の行はdivなので、指定しないとpadding分はみ出して横スクロールバーが出る */
    box-sizing: border-box;
  }
  .row:hover {
    background: var(--surface-subtle);
  }
  .row.active {
    background: var(--accent-soft-bg);
    color: var(--text);
    font-weight: 700;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
    background: transparent;
  }
  .dot-visible {
    background: var(--accent);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 11px;
    color: var(--text-faint);
  }
  .row.active .count {
    color: var(--accent);
  }
  .section-header {
    margin: 18px 10px 6px;
    font-size: 11px;
    letter-spacing: 0.1em;
    color: var(--text-faint);
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .add-label {
    border: none;
    background: none;
    color: var(--text-faint);
    font-size: 15px;
    cursor: pointer;
    line-height: 1;
    padding: 0 4px;
  }
  .label-row {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .row-main {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border: none;
    background: none;
    cursor: pointer;
    text-align: left;
    font: inherit;
    font-size: 13px;
    color: var(--text-secondary);
    flex: 1;
    min-width: 0;
    border-radius: 7px;
  }
  .label-row.active .row-main {
    background: var(--accent-soft-bg);
    color: var(--text);
    font-weight: 700;
  }
  .row-main:hover {
    background: var(--surface-subtle);
  }
  .delete-label {
    border: none;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    font-size: 14px;
    padding: 0 8px;
    visibility: hidden;
  }
  .label-row:hover .delete-label {
    visibility: visible;
  }
  .search-icon {
    width: 8px;
    flex: none;
    color: var(--text-faint);
    font-size: 13px;
    line-height: 1;
  }
  .new-label-form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px 10px 14px;
  }
  .new-label-form input {
    padding: 6px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    font: inherit;
    font-size: 12.5px;
  }
  .swatches {
    display: flex;
    gap: 6px;
  }
  .swatch {
    width: 18px;
    height: 18px;
    border-radius: 5px;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
  }
  .swatch.selected {
    border-color: var(--text);
  }
  .create-button {
    align-self: flex-start;
    border: none;
    border-radius: 6px;
    padding: 5px 12px;
    font-size: 12px;
    font-weight: 700;
    color: #fff;
    background: var(--accent);
    cursor: pointer;
  }
  .error {
    color: var(--danger);
    font-size: 11.5px;
    margin: 0;
  }
  .settings-link {
    margin-top: auto;
    border: none;
    background: none;
    color: var(--text-muted);
    font-size: 12.5px;
    text-align: left;
    padding: 8px 10px;
    cursor: pointer;
    font: inherit;
  }
  .settings-link:hover {
    color: var(--text);
  }
</style>
