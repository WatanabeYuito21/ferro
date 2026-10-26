<script>
  // 一覧のプレビュー行/差出人アイコン表示/既読にするまでの時間の3トグルのみ。
  // モックアップにあった「画像を自動で読み込む」（本文はプレーンテキストのみ表示する
  // 設計のため対象が無い）「送信の取り消し」（送信機能自体が無い）は実装しない。
  import { invoke } from '@tauri-apps/api/core'
  import { openPath } from '@tauri-apps/plugin-opener'
  import AccountsView from './AccountsView.svelte'
  import ColorRulesView from './ColorRulesView.svelte'
  import LabelRulesView from './LabelRulesView.svelte'
  import { updater, checkForUpdate } from './updater.svelte.js'
  import {
    reindexer,
    runReindex,
    reindexPercent,
    indexStatus,
    refreshIndexStatus,
    describeIndexStatus,
  } from './reindex.svelte.js'
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
    labels = [],
    labelRules = [],
    labelRulesConfigPath = '',
    onAddLabelRule = async () => {},
    onRemoveLabelRule = async () => {},
    onReloadLabelRules = async () => {},
    onApplyLabelRulesNow = async () => {},
    logFilePath = '',
    appVersion = '',
  } = $props()

  let settings = $state(null)
  let error = $state('')
  let saving = $state(false)
  let openLogError = $state('')

  async function openLogFile() {
    openLogError = ''
    try {
      // 同期エラー等を追記している診断ログをOSのデフォルトアプリ（メモ帳等）で開く。
      await openPath(logFilePath)
    } catch (e) {
      openLogError = String(e)
    }
  }

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
  refreshIndexStatus()

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

  // 0 = サーバー側のメールを削除しない（既定）。
  async function updateServerDeleteDays(days) {
    if (!settings || saving) return
    const next = { ...settings, server_delete_days: days }
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

  // ---- 設定の検索 ----
  // 各設定項目の見出し・説明文は検索対象にもなるため、テンプレートに直書きせず
  // ここに一元化している（テンプレート側は`R.xxx.title`/`R.xxx.desc`で参照する）。
  // `keywords`は画面には出ないが検索に引っかけたい語（英語名・言い換え・設定ファイルのキー名など）。
  const R = {
    theme: {
      title: 'テーマ',
      desc: '「システムに合わせる」を選ぶとOSのダーク/ライト設定に追従します。',
      keywords: 'ダークモード ライトモード dark light theme 配色',
    },
    accentColor: {
      title: 'アクセントカラー',
      desc: 'ボタンや選択中の項目などに使う差し色です。',
      keywords: 'accent color 色',
    },
    fontFamily: {
      title: 'フォント',
      desc: '一覧・本文表示に使うフォントです（PCにインストールされているフォントから選べます）。',
      keywords: 'font family 書体',
    },
    fontSize: {
      title: '文字サイズ',
      desc: '一覧・本文の文字の大きさです。',
      keywords: 'font size フォントサイズ 大きさ',
    },
    ...Object.fromEntries(ROWS.map((row) => [row.key, { ...row, keywords: row.key }])),
    syncInterval: {
      title: 'バックグラウンド自動同期の間隔',
      desc:
        'この間隔で全アカウントを自動的に同期します（平文接続のアカウントも対象です）。' +
        '検索インデックスへの投入に失敗した分もこのタイミングで' +
        '自動的に拾い直すので、通常は「検索インデックス再構築」を手動で押す必要はありません。',
      keywords: 'sync interval 自動同期 受信 POP3 分',
    },
    indexStatus: {
      title: 'インデックスの状態',
      desc: '検索できるメッセージの件数です。未投入の分は検索結果に出てきません。',
      keywords: 'index status 投入 件数 未投入 検索できない 取りこぼし',
    },
    reindex: {
      title: '検索インデックスを再構築',
      desc:
        'DB/Maildirから検索インデックスを全件作り直します。バックグラウンド同期が' +
        '投入漏れを自動的に拾い直すので通常は不要ですが、それでも検索結果に' +
        '出てこないメッセージがある場合はここから手動で再構築できます。',
      keywords: 'reindex index tantivy 全文検索',
    },
    retentionDays: {
      title: 'メールの保持日数',
      desc:
        '指定した日数より古いメールを、6時間ごとにローカルから完全に削除します' +
        '（復元できません。スター付きのメールは保持日数に関わらず削除されません）。' +
        '0は無期限（自動削除しない）です。',
      keywords: 'retention 保持期間 自動削除 古いメール',
    },
    serverDeleteDays: {
      title: 'サーバー側メールの削除（日数）',
      desc:
        'ローカルに保存済みで、指定した日数より古いメールを同期時にPOP3サーバーからも削除します' +
        '（ローカルのメールは残ります）。UIDL非対応のサーバーのアカウントは対象外です。' +
        '0は削除しません。',
      keywords: 'server delete DELE サーバー 容量',
    },
    purgeNow: {
      title: '今すぐ整理する',
      desc: '次回のバックグラウンド実行を待たず、保持日数を過ぎたメールを今すぐ削除します。',
      keywords: 'purge 削除 整理 クリーンアップ',
    },
    autoUpdateCheck: {
      title: '1日1回、更新を自動確認',
      desc: '新しいバージョンがあれば、画面上部のバナーでお知らせします（更新するかはその都度選べます）。',
      keywords: 'auto update check 自動確認 通知 バナー',
    },
    checkUpdate: {
      title: '更新を確認',
      desc:
        'GitHub Releasesに新しいバージョンがあれば、ダウンロードしてインストールし、Ferroを再起動します。' +
        '（GUI版のみ。CLI/TUIは手動で入れ替えてください）',
      keywords: 'update アップデート バージョン version 自動更新',
    },
    logFile: {
      title: 'ログファイル',
      desc:
        '同期エラー（サーバーとの接続が切れた場合など）を記録したログファイルです。' +
        '不具合を報告する際はこのファイルの内容を添えてもらえると原因の特定に役立ちます。',
      keywords: 'log ferro.log エラー 不具合',
    },
  }

  // 子コンポーネント(AccountsView等)に中身を任せているセクションは行を持たず、
  // セクション単位でだけ検索にかかる。
  const SECTIONS = {
    accounts: {
      title: 'アカウント',
      keywords:
        'account POP3 サーバー ホスト host ポート port ユーザー名 パスワード TLS 平文 accounts.toml 追加 削除 同期 sync',
      rows: [],
    },
    appearance: {
      title: '外観',
      keywords: 'appearance',
      rows: ['theme', 'accentColor', 'fontFamily', 'fontSize'],
    },
    display: { title: '表示', keywords: 'display', rows: ROWS.map((row) => row.key) },
    sync: { title: '同期', keywords: 'sync', rows: ['syncInterval'] },
    search: { title: '検索', keywords: 'search', rows: ['indexStatus', 'reindex'] },
    retention: {
      title: '保持期間',
      keywords: 'retention',
      rows: ['retentionDays', 'serverDeleteDays', 'purgeNow'],
    },
    update: { title: 'アップデート', keywords: 'update', rows: ['autoUpdateCheck', 'checkUpdate'] },
    diagnostics: { title: '診断', keywords: 'diagnostics', rows: ['logFile'] },
    colorRules: {
      title: '色分けルール',
      keywords: 'color rules 色 ハイライト 件名 差出人 color_rules.toml',
      rows: [],
    },
    labelRules: {
      title: '自動ラベル付けルール',
      keywords: 'label rules ラベル フィルタ 振り分け 受信時 label_rules.toml',
      rows: [],
    },
  }

  let searchQuery = $state('')

  // 全角/半角・大文字小文字の違いを無視する（「ＴＬＳ」でも「tls」でも当たるように）。
  function normalizeForSearch(text) {
    return text.normalize('NFKC').toLowerCase()
  }

  const searchTerms = $derived(normalizeForSearch(searchQuery).split(/\s+/).filter(Boolean))

  function matchesSearch(text) {
    const haystack = normalizeForSearch(text)
    return searchTerms.every((term) => haystack.includes(term))
  }

  // 空白区切りの語は全てAND。セクション名＋項目の文言をまとめて見るので、
  // 「同期 間隔」のように片方がセクション名にしか無い語でも当たる。
  // セクション名自体が当たった場合は、そのセクションの項目を全て表示する。
  const searchResult = $derived.by(() => {
    const sections = new Set()
    const rows = new Set()
    for (const [id, section] of Object.entries(SECTIONS)) {
      const sectionText = `${section.title} ${section.keywords}`
      for (const key of section.rows) {
        const row = R[key]
        if (matchesSearch(`${sectionText} ${row.title} ${row.desc} ${row.keywords}`)) {
          rows.add(key)
        }
      }
      if (matchesSearch(sectionText) || section.rows.some((key) => rows.has(key))) {
        sections.add(id)
      }
    }
    return { sections, rows }
  })

  function showSection(id) {
    return searchTerms.length === 0 || searchResult.sections.has(id)
  }

  function showRow(key) {
    return searchTerms.length === 0 || searchResult.rows.has(key)
  }
</script>

<div class="settings-view">
  <button type="button" class="back" onclick={onBack}>← メッセージへ戻る</button>
  <h2>設定{#if appVersion}<span class="version">v{appVersion}</span>{/if}</h2>
  <p class="lead">アカウント・表示・同期・検索の動作を調整します。</p>

  <div class="settings-search">
    <input
      type="search"
      placeholder="設定を検索（例: ダーク、同期、TLS）"
      aria-label="設定を検索"
      bind:value={searchQuery}
      onkeydown={(e) => {
        if (e.key === 'Escape') searchQuery = ''
      }}
    />
  </div>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if showSection('accounts')}
    <h2 class="section-title">{SECTIONS.accounts.title}</h2>
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
  {/if}

  {#if settings}
    {#if showSection('appearance')}
      <h2 class="section-title">{SECTIONS.appearance.title}</h2>
      <div class="rows">
        {#if showRow('theme')}
          <div class="row">
            <div class="text">
              <div class="title">{R.theme.title}</div>
              <div class="desc">{R.theme.desc}</div>
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
        {/if}
        {#if showRow('accentColor')}
          <div class="row">
            <div class="text">
              <div class="title">{R.accentColor.title}</div>
              <div class="desc">{R.accentColor.desc}</div>
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
        {/if}
        {#if showRow('fontFamily')}
          <div class="row">
            <div class="text">
              <div class="title">{R.fontFamily.title}</div>
              <div class="desc">{R.fontFamily.desc}</div>
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
        {/if}
        {#if showRow('fontSize')}
          <div class="row">
            <div class="text">
              <div class="title">{R.fontSize.title}</div>
              <div class="desc">{R.fontSize.desc}</div>
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
        {/if}
      </div>
    {/if}

    {#if showSection('display')}
      <h2 class="section-title">{SECTIONS.display.title}</h2>
      <div class="rows">
        {#each ROWS.filter((row) => showRow(row.key)) as row (row.key)}
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
    {/if}

    {#if showSection('sync')}
      <h2 class="section-title">{SECTIONS.sync.title}</h2>
      <div class="rows">
        {#if showRow('syncInterval')}
          <div class="row">
            <div class="text">
              <div class="title">{R.syncInterval.title}</div>
              <div class="desc">{R.syncInterval.desc}</div>
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
        {/if}
      </div>
    {/if}

    {#if showSection('search')}
      <h2 class="section-title">{SECTIONS.search.title}</h2>
      <div class="rows">
        {#if showRow('indexStatus')}
          <div class="row">
            <div class="text">
              <div class="title">{R.indexStatus.title}</div>
              <div class="desc">{R.indexStatus.desc}</div>
            </div>
            <button
              type="button"
              class="action-button"
              onclick={refreshIndexStatus}
              disabled={indexStatus.loading}
            >
              再確認
            </button>
          </div>
          {#if indexStatus.error}
            <p class="reindex-status"><span class="error">{indexStatus.error}</span></p>
          {:else if indexStatus.data}
            {@const s = indexStatus.data}
            {@const summary = describeIndexStatus(s)}
            <div class="index-status">
              <span class="index-status-count">
                投入済み {s.indexed.toLocaleString()} / {s.total.toLocaleString()} 件
              </span>
              <span class="index-status-docs">（索引の文書数 {s.index_docs.toLocaleString()}）</span>
              <p class="index-status-summary {summary.kind}">{summary.text}</p>
            </div>
          {/if}
        {/if}
        {#if showRow('reindex')}
          <div class="row">
            <div class="text">
              <div class="title">{R.reindex.title}</div>
              <div class="desc">{R.reindex.desc}</div>
            </div>
            <button type="button" class="action-button" onclick={runReindex} disabled={reindexer.running}>
              再構築
            </button>
          </div>
          {#if reindexer.running}
            <div class="reindex-progress">
              <progress max={Math.max(reindexer.total, 1)} value={reindexer.processed}></progress>
              <span class="reindex-status">
                再構築中… {reindexer.processed.toLocaleString()} / {reindexer.total.toLocaleString()} 件（{reindexPercent()}%）
              </span>
            </div>
          {:else if reindexer.status}
            <p class="reindex-status">{reindexer.status}</p>
          {/if}
        {/if}
      </div>
    {/if}

    {#if showSection('retention')}
      <h2 class="section-title">{SECTIONS.retention.title}</h2>
      <div class="rows">
        {#if showRow('retentionDays')}
          <div class="row">
            <div class="text">
              <div class="title">{R.retentionDays.title}</div>
              <div class="desc">{R.retentionDays.desc}</div>
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
        {/if}
        {#if showRow('serverDeleteDays')}
          <div class="row">
            <div class="text">
              <div class="title">{R.serverDeleteDays.title}</div>
              <div class="desc">{R.serverDeleteDays.desc}</div>
            </div>
            <input
              type="number"
              class="interval-select retention-input"
              min="0"
              step="1"
              value={settings.server_delete_days}
              onchange={(e) => updateServerDeleteDays(Math.max(0, Number(e.target.value) || 0))}
            />
          </div>
        {/if}
        {#if showRow('purgeNow')}
          <div class="row">
            <div class="text">
              <div class="title">{R.purgeNow.title}</div>
              <div class="desc">{R.purgeNow.desc}</div>
            </div>
            <button type="button" class="action-button danger" onclick={runPurge} disabled={purging}>
              今すぐ整理する
            </button>
          </div>
          {#if purgeStatus}
            <p class="reindex-status">{purgeStatus}</p>
          {/if}
        {/if}
      </div>
    {/if}

    {#if showSection('update')}
      <h2 class="section-title" id="update-section">{SECTIONS.update.title}</h2>
      <div class="rows">
        {#if showRow('autoUpdateCheck')}
          <div class="row">
            <div class="text">
              <div class="title">{R.autoUpdateCheck.title}</div>
              <div class="desc">{R.autoUpdateCheck.desc}</div>
            </div>
            <button
              type="button"
              class="switch"
              class:on={settings.auto_update_check}
              onclick={() => toggle('auto_update_check')}
              aria-pressed={settings.auto_update_check}
              aria-label={R.autoUpdateCheck.title}
            >
              <span class="knob"></span>
            </button>
          </div>
        {/if}
        {#if showRow('checkUpdate')}
          <div class="row">
            <div class="text">
              <div class="title">{R.checkUpdate.title}</div>
              <div class="desc">{R.checkUpdate.desc}</div>
            </div>
            <button type="button" class="action-button" onclick={checkForUpdate} disabled={updater.busy}>
              {updater.busy ? '処理中…' : '更新を確認'}
            </button>
          </div>
          {#if updater.status}
            <p class="reindex-status">{updater.status}</p>
          {/if}
          {#if updater.error}
            <p class="reindex-status"><span class="error">{updater.error}</span></p>
          {/if}
        {/if}
      </div>
    {/if}

    {#if showSection('diagnostics')}
      <h2 class="section-title">{SECTIONS.diagnostics.title}</h2>
      <div class="rows">
        {#if showRow('logFile')}
          <div class="row">
            <div class="text">
              <div class="title">{R.logFile.title}</div>
              <div class="desc">{R.logFile.desc}</div>
            </div>
            <button type="button" class="action-button" onclick={openLogFile}>
              ログファイルを開く
            </button>
          </div>
          {#if openLogError}
            <p class="reindex-status"><span class="error">{openLogError}</span></p>
          {/if}
        {/if}
      </div>
    {/if}

    {#if showSection('colorRules')}
      <h2 class="section-title">{SECTIONS.colorRules.title}</h2>
      <ColorRulesView
        rules={colorRules}
        configPath={colorRulesConfigPath}
        onAdd={onAddColorRule}
        onRemove={onRemoveColorRule}
        onReload={onReloadColorRules}
      />
    {/if}

    {#if showSection('labelRules')}
      <h2 class="section-title">{SECTIONS.labelRules.title}</h2>
      <LabelRulesView
        {labels}
        rules={labelRules}
        configPath={labelRulesConfigPath}
        onAdd={onAddLabelRule}
        onRemove={onRemoveLabelRule}
        onReload={onReloadLabelRules}
        onApplyNow={onApplyLabelRulesNow}
      />
    {/if}
  {/if}

  {#if searchTerms.length > 0 && searchResult.sections.size === 0}
    <p class="no-results">「{searchQuery}」に一致する設定はありません。</p>
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
  /* スクロールしても検索欄は上端に残す（親の.main-contentの上paddingぶん食い込ませる）。 */
  .settings-search {
    position: sticky;
    top: -20px;
    z-index: 1;
    margin-top: 1rem;
    padding: 8px 0;
    background: var(--surface);
  }
  .settings-search input {
    width: 100%;
    box-sizing: border-box;
    padding: 8px 12px;
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }
  .settings-search input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .no-results {
    margin-top: 2rem;
    color: var(--text-muted);
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
  .version {
    margin-left: 0.5rem;
    font-size: 13px;
    font-weight: normal;
    color: var(--text-muted);
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
  .reindex-progress {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 0;
  }
  .reindex-progress progress {
    width: 100%;
    height: 8px;
    accent-color: var(--accent);
  }
  .reindex-progress .reindex-status {
    padding: 0;
  }
  .index-status {
    padding: 4px 0 8px;
    font-size: 12.5px;
    color: var(--text-secondary);
  }
  .index-status-count {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .index-status-docs {
    color: var(--text-muted);
  }
  .index-status-summary {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .index-status-summary.warn {
    color: var(--danger);
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
