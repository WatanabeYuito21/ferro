<script>
  // 詳細検索のフォーム。入力から検索欄の文字列（`from:alice has:attachment`のような書式。
  // 解釈はRust側の`ferro_core::search::query`）を組み立てて`onSearch`に渡すだけで、
  // 検索自体は通常の検索と同じ経路で行う。書式を覚えていなくても使えるようにするためのもの。
  let { labels = [], onSearch = () => {}, onClose = () => {} } = $props()

  let from = $state('')
  let subject = $state('')
  let body = $state('')
  let words = $state('')
  let excluded = $state('')
  let afterDate = $state('')
  let untilDate = $state('')
  let hasAttachment = $state(false)
  let unreadOnly = $state(false)
  let starredOnly = $state(false)
  let label = $state('')
  let folder = $state('')

  const FOLDERS = [
    ['', 'すべて'],
    ['inbox', '受信箱'],
    ['starred', 'スター付き'],
    ['snoozed', 'スヌーズ'],
    ['archive', 'アーカイブ'],
    ['trash', 'ゴミ箱'],
  ]

  // 空白や`"`を含む値は`"`で囲む（中の`"`は検索語として意味を持たないので取り除く）。
  function quote(value) {
    const v = value.trim().replaceAll('"', '')
    return /\s/.test(v) ? `"${v}"` : v
  }

  // 「〜まで」はその日を含めたいので、`before:`には翌日を渡す。
  function nextDay(date) {
    const [y, m, d] = date.split('-').map(Number)
    const next = new Date(Date.UTC(y, m - 1, d + 1))
    return next.toISOString().slice(0, 10)
  }

  function buildQuery() {
    const parts = []
    if (words.trim()) parts.push(...words.trim().split(/\s+/))
    if (from.trim()) parts.push(`from:${quote(from)}`)
    if (subject.trim()) parts.push(`subject:${quote(subject)}`)
    if (body.trim()) parts.push(`body:${quote(body)}`)
    for (const word of excluded.trim().split(/\s+/).filter(Boolean)) parts.push(`-${word}`)
    if (afterDate) parts.push(`after:${afterDate}`)
    if (untilDate) parts.push(`before:${nextDay(untilDate)}`)
    if (hasAttachment) parts.push('has:attachment')
    if (unreadOnly) parts.push('is:unread')
    if (starredOnly) parts.push('is:starred')
    if (label) parts.push(`label:${quote(label)}`)
    if (folder) parts.push(`in:${folder}`)
    return parts.join(' ')
  }

  const query = $derived(buildQuery())

  function submit(event) {
    event.preventDefault()
    if (!query) return
    onSearch(query)
  }

  function reset() {
    from = subject = body = words = excluded = afterDate = untilDate = label = folder = ''
    hasAttachment = unreadOnly = starredOnly = false
  }
</script>

<form class="advanced" onsubmit={submit}>
  <div class="grid">
    <label>
      <span>差出人</span>
      <input bind:value={from} placeholder="名前またはアドレスの一部" />
    </label>
    <label>
      <span>件名</span>
      <input bind:value={subject} />
    </label>
    <label>
      <span>本文</span>
      <input bind:value={body} />
    </label>
    <label>
      <span>含む語</span>
      <input bind:value={words} placeholder="件名・差出人・本文のどこか" />
    </label>
    <label>
      <span>含まない語</span>
      <input bind:value={excluded} />
    </label>
    <div class="dates">
      <span>受信日</span>
      <input type="date" bind:value={afterDate} aria-label="この日から" />
      <span class="sep">〜</span>
      <input type="date" bind:value={untilDate} aria-label="この日まで" />
    </div>
    <label>
      <span>ラベル</span>
      <select bind:value={label}>
        <option value="">指定しない</option>
        {#each labels as l (l.id)}
          <option value={l.name}>{l.name}</option>
        {/each}
      </select>
    </label>
    <label>
      <span>フォルダ</span>
      <select bind:value={folder}>
        {#each FOLDERS as [value, name] (value)}
          <option {value}>{name}</option>
        {/each}
      </select>
    </label>
  </div>
  <div class="checks">
    <label><input type="checkbox" bind:checked={hasAttachment} /> 添付あり</label>
    <label><input type="checkbox" bind:checked={unreadOnly} /> 未読のみ</label>
    <label><input type="checkbox" bind:checked={starredOnly} /> スター付きのみ</label>
  </div>
  <div class="footer">
    <code class="preview" title="検索欄に直接この書式で入力することもできます">{query || '（条件を入力してください）'}</code>
    <button type="button" class="plain" onclick={reset}>クリア</button>
    <button type="button" class="plain" onclick={onClose}>閉じる</button>
    <button type="submit" class="primary" disabled={!query}>検索</button>
  </div>
  <details class="syntax">
    <summary>検索欄で使える書式</summary>
    <ul>
      <li><code>from:</code> <code>subject:</code> <code>body:</code>（<code>差出人:</code> <code>件名:</code> <code>本文:</code>）… そのフィールドに含む</li>
      <li><code>has:attachment</code> … 添付あり ／ <code>is:unread</code> <code>is:read</code> <code>is:starred</code></li>
      <li><code>in:inbox</code> <code>in:archive</code> <code>in:trash</code> など ／ <code>label:ラベル名</code></li>
      <li><code>after:2026-01-01</code>（その日を含む） <code>before:2026-02-01</code>（その日を含まない）</li>
      <li>先頭に <code>-</code> で否定（<code>-from:noreply</code>）、空白を含む値は <code>"</code> で囲む</li>
    </ul>
  </details>
</form>

<style>
  .advanced {
    margin-bottom: 0.75rem;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-subtle);
    font-size: 12.5px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
    gap: 8px 16px;
  }
  label,
  .dates {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  label > span:first-child,
  .dates > span:first-child {
    flex: 0 0 72px;
    color: var(--text-muted);
  }
  input:not([type='checkbox']),
  select {
    flex: 1;
    min-width: 0;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font: inherit;
  }
  .dates input {
    flex: 1 1 0;
  }
  .sep {
    color: var(--text-muted);
  }
  .checks {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    margin-top: 10px;
  }
  .checks label {
    gap: 4px;
  }
  .checks input {
    accent-color: var(--accent);
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .preview {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: 12px;
  }
  button {
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 5px 12px;
    background: var(--surface);
    color: var(--text-secondary);
    font: inherit;
    cursor: pointer;
  }
  button.plain {
    border-color: transparent;
    background: none;
    color: var(--text-muted);
  }
  button.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: #fff;
  }
  button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .syntax {
    margin-top: 8px;
    color: var(--text-muted);
  }
  .syntax summary {
    cursor: pointer;
    font-size: 12px;
  }
  .syntax ul {
    margin: 6px 0 0;
    padding-left: 18px;
    line-height: 1.8;
  }
  code {
    font-size: 11.5px;
  }
</style>
