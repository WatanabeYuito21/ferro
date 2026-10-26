<script>
  // 設定画面(SettingsView)内の「自動ラベル付けルール」セクション。ColorRulesView.svelteと
  // 同じ構成（状態はApp.svelte側、ここは表示とフォームのローカル状態だけ）だが、
  // こちらは表示だけでなく実際にDBへラベルを付与する機能なので「今すぐ適用」
  // （保持期間の「今すぐ整理する」と同じ思想）も持つ。
  import { openPath } from '@tauri-apps/plugin-opener'

  let {
    labels = [],
    rules = [],
    configPath = '',
    onAdd = async () => {},
    onRemove = async () => {},
    onReload = async () => {},
    onApplyNow = async () => {},
  } = $props()

  let labelName = $state('')
  let fromContains = $state('')
  let subjectContains = $state('')
  let bodyContains = $state('')
  // ''=問わない, 'true'=添付あり, 'false'=添付なし
  let hasAttachment = $state('')
  let markRead = $state(false)
  let archive = $state(false)
  let star = $state(false)
  let skipNotification = $state(false)
  let formError = $state('')
  let submitting = $state(false)
  let openConfigError = $state('')
  let reloading = $state(false)
  let reloadStatus = $state('')
  let applying = $state(false)
  let applyStatus = $state('')

  function describeFilter(rule) {
    const parts = []
    if (rule.from_contains) parts.push(`差出人に「${rule.from_contains}」`)
    if (rule.subject_contains) parts.push(`件名に「${rule.subject_contains}」`)
    if (rule.body_contains) parts.push(`本文に「${rule.body_contains}」`)
    if (rule.has_attachment === true) parts.push('添付あり')
    if (rule.has_attachment === false) parts.push('添付なし')
    return parts.length > 0 ? parts.join(' かつ ') : '(条件なし)'
  }

  function describeActions(rule) {
    const parts = []
    if (rule.mark_read) parts.push('既読にする')
    if (rule.archive) parts.push('アーカイブ')
    if (rule.star) parts.push('スター')
    if (rule.skip_notification) parts.push('通知しない')
    return parts.join('・')
  }

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
      await onAdd({
        labelName,
        fromContains: fromContains.trim() || undefined,
        subjectContains: subjectContains.trim() || undefined,
        bodyContains: bodyContains.trim() || undefined,
        hasAttachment: hasAttachment === '' ? undefined : hasAttachment === 'true',
        markRead,
        archive,
        star,
        skipNotification,
      })
      fromContains = ''
      subjectContains = ''
      bodyContains = ''
      hasAttachment = ''
      markRead = false
      archive = false
      star = false
      skipNotification = false
    } catch (e) {
      formError = String(e)
    } finally {
      submitting = false
    }
  }

  // From/Subject/Body/添付有無のいずれか1つも指定していない条件は送信できないように
  // する（Rust側の`MessageFilter::is_empty`と同じ判定をフォームでも先に行う）。
  // ラベルか動作のどちらかも必要（Rust側の`LabelRule::has_effect`と同じ判定）。
  let canSubmit = $derived(
    (labelName !== '' || markRead || archive || star || skipNotification) &&
      (fromContains.trim() !== '' ||
        subjectContains.trim() !== '' ||
        bodyContains.trim() !== '' ||
        hasAttachment !== ''),
  )

  async function applyNow() {
    applying = true
    applyStatus = '適用中…'
    try {
      const applied = await onApplyNow()
      applyStatus = `${applied}件のメッセージに適用しました`
    } catch (e) {
      applyStatus = `エラー: ${e}`
    } finally {
      applying = false
    }
  }
</script>

<div class="label-rules-view">
  <p class="desc">
    差出人・件名・本文・添付の有無を条件に、受信時に自動でラベルを付けたり、既読にする・
    アーカイブする・スターを付ける・通知しない、を行います。1つのルール内で指定した条件は
    すべて満たした場合にのみ当たります（Gmailのフィルタと同じ考え方）。「いずれかを満たす」に
    したい場合は、同じ動作のルールを複数作ってください。既読にしたメールは新着通知も出ません。
  </p>

  <form class="add-form" onsubmit={(e) => { e.preventDefault(); handleSubmit() }}>
    <label class="field">
      <span>ラベル</span>
      <select bind:value={labelName}>
        <option value="">付けない</option>
        {#each labels as label (label.id)}
          <option value={label.name}>{label.name}</option>
        {/each}
      </select>
    </label>
    <label class="field">
      <span>差出人</span>
      <input bind:value={fromContains} placeholder="例: invoice@" />
    </label>
    <label class="field">
      <span>件名</span>
      <input bind:value={subjectContains} placeholder="例: 領収書" />
    </label>
    <label class="field">
      <span>本文</span>
      <input bind:value={bodyContains} placeholder="例: critical" />
    </label>
    <label class="field">
      <span>添付</span>
      <select bind:value={hasAttachment}>
        <option value="">問わない</option>
        <option value="true">あり</option>
        <option value="false">なし</option>
      </select>
    </label>
    <div class="field">
      <span>動作</span>
      <div class="actions">
        <label><input type="checkbox" bind:checked={markRead} /> 既読にする</label>
        <label><input type="checkbox" bind:checked={archive} /> アーカイブする</label>
        <label><input type="checkbox" bind:checked={star} /> スターを付ける</label>
        <label><input type="checkbox" bind:checked={skipNotification} /> 通知しない</label>
      </div>
    </div>
    <button type="submit" class="primary-button" disabled={submitting || !canSubmit}>
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
    <p class="empty">受信ルールがまだありません。</p>
  {:else}
    <ul class="rule-list">
      {#each rules as rule, index (index)}
        <li class="rule-row">
          {#if rule.label_name}
            <span class="label-name">{rule.label_name}</span>
          {/if}
          <span class="filter">
            {describeFilter(rule)}
            {#if describeActions(rule)}<span class="rule-actions">→ {describeActions(rule)}</span>{/if}
          </span>
          <button type="button" class="danger" onclick={() => onRemove(index)}>削除</button>
        </li>
      {/each}
    </ul>

    <div class="apply-now">
      <button type="button" class="action-button" onclick={applyNow} disabled={applying}>
        今すぐ適用
      </button>
      <span class="apply-desc">既存の受信済みメールにも遡ってルールを適用します（ラベル・既読・アーカイブ・スター）。</span>
      {#if applyStatus}
        <p class="status">{applyStatus}</p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: 12.5px;
  }
  .actions label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .field .actions input {
    padding: 0;
  }
  .rule-actions {
    margin-left: 6px;
    color: var(--accent);
  }
  .label-rules-view {
    max-width: 720px;
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
  .field input,
  .field select {
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    font: inherit;
    background: var(--surface);
    color: var(--text);
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
  .label-name {
    flex: none;
    font-size: 12px;
    font-weight: 700;
    padding: 2px 8px;
    border-radius: 10px;
    background: var(--surface-muted);
    color: var(--text);
  }
  .filter {
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
  .apply-now {
    margin-top: 0.75rem;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .action-button {
    border: 1px solid var(--border-strong);
    border-radius: 7px;
    padding: 8px 14px;
    font: inherit;
    font-size: 13px;
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }
  .action-button:hover {
    background: var(--surface-muted);
  }
  .action-button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .apply-desc {
    color: var(--text-secondary);
    font-size: 0.85em;
  }
</style>
