<script>
  // フォルダ(受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱)とラベルの一覧。
  // 状態(件数・選択中フォルダ等)はApp.svelte側で持ち、ここは表示とラベル作成
  // フォームのローカル状態(未送信の入力内容)だけを持つ。
  let {
    folderCounts = { inbox: 0, starred: 0, snoozed: 0, archive: 0, trash: 0 },
    labels = [],
    selectedFolder = 'inbox',
    selectedLabelId = null,
    onSelectFolder = () => {},
    onSelectLabel = () => {},
    onCreateLabel = async () => {},
    onDeleteLabel = async () => {},
    onOpenSettings = () => {},
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

<aside class="sidebar">
  <div class="folders">
    {#each FOLDERS as folder (folder.key)}
      <button
        type="button"
        class="row"
        class:active={selectedFolder === folder.key && selectedLabelId === null}
        onclick={() => onSelectFolder(folder.key)}
      >
        <span class="dot" class:dot-visible={selectedFolder === folder.key && selectedLabelId === null}></span>
        <span class="name">{folder.name}</span>
        <span class="count">{folderCount(folder.key)}</span>
      </button>
    {/each}
  </div>

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
      <div class="row label-row" class:active={selectedLabelId === label.id}>
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
    background: #eaedeb;
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
    background: #eaedeb;
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
