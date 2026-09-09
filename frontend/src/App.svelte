<script>
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { onDestroy, onMount } from 'svelte'
  import MessageList from './lib/MessageList.svelte'
  import MessageDetail from './lib/MessageDetail.svelte'
  import Sidebar from './lib/Sidebar.svelte'
  import SettingsView from './lib/SettingsView.svelte'
  import { matchColor } from './lib/colorRules.js'
  import { formatDate } from './lib/formatDate.js'
  import { applyAppearance } from './lib/appearance.js'

  // Add account/Account listはメニューバー(View > Manage Accounts…)から開く
  // 別画面として切り出している（`navigate`イベントで切り替える。下のonMount参照）。
  // 'settings'はサイドバー下部の「設定」リンクから開く。
  let currentView = 'messages'

  let accounts = []
  let error = ''
  let syncStatus = {}
  // MessageListの`{#key}`に渡し、値が変わるたびにコンポーネントを
  // 作り直させることで、最初のページから読み直させる（スクロール位置は失われる）。
  let messageListRefreshToken = 0

  // バックグラウンド同期で新着があっても、読んでいる途中でいきなり一覧の
  // スクロール位置がリセットされると使い勝手が悪いという指摘を受けて、
  // background-sync受信時はmessageListRefreshTokenを即座には動かさず、
  // 代わりにこの件数を積み上げて「新着があります」バナーで知らせるだけにする
  // （実際に一覧を作り直す＝スクロール位置がリセットされるのは、ユーザーが
  // バナーをクリックした時だけ）。
  let newMailCount = 0

  // サイドバーで選ぶフォルダ/ラベル。フォルダとラベルはどちらか一方だけを選ぶ
  // （ラベルを選んだらselectedLabelIdが優先され、MessageListはfolderを無視する）。
  let selectedFolder = 'inbox'
  let selectedLabelId = null
  let folderCounts = { inbox: 0, starred: 0, snoozed: 0, archive: 0, trash: 0 }
  let labels = []

  // MessageDetailの「既読にするまでの時間」設定に使う。設定画面から戻るたびに
  // 読み直す（SettingsView側で変更されている可能性があるため）。
  let settings = { show_preview_line: true, show_sender_avatar: false, mark_read_delay: true }

  let searchQuery = ''
  // null = 検索していない（通常のMessageList表示）。配列なら検索結果表示に切り替える。
  let searchResults = null
  let searchError = ''
  let searching = false
  let searchDebounceTimer = null
  // 入力のたびに検索を投げるため、後から入力した検索より前の検索の応答が
  // 遅れて返ってきて上書きしてしまう競合を避ける（最新のリクエストIDだけ反映する）。
  let searchRequestId = 0

  let selectedMessageId = null

  let accountsConfigPath = ''
  let reloadingConfig = false
  let reloadConfigStatus = ''

  // 特定文字列を含むメッセージの一覧表示を色分けするルール(color_rules.toml)。
  // MessageListと検索結果一覧(下の方)の両方で使うため、labels/folderCounts同様
  // ここで状態を持つ。
  let colorRules = []
  let colorRulesConfigPath = ''

  async function refreshAccounts() {
    accounts = await invoke('list_accounts')
  }

  async function refreshFolderCounts() {
    try {
      folderCounts = await invoke('get_folder_counts', { accountId: null })
    } catch (e) {
      error = String(e)
    }
  }

  async function refreshLabels() {
    try {
      labels = await invoke('list_labels')
    } catch (e) {
      error = String(e)
    }
  }

  async function refreshSettings() {
    try {
      settings = await invoke('get_settings')
      applyAppearance(settings)
    } catch (e) {
      error = String(e)
    }
  }

  // メッセージの状態が変わった（アーカイブ/スヌーズ/ラベル付与/削除）ときの共通処理。
  // 一覧を作り直し、フォルダ件数も再取得する（件数が変わりうるため）。
  function handleMessageChanged() {
    messageListRefreshToken += 1
    refreshFolderCounts()
  }

  function selectFolder(key) {
    currentView = 'messages'
    selectedFolder = key
    selectedLabelId = null
    selectedMessageId = null
    newMailCount = 0
  }

  function selectLabel(labelId) {
    currentView = 'messages'
    selectedLabelId = labelId
    selectedMessageId = null
    newMailCount = 0
  }

  // 「新着があります」バナーがクリックされた時だけ、実際に一覧を最初のページから
  // 作り直す（この時だけスクロール位置がリセットされる。ユーザー自身の操作なので、
  // 突然リセットされるのとは違い違和感が無い）。
  function refreshMessageListForNewMail() {
    messageListRefreshToken += 1
    newMailCount = 0
  }

  async function createLabel({ name, color }) {
    await invoke('create_label', { name, color })
    await refreshLabels()
  }

  async function deleteLabel(labelId) {
    try {
      await invoke('delete_label', { labelId })
      if (selectedLabelId === labelId) {
        selectFolder('inbox')
      }
      await refreshLabels()
    } catch (e) {
      error = String(e)
    }
  }

  async function refreshColorRules() {
    colorRules = await invoke('get_color_rules')
  }

  async function addColorRule({ pattern, color }) {
    colorRules = await invoke('add_color_rule', { pattern, color })
  }

  async function removeColorRule(index) {
    colorRules = await invoke('remove_color_rule', { index })
  }

  let unlistenBackgroundSync
  let unlistenSyncProgress
  let unlistenNavigate
  let unlistenRetentionPurge

  onMount(async () => {
    try {
      await refreshAccounts()
      accountsConfigPath = await invoke('accounts_config_path')
      colorRulesConfigPath = await invoke('color_rules_config_path')
      await Promise.all([refreshFolderCounts(), refreshLabels(), refreshSettings(), refreshColorRules()])
    } catch (e) {
      error = String(e)
    }

    // バックグラウンド定期同期（src-tauri側のspawn_background_sync）の結果を
    // 手動Syncボタンと同じステータス表示に反映する。
    unlistenBackgroundSync = await listen('background-sync', (event) => {
      const { account_id, fetched, remaining, ended_early, error: syncError } = event.payload
      if (syncError) {
        syncStatus = { ...syncStatus, [account_id]: `background sync error: ${syncError}` }
        return
      }
      const extra = ended_early ? ' (stopped early after repeated disconnects)' : ''
      syncStatus = {
        ...syncStatus,
        [account_id]: `background sync: fetched ${fetched}, ${remaining} remaining${extra}`,
      }
      if (fetched > 0) {
        newMailCount += fetched
        refreshFolderCounts()
      }
    })

    // sync_account_with_limitが1バッチ完了するごとに発火する進捗イベント
    // （手動Sync・背景同期どちらも共通）。"30/1000"のような表示に使う。
    unlistenSyncProgress = await listen('sync-progress', (event) => {
      const { account_id, fetched, total } = event.payload
      syncStatus = { ...syncStatus, [account_id]: `syncing… ${fetched}/${total}` }
    })

    // メニューバー(View > Messages / Manage Accounts…)のクリックで発火する
    // 画面切り替えイベント（src-tauri側の`on_menu_event`参照）。
    unlistenNavigate = await listen('navigate', (event) => {
      currentView = event.payload
    })

    // バックグラウンドのメール保持期間クリーンアップ(`spawn_retention_cleanup`)が
    // 何か削除した時に発火する。一覧の再読み込みまではしない（頻度は低いが、
    // 読んでいる最中に一覧のスクロール位置が急に変わる方が煩わしいため。
    // 新着メールの通知バナーと同じ考え方）。件数だけ最新にしておく。
    unlistenRetentionPurge = await listen('retention-purge', () => {
      refreshFolderCounts()
    })
  })

  onDestroy(() => {
    unlistenBackgroundSync?.()
    unlistenSyncProgress?.()
    unlistenNavigate?.()
    unlistenRetentionPurge?.()
  })

  // AccountsView側のフォームからそのまま渡された値を受け取り、
  // 失敗時はエラーを投げ返してAccountsView側でフォームエラーとして表示させる。
  async function addAccount({ name, host, port, username, useTls, password }) {
    await invoke('add_account', { name, host, port, username, useTls, password })
    await refreshAccounts()
  }

  async function reloadAccountsConfig() {
    reloadingConfig = true
    reloadConfigStatus = ''
    try {
      accounts = await invoke('reload_accounts_config')
      reloadConfigStatus = `loaded ${accounts.length} account(s)`
    } catch (e) {
      reloadConfigStatus = `error: ${e}`
    } finally {
      reloadingConfig = false
    }
  }

  async function removeAccount(accountId) {
    try {
      await invoke('remove_account', { accountId })
      delete syncStatus[accountId]
      syncStatus = syncStatus
      await refreshAccounts()
    } catch (e) {
      error = String(e)
    }
  }

  async function syncAccount(accountId) {
    syncStatus = { ...syncStatus, [accountId]: 'syncing…' }
    try {
      // アカウント作成時にuse_tls=falseを明示選択済みのアカウントは、
      // それ自体が平文接続へのopt-inとみなす（CLIの`--allow-plaintext`に相当）。
      // 背景同期も同じ理由でuse_tls=falseなら許可する（src-tauri/src/lib.rs参照）。
      const account = accounts.find((a) => a.id === accountId)
      const summary = await invoke('sync_account', {
        accountId,
        limit: null,
        allowPlaintext: account ? !account.use_tls : false,
      })
      const extra = summary.ended_early ? ' (stopped early after repeated disconnects)' : ''
      syncStatus = {
        ...syncStatus,
        [accountId]: `fetched ${summary.fetched}, ${summary.remaining} remaining${extra}`,
      }
      messageListRefreshToken += 1
      refreshFolderCounts()
    } catch (e) {
      syncStatus = { ...syncStatus, [accountId]: `error: ${e}` }
    }
  }

  const SEARCH_DEBOUNCE_MS = 250

  function onSearchInput() {
    clearTimeout(searchDebounceTimer)
    const query = searchQuery.trim()
    if (!query) {
      // 空になったら直前にスケジュール済みの検索も含めて即座に一覧表示へ戻す。
      searchRequestId += 1
      searchResults = null
      searchError = ''
      searching = false
      return
    }
    searching = true
    searchDebounceTimer = setTimeout(() => runSearch(query), SEARCH_DEBOUNCE_MS)
  }

  async function runSearch(query) {
    const requestId = ++searchRequestId
    try {
      const results = await invoke('search_messages', { query, limit: 50 })
      if (requestId !== searchRequestId) return // より新しい検索が既に走っている
      searchResults = results
      searchError = ''
    } catch (e) {
      if (requestId !== searchRequestId) return
      searchError = String(e)
      searchResults = []
    } finally {
      if (requestId === searchRequestId) searching = false
    }
  }

  function clearSearch() {
    clearTimeout(searchDebounceTimer)
    searchRequestId += 1
    searchQuery = ''
    searchResults = null
    searchError = ''
    searching = false
  }
</script>

<main>
  {#if error}
    <p class="error top-error">{error}</p>
  {/if}

  <div class="app-layout">
    <Sidebar
      {folderCounts}
      {labels}
      {selectedFolder}
      {selectedLabelId}
      onSelectFolder={selectFolder}
      onSelectLabel={selectLabel}
      onCreateLabel={createLabel}
      onDeleteLabel={deleteLabel}
      onOpenSettings={() => (currentView = 'settings')}
    />

    <div class="main-content">
      {#if currentView === 'settings'}
        <SettingsView
          {accounts}
          {accountsConfigPath}
          {syncStatus}
          {reloadingConfig}
          {reloadConfigStatus}
          onAddAccount={addAccount}
          onRemoveAccount={removeAccount}
          onSyncAccount={syncAccount}
          onReloadAccountsConfig={reloadAccountsConfig}
          {colorRules}
          {colorRulesConfigPath}
          onAddColorRule={addColorRule}
          onRemoveColorRule={removeColorRule}
          onReloadColorRules={refreshColorRules}
          onBack={() => {
            currentView = 'messages'
            refreshSettings()
          }}
        />
      {:else}
        <section class="messages-view">
            <div class="search-bar">
              <input
                placeholder="件名/差出人/本文を検索…"
                bind:value={searchQuery}
                on:input={onSearchInput}
              />
              {#if searching}
                <span class="search-spinner">検索中…</span>
              {/if}
              {#if searchResults !== null}
                <button type="button" on:click={clearSearch}>クリア</button>
              {/if}
            </div>
            {#if searchError}
              <p class="error">{searchError}</p>
            {/if}

            {#if searchResults === null && selectedFolder === 'inbox' && selectedLabelId === null && newMailCount > 0}
              <button type="button" class="new-mail-banner" on:click={refreshMessageListForNewMail}>
                新着メッセージが{newMailCount}件あります（クリックで表示）
              </button>
            {/if}

            <div class="messages-layout">
              <div class="list-pane">
                {#if searchResults !== null}
                  <ul class="search-results">
                    {#if searchResults.length === 0 && !searching}
                      <li class="empty">該当するメッセージがありません。</li>
                    {/if}
                    {#each searchResults as message (message.id)}
                      {@const rowColor = matchColor(colorRules, message)}
                      <li>
                        <button
                          type="button"
                          class="result-row"
                          on:click={() => (selectedMessageId = message.id)}
                        >
                          <span class="from" style={rowColor ? `color: ${rowColor}` : ''}>{message.from_name ?? message.from_addr ?? '(unknown sender)'}</span>
                          <span class="subject" style={rowColor ? `color: ${rowColor}` : ''}>{message.subject ?? '(no subject)'}</span>
                          <span class="date">{formatDate(message.date_header)}</span>
                        </button>
                      </li>
                    {/each}
                  </ul>
                {:else}
                  {#key messageListRefreshToken}
                    <MessageList
                      folder={selectedFolder}
                      labelId={selectedLabelId}
                      {colorRules}
                      onSelect={(id) => (selectedMessageId = id)}
                    />
                  {/key}
                {/if}
              </div>

              {#if selectedMessageId !== null}
                <div class="detail-pane">
                  <MessageDetail
                    messageId={selectedMessageId}
                    allLabels={labels}
                    markReadDelay={settings.mark_read_delay}
                    onClose={() => (selectedMessageId = null)}
                    onChanged={handleMessageChanged}
                  />
                </div>
              {/if}
            </div>
          </section>
      {/if}
    </div>
  </div>
</main>

<style>
  main {
    margin: 0;
    height: 100%;
    padding: 1rem;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
  }
  .app-layout {
    display: flex;
    align-items: stretch;
    flex: 1;
    min-height: 0;
    gap: 0;
    border: 1px solid var(--border);
    border-radius: 14px;
    overflow: hidden;
    background: var(--surface);
    box-shadow:
      0 1px 0 rgba(28, 27, 24, 0.06),
      0 18px 40px -24px rgba(28, 27, 24, 0.35);
  }
  .main-content {
    flex: 1;
    min-width: 0;
    min-height: 0;
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  .messages-view {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .messages-layout {
    display: flex;
    align-items: stretch;
    flex: 1;
    min-height: 0;
    gap: 1rem;
  }
  .list-pane {
    flex: 1.2 1 420px;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .detail-pane {
    flex: 1 1 420px;
    min-width: 0;
    min-height: 0;
  }
  .error {
    color: var(--danger);
  }
  .top-error {
    margin-bottom: 1rem;
  }
  .search-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.75rem;
  }
  .search-bar input {
    flex: 1 1 auto;
    min-width: 200px;
    padding: 8px 11px;
    border: 1px solid var(--border);
    border-radius: 8px;
    font: inherit;
    font-size: 13px;
  }
  .search-bar button {
    border: 1px solid var(--border);
    background: var(--surface-subtle);
    border-radius: 7px;
    padding: 7px 14px;
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .search-spinner {
    color: var(--text-muted);
    font-size: 12.5px;
  }
  .new-mail-banner {
    display: block;
    width: 100%;
    margin-bottom: 0.75rem;
    padding: 8px 14px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--accent-soft-bg);
    color: var(--accent-hover);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    text-align: left;
    cursor: pointer;
  }
  .new-mail-banner:hover {
    background: var(--accent-soft-bg);
    border-color: var(--accent-hover);
  }
  .search-bar button:hover {
    background: var(--surface-muted);
  }
  .search-results {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    background: var(--surface-subtle);
  }
  .search-results li {
    border-bottom: 1px solid var(--border);
  }
  .search-results li.empty {
    padding: 0.6rem 0.75rem;
    color: var(--text-muted);
  }
  .result-row {
    display: flex;
    width: 100%;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border: none;
    background: none;
    font: inherit;
    text-align: left;
    cursor: pointer;
    box-sizing: border-box;
  }
  .result-row:hover {
    background: var(--surface-muted);
  }
  .search-results .from {
    flex: 0 0 160px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .search-results .subject {
    flex: 1 1 auto;
    min-width: 0;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .search-results .date {
    flex: none;
    font-size: 11px;
    color: var(--text-faint);
  }
</style>
