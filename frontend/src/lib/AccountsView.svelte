<script>
  // Add account/Account listは設定画面(SettingsView)内の「アカウント」セクションに
  // 埋め込まれる（単独の画面ではない。見出し・戻る導線はSettingsView側が持つ）。
  // 状態そのもの(accounts配列・syncStatus等)はApp.svelte側で持ち続け、
  // ここは表示とフォームのローカル状態(未送信の入力内容)だけを持つ。
  import { openPath } from '@tauri-apps/plugin-opener'

  let {
    accounts = [],
    accountsConfigPath = '',
    syncStatus = {},
    reloadingConfig = false,
    reloadConfigStatus = '',
    onAdd = async () => {},
    onRemove = async () => {},
    onSync = async () => {},
    onReloadConfig = async () => {},
  } = $props()

  let form = $state({ name: '', host: '', port: 995, username: '', useTls: true, password: '' })
  let formError = $state('')
  let submitting = $state(false)
  let openConfigError = $state('')

  async function openConfigFile() {
    openConfigError = ''
    try {
      // OSに紐づいたデフォルトアプリ(メモ帳等)でaccounts.tomlを開く。
      await openPath(accountsConfigPath)
    } catch (e) {
      openConfigError = String(e)
    }
  }

  async function handleSubmit() {
    formError = ''
    submitting = true
    try {
      await onAdd({ ...form, port: Number(form.port) })
      form = { name: '', host: '', port: 995, username: '', useTls: true, password: '' }
    } catch (e) {
      formError = String(e)
    } finally {
      submitting = false
    }
  }
</script>

<div class="accounts-view">
  <div class="layout">
    <section class="add-section">
      <div class="section-label">アカウントを追加</div>
      <form class="add-account-form" onsubmit={(e) => { e.preventDefault(); handleSubmit() }}>
        <label class="field">
          <span>Name</span>
          <input bind:value={form.name} required />
        </label>
        <label class="field">
          <span>Host</span>
          <input bind:value={form.host} required />
        </label>
        <label class="field">
          <span>Port</span>
          <input type="number" bind:value={form.port} required min="1" max="65535" />
        </label>
        <label class="field">
          <span>Username</span>
          <input bind:value={form.username} required />
        </label>
        <label class="field">
          <span>Password</span>
          <input type="password" bind:value={form.password} required />
        </label>
        <label class="field checkbox-field">
          <span>Use TLS</span>
          <input type="checkbox" bind:checked={form.useTls} />
        </label>
        <button type="submit" class="primary-button" disabled={submitting}>アカウントを追加</button>
      </form>
      {#if formError}
        <p class="error">{formError}</p>
      {/if}
    </section>

    <section class="list-section">
      <div class="section-label">登録済みアカウント（{accounts.length}）</div>

      {#if accountsConfigPath}
        <p class="hint">
          設定（パスワードを除く）は <code>{accountsConfigPath}</code> に保存されています。
          直接編集して読み込み直すこともできます。
          <button type="button" class="text-button" onclick={openConfigFile}>ファイルを開く</button>
          <button type="button" class="text-button" onclick={onReloadConfig} disabled={reloadingConfig}>
            再読み込み
          </button>
          {#if reloadConfigStatus}
            <span class="status">{reloadConfigStatus}</span>
          {/if}
          {#if openConfigError}
            <span class="error">{openConfigError}</span>
          {/if}
        </p>
      {/if}

      {#if accounts.length === 0}
        <p class="empty">アカウントがまだありません。</p>
      {:else}
        <div class="account-cards">
          {#each accounts as account (account.id)}
            <div class="account-card">
              <div class="account-info">
                <div class="account-name">{account.name}</div>
                <div class="account-meta">{account.username}@{account.host}:{account.port}</div>
                {#if syncStatus[account.id]}
                  <div class="sync-status">{syncStatus[account.id]}</div>
                {/if}
              </div>
              <div class="account-actions">
                <button type="button" onclick={() => onSync(account.id)}>Sync</button>
                <button type="button" class="danger" onclick={() => onRemove(account.id)}>削除</button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  </div>
</div>

<style>
  .accounts-view {
    max-width: 760px;
  }
  .layout {
    display: flex;
    flex-direction: column;
    gap: 2rem;
  }
  .section-label {
    font-size: 11px;
    letter-spacing: 0.1em;
    color: var(--text-faint);
    margin-bottom: 12px;
  }
  .add-account-form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    max-width: 360px;
  }
  .field {
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.9em;
    color: var(--text-secondary);
  }
  .field span {
    flex: 0 0 90px;
  }
  .field span::after {
    content: ':';
  }
  .field input {
    flex: 1 1 auto;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    font: inherit;
    background: var(--surface);
    color: var(--text);
  }
  .checkbox-field input {
    flex: 0 0 auto;
  }
  .primary-button {
    align-self: flex-start;
    margin-top: 0.25rem;
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
  .account-cards {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 12px;
  }
  .account-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 13px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-subtle);
  }
  .account-card:hover {
    border-color: var(--border-strong);
  }
  .account-info {
    min-width: 0;
  }
  .account-name {
    font-size: 13.5px;
    font-weight: 500;
    color: var(--text);
  }
  .account-meta {
    font-size: 12px;
    color: var(--text-muted);
    margin-top: 2px;
  }
  .sync-status {
    font-size: 11.5px;
    color: var(--accent);
    margin-top: 4px;
  }
  .account-actions {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .account-actions button {
    font-size: 12px;
    color: #5a574f;
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 6px 12px;
    background: var(--surface);
    cursor: pointer;
  }
  .account-actions button:hover {
    background: var(--surface-muted);
  }
  .account-actions button.danger {
    color: var(--danger);
  }
</style>
