<script>
  import { invoke } from '@tauri-apps/api/core'
  import { onMount } from 'svelte'
  import MessageList from './lib/MessageList.svelte'

  let accounts = []
  let error = ''
  let syncStatus = {}
  // MessageListの`{#key}`に渡し、値が変わるたびにコンポーネントを
  // 作り直させることで、syncで増えたメッセージを最初のページから読み直させる。
  let messageListRefreshToken = 0

  let form = { name: '', host: '', port: 995, username: '', useTls: true, password: '' }
  let formError = ''
  let submitting = false

  let searchQuery = ''
  // null = 検索していない（通常のMessageList表示）。配列なら検索結果表示に切り替える。
  let searchResults = null
  let searchError = ''
  let searching = false

  let reindexStatus = ''
  let reindexing = false

  async function refreshAccounts() {
    accounts = await invoke('list_accounts')
  }

  onMount(async () => {
    try {
      await refreshAccounts()
    } catch (e) {
      error = String(e)
    }
  })

  async function addAccount() {
    formError = ''
    submitting = true
    try {
      await invoke('add_account', {
        name: form.name,
        host: form.host,
        port: Number(form.port),
        username: form.username,
        useTls: form.useTls,
        password: form.password,
      })
      form = { name: '', host: '', port: 995, username: '', useTls: true, password: '' }
      await refreshAccounts()
    } catch (e) {
      formError = String(e)
    } finally {
      submitting = false
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
      const summary = await invoke('sync_account', {
        accountId,
        limit: null,
        allowPlaintext: false,
      })
      const extra = summary.ended_early ? ' (stopped early after repeated disconnects)' : ''
      syncStatus = {
        ...syncStatus,
        [accountId]: `fetched ${summary.fetched}, ${summary.remaining} remaining${extra}`,
      }
      messageListRefreshToken += 1
    } catch (e) {
      syncStatus = { ...syncStatus, [accountId]: `error: ${e}` }
    }
  }

  async function runSearch() {
    if (!searchQuery.trim()) return
    searching = true
    searchError = ''
    try {
      searchResults = await invoke('search_messages', { query: searchQuery, limit: 50 })
    } catch (e) {
      searchError = String(e)
      searchResults = []
    } finally {
      searching = false
    }
  }

  function clearSearch() {
    searchQuery = ''
    searchResults = null
    searchError = ''
  }

  async function runReindex() {
    reindexing = true
    reindexStatus = 'rebuilding…'
    try {
      const count = await invoke('reindex_all')
      reindexStatus = `reindexed ${count} message(s)`
    } catch (e) {
      reindexStatus = `error: ${e}`
    } finally {
      reindexing = false
    }
  }
</script>

<main>
  <h1>Ferro</h1>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <section>
    <h2>Add account</h2>
    <form on:submit|preventDefault={addAccount}>
      <input placeholder="Name" bind:value={form.name} required />
      <input placeholder="Host" bind:value={form.host} required />
      <input type="number" placeholder="Port" bind:value={form.port} required min="1" max="65535" />
      <input placeholder="Username" bind:value={form.username} required />
      <label>
        <input type="checkbox" bind:checked={form.useTls} />
        Use TLS
      </label>
      <input type="password" placeholder="Password" bind:value={form.password} required />
      <button type="submit" disabled={submitting}>Add account</button>
    </form>
    {#if formError}
      <p class="error">{formError}</p>
    {/if}
  </section>

  <section>
    <h2>Accounts ({accounts.length})</h2>
    {#if accounts.length === 0}
      <p>No accounts yet.</p>
    {:else}
      <ul>
        {#each accounts as account (account.id)}
          <li>
            #{account.id} {account.name} — {account.username}@{account.host}:{account.port}
            <button on:click={() => syncAccount(account.id)}>Sync</button>
            <button on:click={() => removeAccount(account.id)}>Remove</button>
            {#if syncStatus[account.id]}
              <span class="status">{syncStatus[account.id]}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section>
    <h2>Messages</h2>

    <form class="search-bar" on:submit|preventDefault={runSearch}>
      <input placeholder="Search subject/from/body…" bind:value={searchQuery} />
      <button type="submit" disabled={searching || !searchQuery.trim()}>Search</button>
      {#if searchResults !== null}
        <button type="button" on:click={clearSearch}>Clear</button>
      {/if}
      <button type="button" on:click={runReindex} disabled={reindexing}>
        Rebuild search index
      </button>
      {#if reindexStatus}
        <span class="status">{reindexStatus}</span>
      {/if}
    </form>
    {#if searchError}
      <p class="error">{searchError}</p>
    {/if}

    {#if searchResults !== null}
      <ul class="search-results">
        {#if searchResults.length === 0 && !searching}
          <li class="empty">No matches.</li>
        {/if}
        {#each searchResults as message (message.id)}
          <li>
            <span class="from">{message.from_name ?? message.from_addr ?? '(unknown sender)'}</span>
            <span class="subject">{message.subject ?? '(no subject)'}</span>
          </li>
        {/each}
      </ul>
    {:else}
      {#key messageListRefreshToken}
        <MessageList accountId={null} />
      {/key}
    {/if}
  </section>
</main>

<style>
  main {
    max-width: 720px;
    margin: 2rem auto;
    padding: 0 1rem;
    font-family: system-ui, sans-serif;
  }
  form {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: center;
  }
  .error {
    color: #b00020;
  }
  .status {
    margin-left: 0.5rem;
    color: #555;
    font-size: 0.9em;
  }
  .search-bar {
    margin-bottom: 0.75rem;
  }
  .search-bar input {
    flex: 1 1 auto;
    min-width: 200px;
  }
  .search-results {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid #ddd;
    border-radius: 4px;
    max-height: 420px;
    overflow-y: auto;
  }
  .search-results li {
    display: flex;
    gap: 0.75rem;
    padding: 0.4rem 0.5rem;
    border-bottom: 1px solid #eee;
  }
  .search-results li.empty {
    color: #666;
  }
  .search-results .from {
    flex: 0 0 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .search-results .subject {
    flex: 1 1 auto;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
