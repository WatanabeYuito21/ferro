<script>
  import { invoke } from '@tauri-apps/api/core'
  import { onMount } from 'svelte'

  let accounts = []
  let messages = []
  let error = ''

  onMount(async () => {
    try {
      accounts = await invoke('list_accounts')
      messages = await invoke('list_messages', { accountId: null, before: null, limit: 20 })
    } catch (e) {
      error = String(e)
    }
  })
</script>

<main>
  <h1>Ferro</h1>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <section>
    <h2>Accounts ({accounts.length})</h2>
    {#if accounts.length === 0}
      <p>No accounts yet. Add one with <code>ferro account add</code>.</p>
    {:else}
      <ul>
        {#each accounts as account (account.id)}
          <li>#{account.id} {account.name} — {account.username}@{account.host}:{account.port}</li>
        {/each}
      </ul>
    {/if}
  </section>

  <section>
    <h2>Recent messages ({messages.length})</h2>
    {#if messages.length === 0}
      <p>No messages yet.</p>
    {:else}
      <ul>
        {#each messages as message (message.id)}
          <li>{message.subject ?? '(no subject)'} — {message.from_addr ?? '(unknown sender)'}</li>
        {/each}
      </ul>
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
  .error {
    color: #b00020;
  }
</style>
