<script>
  // 複数選択中に一覧の上へ出す一括操作バー。操作の実行と一覧への反映はApp.svelteの
  // `runBulk`が行い、ここは`onAction({ kind, ... })`で何をするかを伝えるだけ
  // （`bulk_update`コマンドに渡す形と同じ）。
  let {
    count = 0,
    folder = 'inbox',
    labelId = null,
    labels = [],
    busy = false,
    onAction = () => {},
    onSelectAll = () => {},
    onClear = () => {},
  } = $props()

  let menu = $state(null) // 'snooze' | 'label' | null

  function act(action) {
    menu = null
    onAction(action)
  }

  function toggleMenu(name) {
    menu = menu === name ? null : name
  }

  const inArchive = $derived(labelId === null && folder === 'archive')
  const inSnoozed = $derived(labelId === null && folder === 'snoozed')
  const currentLabel = $derived(labels.find((l) => l.id === labelId) ?? null)
</script>

<div class="bulk-bar" role="toolbar" aria-label="選択したメッセージの操作">
  <span class="count">{count}件を選択中</span>
  <button type="button" onclick={() => act({ kind: 'read', value: true })} disabled={busy}>既読</button>
  <button type="button" onclick={() => act({ kind: 'read', value: false })} disabled={busy}>未読</button>
  <button type="button" onclick={() => act({ kind: 'flag', value: true })} disabled={busy}>スター</button>
  <button type="button" onclick={() => act({ kind: 'flag', value: false })} disabled={busy}>スター解除</button>
  {#if inArchive}
    <button type="button" onclick={() => act({ kind: 'archive', value: false })} disabled={busy}>
      受信箱に戻す
    </button>
  {:else}
    <button type="button" onclick={() => act({ kind: 'archive', value: true })} disabled={busy}>
      アーカイブ
    </button>
  {/if}
  <div class="menu-wrap">
    <button type="button" onclick={() => toggleMenu('snooze')} disabled={busy}>スヌーズ</button>
    {#if menu === 'snooze'}
      <div class="menu">
        <button type="button" onclick={() => act({ kind: 'snooze', preset: '1d' })}>1日後</button>
        <button type="button" onclick={() => act({ kind: 'snooze', preset: '3d' })}>3日後</button>
        <button type="button" onclick={() => act({ kind: 'snooze', preset: '1w' })}>1週間後</button>
        {#if inSnoozed}
          <button type="button" onclick={() => act({ kind: 'snooze', preset: null })}>スヌーズ解除</button>
        {/if}
      </div>
    {/if}
  </div>
  <div class="menu-wrap">
    <button type="button" onclick={() => toggleMenu('label')} disabled={busy}>ラベル</button>
    {#if menu === 'label'}
      <div class="menu">
        {#if labels.length === 0}
          <span class="menu-empty">ラベルがありません</span>
        {/if}
        {#each labels as label (label.id)}
          <button type="button" onclick={() => act({ kind: 'label', label_id: label.id, assigned: true })}>
            <span class="dot" style="background: {label.color};"></span>
            {label.name} を付ける
          </button>
        {/each}
        {#if currentLabel}
          <button
            type="button"
            onclick={() => act({ kind: 'label', label_id: currentLabel.id, assigned: false })}
          >
            「{currentLabel.name}」を外す
          </button>
        {/if}
      </div>
    {/if}
  </div>
  <button type="button" class="danger" onclick={() => act({ kind: 'delete' })} disabled={busy}>削除</button>
  <span class="spacer"></span>
  <button type="button" class="plain" onclick={onSelectAll} disabled={busy}>表示中をすべて選択</button>
  <button type="button" class="plain" onclick={onClear} disabled={busy}>選択解除</button>
</div>

<style>
  .bulk-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-bottom: 0.75rem;
    padding: 6px 10px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--accent-soft-bg);
    font-size: 12.5px;
  }
  .count {
    font-weight: 600;
    color: var(--accent-hover);
    margin-right: 4px;
  }
  .spacer {
    flex: 1;
  }
  button {
    font: inherit;
    font-size: 12px;
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 4px 10px;
    background: var(--surface);
    cursor: pointer;
  }
  button:hover {
    background: var(--surface-muted);
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  button.danger {
    color: var(--danger);
  }
  button.plain {
    border-color: transparent;
    background: none;
    color: var(--text-muted);
  }
  .menu-wrap {
    position: relative;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 170px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 8px 20px -8px rgba(28, 27, 24, 0.35);
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    border: none;
    background: none;
    text-align: left;
    padding: 7px 10px;
  }
  .menu button:hover {
    background: var(--surface-muted);
  }
  .menu-empty {
    padding: 7px 10px;
    color: var(--text-faint);
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
  }
</style>
