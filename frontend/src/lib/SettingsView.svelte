<script>
  // 一覧のプレビュー行/差出人アイコン表示/既読にするまでの時間の3トグルのみ。
  // モックアップにあった「画像を自動で読み込む」（本文はプレーンテキストのみ表示する
  // 設計のため対象が無い）「送信の取り消し」（送信機能自体が無い）は実装しない。
  import { invoke } from '@tauri-apps/api/core'
  import AccountsView from './AccountsView.svelte'
  import ColorRulesView from './ColorRulesView.svelte'
  import { applyAppearance, THEME_OPTIONS, ACCENT_COLOR_OPTIONS, FONT_SIZE_OPTIONS } from './appearance.js'

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

  // OSにインストールされているフォントの一覧（フォント選択プルダウン用）。
  // 一覧取得自体は失敗しても致命的ではない（空のままなら、現在保存されている
  // フォント名だけがプルダウンに出る。下の`fontFamilyOptions`参照）ので、
  // ここではエラーメッセージを出さずに黙って諦める。
  let installedFonts = $state([])

  async function load() {
    try {
      settings = await invoke('get_settings')
      applyAppearance(settings)
    } catch (e) {
      error = String(e)
    }
    try {
      installedFonts = await invoke('list_installed_fonts')
    } catch {
      installedFonts = []
    }
  }
  load()

  // 現在保存されているフォント名がインストール済み一覧に無い場合でも
  // （手編集・別環境からの引き継ぎ・一覧取得失敗など）、選択肢から消えて
  // 「何も選ばれていないように見える」ことが無いよう先頭に足しておく。
  let fontFamilyOptions = $derived(
    settings && settings.font_family && !installedFonts.includes(settings.font_family)
      ? [settings.font_family, ...installedFonts]
      : installedFonts,
  )

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

  // テーマ/アクセントカラー/フォント/文字サイズは変更した瞬間に見た目へ
  // 反映する（保存を待たず即座にプレビューできた方が、選んでいる最中に
  // 分かりやすいため）。保存自体は他の設定と同じくupdate_settingsで行う。
  async function updateAppearance(key, value) {
    if (!settings || saving) return
    const next = { ...settings, [key]: value }
    settings = next
    applyAppearance(next)
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

  // 0 = 無期限（自動削除しない）。既存ユーザーが不意にメールを失わないよう
  // 既定値も0にしている（db::settings::Settingsのドキュメント参照）。
  async function updateRetentionDays(days) {
    if (!settings || saving) return
    const next = { ...settings, retention_days: days }
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

  let purging = $state(false)
  let purgeStatus = $state('')

  async function runPurge() {
    purging = true
    purgeStatus = '整理中…'
    try {
      const count = await invoke('purge_expired_messages')
      purgeStatus =
        settings.retention_days > 0
          ? `${count}件を削除しました`
          : '保持期間が無期限のため、何も削除されませんでした'
    } catch (e) {
      purgeStatus = `エラー: ${e}`
    } finally {
      purging = false
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
    <h2 class="section-title">外観</h2>
    <div class="rows">
      <div class="row">
        <div class="text">
          <div class="title">テーマ</div>
          <div class="desc">「システムに合わせる」を選ぶとOSのダーク/ライト設定に追従します。</div>
        </div>
        <select
          class="interval-select"
          value={settings.theme}
          onchange={(e) => updateAppearance('theme', e.target.value)}
        >
          {#each THEME_OPTIONS as opt (opt.value)}
            <option value={opt.value}>{opt.label}</option>
          {/each}
        </select>
      </div>
      <div class="row">
        <div class="text">
          <div class="title">アクセントカラー</div>
          <div class="desc">ボタンや選択中の項目などに使う差し色です。</div>
        </div>
        <div class="swatch-picker">
          {#each ACCENT_COLOR_OPTIONS as opt (opt.value)}
            <button
              type="button"
              class="swatch-button"
              class:selected={settings.accent_color === opt.value}
              style="background: {opt.value};"
              title={opt.label}
              aria-label={opt.label}
              aria-pressed={settings.accent_color === opt.value}
              onclick={() => updateAppearance('accent_color', opt.value)}
            ></button>
          {/each}
        </div>
      </div>
      <div class="row">
        <div class="text">
          <div class="title">フォント</div>
          <div class="desc">一覧・本文表示に使うフォントです（PCにインストールされているフォントから選べます）。</div>
        </div>
        <select
          class="interval-select font-select"
          value={settings.font_family}
          onchange={(e) => updateAppearance('font_family', e.target.value)}
        >
          {#each fontFamilyOptions as name (name)}
            <option value={name}>{name}</option>
          {/each}
        </select>
      </div>
      <div class="row">
        <div class="text">
          <div class="title">文字サイズ</div>
          <div class="desc">一覧・本文の文字の大きさです。</div>
        </div>
        <select
          class="interval-select"
          value={settings.font_size}
          onchange={(e) => updateAppearance('font_size', e.target.value)}
        >
          {#each FONT_SIZE_OPTIONS as opt (opt.value)}
            <option value={opt.value}>{opt.label}</option>
          {/each}
        </select>
      </div>
    </div>

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

    <h2 class="section-title">保持期間</h2>
    <div class="rows">
      <div class="row">
        <div class="text">
          <div class="title">メールの保持日数</div>
          <div class="desc">
            指定した日数より古いメールを、6時間ごとにローカルから完全に削除します
            （復元できません。スター付きのメールは保持日数に関わらず削除されません）。
            0は無期限（自動削除しない）です。
          </div>
        </div>
        <input
          type="number"
          class="interval-select retention-input"
          min="0"
          step="1"
          value={settings.retention_days}
          onchange={(e) => updateRetentionDays(Math.max(0, Number(e.target.value) || 0))}
        />
      </div>
      <div class="row">
        <div class="text">
          <div class="title">今すぐ整理する</div>
          <div class="desc">
            次回のバックグラウンド実行を待たず、保持日数を過ぎたメールを今すぐ削除します。
          </div>
        </div>
        <button type="button" class="action-button danger" onclick={runPurge} disabled={purging}>
          今すぐ整理する
        </button>
      </div>
      {#if purgeStatus}
        <p class="reindex-status">{purgeStatus}</p>
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
  .font-select {
    max-width: 220px;
  }
  .swatch-picker {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .swatch-button {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
  }
  .swatch-button.selected {
    border-color: var(--text);
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
    background: var(--border-strong);
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
  .action-button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }
  .retention-input {
    width: 70px;
    text-align: right;
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
