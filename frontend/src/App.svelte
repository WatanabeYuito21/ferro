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
    {#key messageListRefreshToken}
      <MessageList accountId={null} />
    {/key}
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
</style>
