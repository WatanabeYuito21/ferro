<script>
  // フォルダ(受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱)とラベルの一覧。
  // 状態(件数・選択中フォルダ等)はApp.svelte側で持ち、ここは表示とラベル作成
  // フォームのローカル状態(未送信の入力内容)だけを持つ。
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
    // 折りたたみ中は、開く・設定のボタンだけの細い帯にする（状態はApp.svelteが持つ）。
    collapsed = false,
    onToggleCollapsed = () => {},
  } = $props()

  const FOLDERS = [
    { key: 'inbox', name: '受信箱' },
    { key: 'starred', name: 'スター付き' },
    { key: 'snoozed', name: 'スヌーズ' },
    { key: 'archive', name: 'アーカイブ' },
    { key: 'trash', name: 'ゴミ箱' },
  ]

  // モックアップのラベル配色から抜粋したプリセット（自由入力ではなくここから選ぶ）。
  const LABEL_COLORS = ['#C08A2E', '#7E9E86', '#8D9FC4', '#C58C82', '#5B6E92', '#96701F']

  let creatingLabel = $state(false)
  let newLabelName = $state('')
  let newLabelColor = $state(LABEL_COLORS[0])
  let createError = $state('')

  function folderCount(key) {
    return folderCounts[key] ?? 0
  }

  // 検索中は通常のフォルダ/ラベルを選択中として出さない（保存した検索の方を出す）。
  let searching = $derived(activeSavedQuery !== null)

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
<aside class="sidebar">
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
        <div class="row label-row" class:active={activeSavedQuery === search.query}>
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
      <div class="row label-row" class:active={!searching && selectedLabelId === label.id}>
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
