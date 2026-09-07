<script>
  // 一覧のプレビュー行/差出人アイコン表示/既読にするまでの時間の3トグルのみ。
  // モックアップにあった「画像を自動で読み込む」（本文はプレーンテキストのみ表示する
  // 設計のため対象が無い）「送信の取り消し」（送信機能自体が無い）は実装しない。
  import { invoke } from '@tauri-apps/api/core'
  import AccountsView from './AccountsView.svelte'
  import ColorRulesView from './ColorRulesView.svelte'

  let {
    onBack = () => {},
    accounts = [],
    accountsConfigPath = '',
    syncStatus = {},
    reloadingConfig = false,
    reloadConfigStatus = '',
    onAddAccount = async () => {},
    onRemoveAccount = async () => {},
    onSyncAccount = async () => {},
    onReloadAccountsConfig = async () => {},
    colorRules = [],
    colorRulesConfigPath = '',
    onAddColorRule = async () => {},
    onRemoveColorRule = async () => {},
    onReloadColorRules = async () => {},
  } = $props()

  let settings = $state(null)
  let error = $state('')
  let saving = $state(false)

  async function load() {
    try {
      settings = await invoke('get_settings')
    } catch (e) {
      error = String(e)
    }
  }
  load()

  async function toggle(key) {
    if (!settings || saving) return
    const next = { ...settings, [key]: !settings[key] }
    settings = next
    saving = true
    try {
      await invoke('update_settings', { settings: next })
    } catch (e) {
      error = String(e)
    } finally {
      saving = false
    }
  }

  const SYNC_INTERVAL_OPTIONS = [1, 5, 10, 15, 30, 60]

  async function updateSyncInterval(minutes) {
    if (!settings || saving) return
    const next = { ...settings, sync_interval_minutes: minutes }
    settings = next
    saving = true
    try {
      await invoke('update_settings', { settings: next })
    } catch (e) {
      error = String(e)
    } finally {
      saving = false
    }
  }

  let reindexing = $state(false)
  let reindexStatus = $state('')

  async function runReindex() {
    reindexing = true
    reindexStatus = '再構築中…'
    try {
      const count = await invoke('reindex_all')
      reindexStatus = `${count}件を再構築しました`
    } catch (e) {
      reindexStatus = `エラー: ${e}`
    } finally {
      reindexing = false
    }
  }

  const ROWS = [
    {
      key: 'show_preview_line',
      title: '一覧のプレビュー行',
      desc: '件名の下に本文の冒頭を表示します。',
    },
    {
      key: 'show_sender_avatar',
      title: '差出人アイコンを表示',
      desc: '一覧と本文にアバターを表示します。',
    },
    {
      key: 'mark_read_delay',
      title: '既読にするまでの時間',
      desc: '本文を2秒表示したら既読にします。',
    },
  ]
</script>

<div class="settings-view">
  <button type="button" class="back" onclick={onBack}>← メッセージへ戻る</button>
  <h2>設定</h2>
  <p class="lead">アカウント・表示・同期・検索の動作を調整します。</p>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <h2 class="section-title">アカウント</h2>
  <AccountsView
    {accounts}
    {accountsConfigPath}
    {syncStatus}
    {reloadingConfig}
    {reloadConfigStatus}
    onAdd={onAddAccount}
    onRemove={onRemoveAccount}
    onSync={onSyncAccount}
    onReloadConfig={onReloadAccountsConfig}
  />

  {#if settings}
    <h2 class="section-title">表示</h2>
    <div class="rows">
      {#each ROWS as row (row.key)}
        <div class="row">
          <div class="text">
            <div class="title">{row.title}</div>
            <div class="desc">{row.desc}</div>
          </div>
          <button
            type="button"
            class="switch"
            class:on={settings[row.key]}
            onclick={() => toggle(row.key)}
            aria-pressed={settings[row.key]}
            aria-label={row.title}
          >
            <span class="knob"></span>
          </button>
        </div>
      {/each}
    </div>

    <h2 class="section-title">同期</h2>
    <div class="rows">
      <div class="row">
        <div class="text">
          <div class="title">バックグラウンド自動同期の間隔</div>
          <div class="desc">
            この間隔で全アカウントを自動的に同期します（平文接続のアカウントも対象です）。
            検索インデックスへの投入に失敗した分もこのタイミングで
            自動的に拾い直すので、通常は「検索インデックス再構築」を手動で押す必要はありません。
          </div>
        </div>
        <select
          class="interval-select"
          value={settings.sync_interval_minutes}
          onchange={(e) => updateSyncInterval(Number(e.target.value))}
        >
          {#each SYNC_INTERVAL_OPTIONS as minutes (minutes)}
            <option value={minutes}>{minutes}分</option>
          {/each}
        </select>
      </div>
    </div>

    <h2 class="section-title">検索</h2>
    <div class="rows">
      <div class="row">
        <div class="text">
          <div class="title">検索インデックスを再構築</div>
          <div class="desc">
            DB/Maildirから検索インデックスを全件作り直します。バックグラウンド同期が
            投入漏れを自動的に拾い直すので通常は不要ですが、それでも検索結果に
            出てこないメッセージがある場合はここから手動で再構築できます。
          </div>
        </div>
        <button type="button" class="action-button" onclick={runReindex} disabled={reindexing}>
          再構築
        </button>
      </div>
      {#if reindexStatus}
        <p class="reindex-status">{reindexStatus}</p>
      {/if}
    </div>

    <h2 class="section-title">色分けルール</h2>
    <ColorRulesView
      rules={colorRules}
      configPath={colorRulesConfigPath}
      onAdd={onAddColorRule}
      onRemove={onRemoveColorRule}
      onReload={onReloadColorRules}
    />
  {/if}
</div>

<style>
  .settings-view {
    max-width: 620px;
    margin: 0 auto;
  }
  .back {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    padding: 0;
    margin-bottom: 1.5rem;
    font: inherit;
  }
  .back:hover {
    text-decoration: underline;
  }
  h2 {
    font-size: 28px;
    margin-bottom: 4px;
  }
  .section-title {
    margin-top: 2rem;
  }
  .interval-select {
    flex: none;
    padding: 6px 10px;
    border: 1px solid var(--border-strong);
    border-radius: 7px;
    font: inherit;
    font-size: 13px;
    background: var(--surface);
    color: var(--text);
  }
  .lead {
    font-size: 13px;
    color: var(--text-muted);
    margin-bottom: 1.5rem;
  }
  .rows {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 20px;
    padding: 16px 0;
    border-bottom: 1px solid var(--border);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .title {
    font-size: 13.5px;
    font-weight: 500;
    color: var(--text);
  }
  .desc {
    font-size: 12px;
    color: var(--text-faint);
    margin-top: 3px;
  }
  .switch {
    width: 40px;
    height: 23px;
    border-radius: 999px;
    flex: none;
    display: flex;
    align-items: center;
    padding: 2px;
    background: #dedad0;
    justify-content: flex-start;
    border: none;
    cursor: pointer;
  }
  .switch.on {
    background: var(--accent);
    justify-content: flex-end;
  }
  .knob {
    width: 19px;
    height: 19px;
    border-radius: 50%;
    background: #fff;
    display: block;
    box-shadow: 0 1px 2px rgba(28, 27, 24, 0.2);
  }
  .action-button {
    flex: none;
    border: 1px solid var(--border-strong);
    background: var(--surface-subtle);
    border-radius: 7px;
    padding: 7px 14px;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-secondary);
    cursor: pointer;
  }
  .action-button:hover {
    background: var(--surface-muted);
  }
  .action-button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .reindex-status {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .error {
    color: var(--danger);
  }
</style>
