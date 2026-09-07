<script>
  // 設定画面(SettingsView)内の「色分けルール」セクションに埋め込まれる
  // （AccountsView.svelteと同じ構成: 状態はApp.svelte側で持ち、ここは
  // 表示とフォームのローカル状態(未送信の入力内容)だけを持つ）。
  import { openPath } from '@tauri-apps/plugin-opener'

  let {
    rules = [],
    configPath = '',
    onAdd = async () => {},
    onRemove = async () => {},
    onReload = async () => {},
  } = $props()

  let pattern = $state('')
  let color = $state('#b00020')
  let formError = $state('')
  let submitting = $state(false)
  let openConfigError = $state('')
  let reloading = $state(false)
  let reloadStatus = $state('')

  async function openConfigFile() {
    openConfigError = ''
    try {
      await openPath(configPath)
    } catch (e) {
      openConfigError = String(e)
    }
  }

  async function reload() {
    reloading = true
    reloadStatus = ''
    try {
      await onReload()
      reloadStatus = '再読み込みしました'
    } catch (e) {
      reloadStatus = `error: ${e}`
    } finally {
      reloading = false
    }
  }

  async function handleSubmit() {
    formError = ''
    submitting = true
    try {
      await onAdd({ pattern, color })
      pattern = ''
    } catch (e) {
      formError = String(e)
    } finally {
      submitting = false
    }
  }
</script>

<div class="color-rules-view">
  <p class="desc">
    件名・差出人・本文プレビューに特定の文字列が含まれるメッセージを、一覧で指定した色の
    文字にします（例: 監視アラートの件名に含まれる語を目立たせる）。上にあるルールほど
    優先されます。
  </p>

  <form class="add-form" onsubmit={(e) => { e.preventDefault(); handleSubmit() }}>
    <label class="field">
      <span>文字列</span>
      <input bind:value={pattern} placeholder="例: critical" required />
    </label>
    <label class="field color-field">
      <span>色</span>
      <input type="color" bind:value={color} />
    </label>
    <button type="submit" class="primary-button" disabled={submitting || !pattern.trim()}>
      ルールを追加
    </button>
  </form>
  {#if formError}
    <p class="error">{formError}</p>
  {/if}

  {#if configPath}
    <p class="hint">
      <code>{configPath}</code> に保存されています。直接編集して読み込み直すこともできます。
      <button type="button" class="text-button" onclick={openConfigFile}>ファイルを開く</button>
      <button type="button" class="text-button" onclick={reload} disabled={reloading}>再読み込み</button>
      {#if reloadStatus}
        <span class="status">{reloadStatus}</span>
      {/if}
      {#if openConfigError}
        <span class="error">{openConfigError}</span>
      {/if}
    </p>
  {/if}

  {#if rules.length === 0}
    <p class="empty">色分けルールがまだありません。</p>
  {:else}
    <ul class="rule-list">
      {#each rules as rule, index (index)}
        <li class="rule-row">
          <span class="swatch" style="background: {rule.color};"></span>
          <span class="pattern">{rule.pattern}</span>
          <button type="button" class="danger" onclick={() => onRemove(index)}>削除</button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .color-rules-view {
    max-width: 620px;
  }
  .desc {
    color: var(--text-secondary);
    font-size: 0.9em;
    line-height: 1.6;
    margin: 0 0 1rem;
  }
  .add-form {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.9em;
    color: var(--text-secondary);
  }
  .field span::after {
    content: ':';
  }
  .field input {
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    font: inherit;
    background: var(--surface);
    color: var(--text);
  }
  .color-field input {
    padding: 0.15rem;
    width: 46px;
    height: 30px;
  }
  .primary-button {
    border: none;
    border-radius: 7px;
    padding: 9px 16px;
    font: inherit;
    font-size: 13px;
    font-weight: 700;
    color: #fff;
    background: var(--accent);
    cursor: pointer;
  }
  .primary-button:hover {
    background: var(--accent-hover);
  }
  .primary-button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error {
    color: var(--danger);
  }
  .hint {
    color: var(--text-secondary);
    font-size: 0.9em;
    background: var(--surface-subtle);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.6rem 0.85rem;
    line-height: 1.6;
    margin-bottom: 1rem;
  }
  .hint code {
    background: var(--surface-muted);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
  }
  .text-button {
    border: 1px solid var(--accent);
    background: transparent;
    color: var(--accent);
    font: inherit;
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 6px;
    cursor: pointer;
    margin-left: 6px;
  }
  .text-button:hover {
    background: var(--accent-soft-bg);
  }
  .status {
    margin-left: 0.5rem;
    color: var(--text-muted);
    font-size: 0.9em;
  }
  .empty {
    color: var(--text-muted);
    font-size: 13px;
  }
  .rule-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .rule-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-subtle);
  }
  .swatch {
    flex: none;
    width: 14px;
    height: 14px;
    border-radius: 4px;
    border: 1px solid var(--border-strong);
  }
  .pattern {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rule-row button {
    flex: none;
    font-size: 12px;
    color: var(--danger);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 5px 12px;
    background: var(--surface);
    cursor: pointer;
  }
  .rule-row button:hover {
    background: var(--surface-muted);
  }
</style>
