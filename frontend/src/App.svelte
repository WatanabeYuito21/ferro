<script>
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { onDestroy, onMount, tick } from 'svelte'
  import MessageList from './lib/MessageList.svelte'
  import MessageDetail from './lib/MessageDetail.svelte'
  import Sidebar from './lib/Sidebar.svelte'
  import SettingsView from './lib/SettingsView.svelte'
  import KeyboardHelp from './lib/KeyboardHelp.svelte'
  import BulkActionBar from './lib/BulkActionBar.svelte'
  import AdvancedSearch from './lib/AdvancedSearch.svelte'
  import { applyPatch, neighborId, clickSelection, toggleChecked } from './lib/messageView.js'
  import { matchColor } from './lib/colorRules.js'
  import { formatDate } from './lib/formatDate.js'
  import { applyAppearance } from './lib/appearance.js'
  import {
    updater,
    checkForUpdate,
    installAvailableUpdate,
    dismissAvailableUpdate,
    startDailyUpdateCheck,
  } from './lib/updater.svelte.js'

  // Add account/Account listはメニューバー(View > Manage Accounts…)から開く
  // 別画面として切り出している（`navigate`イベントで切り替える。下のonMount参照）。
  // 'settings'はサイドバー下部の「設定」リンクから開く。
  let currentView = 'messages'

  let accounts = []
  let error = ''
  let syncStatus = {}
  // 手動Sync実行中のアカウントid。この間は自動同期由来のイベント
  // (background-sync)で手動Syncの表示を上書きしない。
  let manualSyncing = new Set()
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
  let showAdvancedSearch = false

  let selectedMessageId = null

  // キーボード操作（`handleKeydown`）から一覧・本文の操作を呼ぶための参照。
  let messageList
  let messageDetail
  let searchInput
  let showKeyboardHelp = false
  // `g`を押した直後なら、次のキーでフォルダへ移動する（`g`→`i`で受信箱、等）。
  let pendingGoTo = false
  let pendingGoToTimer = null

  // 複数選択（一括操作用）。開いているメッセージ(`selectedMessageId`)とは別に持つ。
  // フォルダ/ラベル/検索を切り替えたら解除する。
  let checkedIds = new Set()
  let checkAnchor = null
  let bulkBusy = false

  let accountsConfigPath = ''
  let reloadingConfig = false
  let reloadConfigStatus = ''
  let logFilePath = ''
  let appVersion = ''

  // 特定文字列を含むメッセージの一覧表示を色分けするルール(color_rules.toml)。
  // MessageListと検索結果一覧(下の方)の両方で使うため、labels/folderCounts同様
  // ここで状態を持つ。
  let colorRules = []
  let colorRulesConfigPath = ''

  // 受信時に自動でラベルを付与するルール(label_rules.toml)。設定画面でのみ使うが、
  // color_rulesと同じくApp.svelte側で状態を持つ（invoke呼び出しをApp.svelteに
  // 集約する既存の方針に揃えるため）。
  let labelRules = []
  let labelRulesConfigPath = ''

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
  // 1件ずつの操作は`handleMessagePatched`で行の更新だけにする。こちらは
  // 「今すぐ適用」のように多数のメッセージが一度に変わる場合用。
  function handleMessageChanged() {
    messageListRefreshToken += 1
    refreshFolderCounts()
  }

  // 本文表示（またはキーボード操作）で1件の状態が変わったとき。一覧を作り直さず
  // 該当行だけ更新し（スクロール位置を保つ）、表示中のフォルダ/ラベルから外れたら
  // 次のメッセージへ進む（無ければ本文を閉じる）。
  function handleMessagePatched(id, patch) {
    const result = applyToList(id, patch)
    if (result?.removed && selectedMessageId === id) {
      selectedMessageId = result.nextId
    }
    // フォルダ件数は既読/未読に関係しない（全件数）ので、既読化だけなら数え直さない
    // （本文を開くたびに全フォルダのCOUNTを取り直すことになるため）。
    if (Object.keys(patch).some((key) => key !== 'is_read')) refreshFolderCounts()
    if ('labels' in patch) refreshLabels()
  }

  // 一覧（検索中なら検索結果）の該当行に`patch`を当てる。外れた行は取り除かれる。
  function applyToList(id, patch) {
    if (searchResults !== null) {
      const result = applyPatch(searchResults, id, patch, { kind: 'search' })
      searchResults = result.items
      return result
    }
    return messageList?.applyChange(id, patch)
  }

  // 文字入力中の要素か（チェックボックス等の文字を打たない<input>は含めない。
  // 一覧のチェックボックスをクリックした直後もショートカットが効くように）。
  function isTypingTarget(target) {
    if (!(target instanceof HTMLElement)) return false
    if (target.isContentEditable || ['TEXTAREA', 'SELECT'].includes(target.tagName)) return true
    return target.tagName === 'INPUT' && !['checkbox', 'radio', 'button'].includes(target.type)
  }

  function scrollSearchResultIntoView(id) {
    tick().then(() => {
      document.querySelector(`[data-search-result-id="${id}"]`)?.scrollIntoView({ block: 'nearest' })
    })
  }

  function moveSelection(delta) {
    let id
    if (searchResults !== null) {
      id = neighborId(searchResults, selectedMessageId, delta)
      if (id != null) scrollSearchResultIntoView(id)
    } else {
      id = messageList?.moveSelection(delta) ?? null
    }
    if (id != null) selectedMessageId = id
  }

  const GO_TO_FOLDER = { i: 'inbox', s: 'starred', z: 'snoozed', a: 'archive', t: 'trash' }

  // キーボードショートカット（一覧は`KeyboardHelp.svelte`。変えるときは両方を揃える）。
  // 入力欄にフォーカスがある間は、Escで入力欄から抜ける以外は何もしない。
  function handleKeydown(event) {
    if (event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return
    if (isTypingTarget(event.target)) {
      if (event.key === 'Escape') event.target.blur()
      return
    }
    if (showKeyboardHelp) {
      if (event.key === 'Escape' || event.key === '?') {
        showKeyboardHelp = false
        event.preventDefault()
      }
      return
    }
    if (event.key === '?') {
      showKeyboardHelp = true
      event.preventDefault()
      return
    }
    if (currentView !== 'messages') return

    if (pendingGoTo) {
      pendingGoTo = false
      clearTimeout(pendingGoToTimer)
      const folder = GO_TO_FOLDER[event.key]
      if (folder) {
        clearSearch()
        selectFolder(folder)
        event.preventDefault()
      }
      return
    }

    // 複数選択中は、本文に対する操作の代わりに選択中の全メッセージへ一括で行う。
    if (checkedIds.size > 0 && ['e', 's', 'u', '#', 'Delete'].includes(event.key)) {
      runBulkFromKey(event.key)
      event.preventDefault()
      return
    }

    const detail = selectedMessageId !== null ? messageDetail : null
    let handled = true
    switch (event.key) {
      case 'j':
      case 'ArrowDown':
        moveSelection(1)
        break
      case 'k':
      case 'ArrowUp':
        moveSelection(-1)
        break
      case 'Enter':
      case 'o':
        // ボタン等にフォーカスがある時のEnterは、そのボタンを押す本来の動作に任せる。
        if (event.key === 'Enter' && event.target.closest?.('button, a, [role="button"]')) {
          handled = false
        } else if (selectedMessageId === null) {
          moveSelection(0)
        }
        break
      case 'x':
        if (selectedMessageId !== null) {
          checkedIds = toggleChecked(checkedIds, selectedMessageId)
          checkAnchor = selectedMessageId
        }
        break
      case 'Escape':
        if (checkedIds.size > 0) clearChecked()
        else if (selectedMessageId !== null) selectedMessageId = null
        else if (searchResults !== null) clearSearch()
        else handled = false
        break
      case '/':
        searchInput?.focus()
        break
      case 'g':
        pendingGoTo = true
        // 少し待っても次のキーが来なければ取り消す。
        pendingGoToTimer = setTimeout(() => (pendingGoTo = false), 1500)
        break
      case 'e':
        detail?.toggleArchive()
        break
      case 's':
        detail?.toggleFlag()
        break
      case 'u':
        detail?.toggleRead()
        break
      case '#':
      case 'Delete':
        detail?.deleteMessage()
        break
      case 'b':
        detail?.toggleSnoozeMenu()
        break
      case 'l':
        detail?.toggleLabelPicker()
        break
      default:
        handled = false
    }
    if (handled) event.preventDefault()
  }

  function selectFolder(key) {
    currentView = 'messages'
    selectedFolder = key
    selectedLabelId = null
    selectedMessageId = null
    newMailCount = 0
    clearChecked()
  }

  function selectLabel(labelId) {
    currentView = 'messages'
    selectedLabelId = labelId
    selectedMessageId = null
    newMailCount = 0
    clearChecked()
  }

  function clearChecked() {
    checkedIds = new Set()
    checkAnchor = null
  }

  function setChecked(checked, anchor) {
    checkedIds = checked
    checkAnchor = anchor
  }

  // いま一覧に出ている行（検索中なら検索結果）。
  function visibleItems() {
    return searchResults ?? messageList?.getItems() ?? []
  }

  function onSearchResultClick(id, event) {
    const result = clickSelection(searchResults, checkedIds, checkAnchor, id, event)
    setChecked(result.checked, result.anchor)
    if (result.open) selectedMessageId = id
  }

  const SNOOZE_SECONDS = { '1d': 24 * 60 * 60, '3d': 3 * 24 * 60 * 60, '1w': 7 * 24 * 60 * 60 }

  // 一括操作の結果を一覧の行に当てるための変更内容（`messageView.js`の`applyPatch`）。
  function bulkPatch(action) {
    switch (action.kind) {
      case 'read':
        return { is_read: action.value }
      case 'flag':
        return { is_flagged: action.value }
      case 'archive':
        return { is_archived: action.value }
      case 'snooze':
        return {
          snoozed_until:
            action.preset == null ? null : Math.floor(Date.now() / 1000) + SNOOZE_SECONDS[action.preset],
        }
      case 'delete':
        return { is_deleted: true }
      case 'label': {
        const label = labels.find((l) => l.id === action.label_id)
        return (message) => {
          const others = message.labels.filter((l) => l.id !== action.label_id)
          return {
            labels:
              action.assigned && label
                ? [...others, { id: label.id, name: label.name, color: label.color }]
                : others,
          }
        }
      }
    }
    return {}
  }

  async function runBulk(action) {
    const ids = [...checkedIds]
    if (ids.length === 0 || bulkBusy) return
    bulkBusy = true
    try {
      await invoke('bulk_update', { messageIds: ids, action })
      const patch = bulkPatch(action)
      for (const id of ids) applyToList(id, patch)
      // 開いている本文の表示は古くなるので閉じる（一覧の行は上で更新済み）。
      if (ids.includes(selectedMessageId)) selectedMessageId = null
      clearChecked()
      refreshFolderCounts()
      if (action.kind === 'label') refreshLabels()
    } catch (e) {
      error = String(e)
    } finally {
      bulkBusy = false
    }
  }

  // キーボードの`e`/`s`/`u`/`#`を、選択中の全メッセージに対して行う。
  // スター・既読は、全部が既にその状態なら外す（1件の時の切り替えと同じ感覚にする）。
  function runBulkFromKey(key) {
    const items = visibleItems().filter((m) => checkedIds.has(m.id))
    switch (key) {
      case 'e':
        runBulk({
          kind: 'archive',
          value: !(selectedLabelId === null && selectedFolder === 'archive'),
        })
        break
      case 's':
        runBulk({ kind: 'flag', value: !items.every((m) => m.is_flagged) })
        break
      case 'u':
        runBulk({ kind: 'read', value: items.some((m) => !m.is_read) })
        break
      case '#':
      case 'Delete':
        runBulk({ kind: 'delete' })
        break
    }
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

  async function refreshLabelRules() {
    labelRules = await invoke('get_label_rules')
  }

  async function addLabelRule({ labelName, fromContains, subjectContains, bodyContains, hasAttachment }) {
    labelRules = await invoke('add_label_rule', {
      labelName,
      fromContains,
      subjectContains,
      bodyContains,
      hasAttachment,
    })
  }

  async function removeLabelRule(index) {
    labelRules = await invoke('remove_label_rule', { index })
  }

  // 既存の全メッセージへ遡ってルールを適用する（「今すぐ適用」ボタン）。ラベル付与の
  // 結果を反映するため、メッセージ一覧・フォルダ件数を作り直す
  // （`handleMessageChanged`と同じ理由）。
  async function applyLabelRulesNow() {
    const applied = await invoke('apply_label_rules_now')
    handleMessageChanged()
    return applied
  }

  let unlistenBackgroundSync
  let unlistenSyncProgress
  let unlistenNavigate
  let unlistenCheckForUpdate
  let stopDailyUpdateCheck
  let unlistenRetentionPurge

  onMount(async () => {
    try {
      await refreshAccounts()
      accountsConfigPath = await invoke('accounts_config_path')
      colorRulesConfigPath = await invoke('color_rules_config_path')
      labelRulesConfigPath = await invoke('label_rules_config_path')
      logFilePath = await invoke('log_file_path')
      appVersion = await invoke('app_version')
      await Promise.all([
        refreshFolderCounts(),
        refreshLabels(),
        refreshSettings(),
        refreshColorRules(),
        refreshLabelRules(),
      ])
    } catch (e) {
      error = String(e)
    }

    // 1日1回の更新確認。設定は設定画面での変更後に`refreshSettings`で
    // 読み直されるので、毎回`settings`を見る。
    stopDailyUpdateCheck = startDailyUpdateCheck(() => settings.auto_update_check !== false)

    // バックグラウンド定期同期（src-tauri側のspawn_background_sync）の結果を
    // 手動Syncボタンと同じステータス表示に反映する。
    unlistenBackgroundSync = await listen('background-sync', (event) => {
      const { account_id, fetched, remaining, ended_early, error: syncError } = event.payload
      if (manualSyncing.has(account_id)) return
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

    // メニューバー(View > Check for Updates…)。進捗・結果は設定画面の
    // 「アップデート」セクションに出すので、そこへ移動してから確認を始める。
    unlistenCheckForUpdate = await listen('check-for-update', async () => {
      currentView = 'settings'
      checkForUpdate()
      // 「アップデート」セクションは設定の読み込み(非同期)が終わってから描画されるので、
      // 現れるまで少し待ってからスクロールする。
      for (let i = 0; i < 20; i++) {
        await tick()
        const section = document.getElementById('update-section')
        if (section) {
          section.scrollIntoView({ block: 'start' })
          break
        }
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
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
    unlistenCheckForUpdate?.()
    stopDailyUpdateCheck?.()
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
    manualSyncing.add(accountId)
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
    } finally {
      manualSyncing.delete(accountId)
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
    clearChecked()
    try {
      const results = await invoke('search_messages', {
        query,
        limit: 50,
        // `after:`/`before:`の日付を利用者のタイムゾーンの0時で区切るため。
        utcOffsetSeconds: -new Date().getTimezoneOffset() * 60,
      })
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

  // 詳細検索フォームから。組み立てた書式を検索欄に入れて、そのまま検索する
  // （検索欄で書式を直して検索し直せるように）。
  function runAdvancedSearch(query) {
    searchQuery = query
    showAdvancedSearch = false
    clearTimeout(searchDebounceTimer)
    searching = true
    runSearch(query)
  }

  function clearSearch() {
    clearChecked()
    clearTimeout(searchDebounceTimer)
    searchRequestId += 1
    searchQuery = ''
    searchResults = null
    searchError = ''
    searching = false
  }
</script>

<svelte:window on:keydown={handleKeydown} />

{#if showKeyboardHelp}
  <KeyboardHelp onClose={() => (showKeyboardHelp = false)} />
{/if}

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
      {#if updater.available && !updater.dismissed}
        <div class="update-banner" role="status">
          <span class="update-banner-text">
            {#if updater.busy || updater.status}
              {updater.status}
            {:else if updater.error}
              更新に失敗しました: {updater.error}
            {:else}
              新しいバージョン v{updater.available.version} があります（現在 v{updater.available.currentVersion}）
            {/if}
          </span>
          <button
            type="button"
            class="update-banner-primary"
            on:click={installAvailableUpdate}
            disabled={updater.busy}
          >
            今すぐ更新
          </button>
          <button type="button" on:click={dismissAvailableUpdate} disabled={updater.busy}>後で</button>
        </div>
      {/if}
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
          {labels}
          {labelRules}
          {labelRulesConfigPath}
          onAddLabelRule={addLabelRule}
          onRemoveLabelRule={removeLabelRule}
          onReloadLabelRules={refreshLabelRules}
          onApplyLabelRulesNow={applyLabelRulesNow}
          {logFilePath}
          {appVersion}
          onBack={() => {
            currentView = 'messages'
            refreshSettings()
          }}
        />
      {:else}
        <section class="messages-view">
            <div class="search-bar">
              <input
                placeholder="件名/差出人/本文を検索…（from: has:attachment などの書式も可。/ で移動）"
                bind:this={searchInput}
                bind:value={searchQuery}
                on:input={onSearchInput}
              />
              {#if searching}
                <span class="search-spinner">検索中…</span>
              {/if}
              {#if searchResults !== null}
                <button type="button" on:click={clearSearch}>クリア</button>
              {/if}
              <button
                type="button"
                class:active={showAdvancedSearch}
                on:click={() => (showAdvancedSearch = !showAdvancedSearch)}
              >
                詳細
              </button>
              <button
                type="button"
                class="shortcut-hint"
                title="キーボードショートカット"
                on:click={() => (showKeyboardHelp = true)}
              >
                ?
              </button>
            </div>
            {#if showAdvancedSearch}
              <AdvancedSearch
                {labels}
                onSearch={runAdvancedSearch}
                onClose={() => (showAdvancedSearch = false)}
              />
            {/if}
            {#if searchError}
              <p class="error">{searchError}</p>
            {/if}

            {#if searchResults === null && selectedFolder === 'inbox' && selectedLabelId === null && newMailCount > 0}
              <button type="button" class="new-mail-banner" on:click={refreshMessageListForNewMail}>
                新着メッセージが{newMailCount}件あります（クリックで表示）
              </button>
            {/if}

            {#if checkedIds.size > 0}
              <BulkActionBar
                count={checkedIds.size}
                folder={selectedFolder}
                labelId={searchResults === null ? selectedLabelId : null}
                {labels}
                busy={bulkBusy}
                onAction={runBulk}
                onSelectAll={() => setChecked(new Set(visibleItems().map((m) => m.id)), checkAnchor)}
                onClear={clearChecked}
              />
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
                        <div
                          class="result-row"
                          class:selected={message.id === selectedMessageId}
                          class:checked={checkedIds.has(message.id)}
                          class:unread={!message.is_read}
                          data-search-result-id={message.id}
                          role="button"
                          tabindex="0"
                          on:click={(e) => onSearchResultClick(message.id, e)}
                          on:keydown={(e) => e.key === 'Enter' && (selectedMessageId = message.id)}
                        >
                          <input
                            type="checkbox"
                            class="check"
                            aria-label="選択"
                            checked={checkedIds.has(message.id)}
                            on:click|stopPropagation={() => setChecked(toggleChecked(checkedIds, message.id), message.id)}
                          />
                          <span class="from" style={rowColor ? `color: ${rowColor}` : ''}>{message.from_name ?? message.from_addr ?? '(unknown sender)'}</span>
                          <span class="subject" style={rowColor ? `color: ${rowColor}` : ''}>{message.subject ?? '(no subject)'}</span>
                          <span class="date">{formatDate(message.date_header)}</span>
                        </div>
                      </li>
                    {/each}
                  </ul>
                {:else}
                  {#key messageListRefreshToken}
                    <MessageList
                      bind:this={messageList}
                      folder={selectedFolder}
                      labelId={selectedLabelId}
                      selectedId={selectedMessageId}
                      {checkedIds}
                      {checkAnchor}
                      {colorRules}
                      onSelect={(id) => (selectedMessageId = id)}
                      onCheck={setChecked}
                    />
                  {/key}
                {/if}
              </div>

              {#if selectedMessageId !== null}
                <div class="detail-pane">
                  <MessageDetail
                    bind:this={messageDetail}
                    messageId={selectedMessageId}
                    allLabels={labels}
                    markReadDelay={settings.mark_read_delay}
                    onClose={() => (selectedMessageId = null)}
                    onChanged={handleMessagePatched}
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
  .update-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 0.75rem;
    padding: 8px 14px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--accent-soft-bg);
    color: var(--accent-hover);
    font-size: 13px;
    font-weight: 500;
  }
  .update-banner-text {
    flex: 1;
    min-width: 0;
  }
  .update-banner button {
    flex: none;
    padding: 4px 12px;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
  }
  .update-banner button.update-banner-primary {
    border-color: var(--accent);
    background: var(--accent);
    color: #fff;
  }
  .update-banner button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .new-mail-banner:hover {
    background: var(--accent-soft-bg);
    border-color: var(--accent-hover);
  }
  .search-bar button:hover {
    background: var(--surface-muted);
  }
  .search-bar button.active {
    border-color: var(--accent);
    color: var(--accent-hover);
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
  .result-row.selected {
    background: var(--accent-soft-bg);
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .result-row.checked {
    background: var(--accent-soft-bg);
  }
  .result-row {
    user-select: none;
  }
  .result-row .check {
    flex: none;
    align-self: center;
    margin: 0;
    accent-color: var(--accent);
    cursor: pointer;
  }
  .search-results .result-row:not(.unread) .subject {
    font-weight: 400;
  }
  .search-bar .shortcut-hint {
    padding: 7px 11px;
    color: var(--text-muted);
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
