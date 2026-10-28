<script>
  // キーボードショートカットの一覧（`?`で開閉）。キーの割り当て自体はApp.svelteの
  // `handleKeydown`にあるので、変えるときは両方を揃えること。
  let { onClose = () => {} } = $props()

  const SHORTCUTS = [
    {
      group: '移動',
      items: [
        ['j / ↓', '次のメッセージ'],
        ['k / ↑', '前のメッセージ'],
        ['Enter / o', '先頭のメッセージを開く'],
        ['Esc', '本文を閉じる / 検索をやめる'],
        ['/', '検索欄へ移動'],
        ['g → i', '受信箱へ'],
        ['g → s', 'スター付きへ'],
        ['g → z', 'スヌーズへ'],
        ['g → a', 'アーカイブへ'],
        ['g → t', 'ゴミ箱へ'],
      ],
    },
    {
      group: '開いているメッセージの操作',
      items: [
        ['e', 'アーカイブ / アーカイブ解除'],
        ['s', 'スター / スター解除'],
        ['u', '既読 / 未読の切り替え'],
        ['# / Delete', '削除'],
        ['b', 'スヌーズのメニュー'],
        ['l', 'ラベルのメニュー'],
        ['v', '本文 / ヘッダー / ソースの切り替え'],
      ],
    },
    {
      group: '複数選択',
      items: [
        ['x', '開いているメッセージを選択 / 解除'],
        ['Ctrl+クリック', '1件ずつ選択 / 解除'],
        ['Shift+クリック', '範囲を選択'],
        ['e / s / u / #', '選択中のすべてに一括で行う'],
        ['Esc', '選択を解除'],
      ],
    },
    {
      group: 'その他',
      items: [
        ['[', 'サイドバーを折りたたむ / 開く'],
        ['?', 'この一覧を開く / 閉じる'],
      ],
    },
  ]
</script>

<div class="backdrop" role="presentation" onclick={onClose}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="キーボードショートカット"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
    onkeydown={() => {}}
  >
    <div class="header">
      <h2>キーボードショートカット</h2>
      <button type="button" class="close" onclick={onClose}>閉じる</button>
    </div>
    <div class="groups">
      {#each SHORTCUTS as section (section.group)}
        <section>
          <h3>{section.group}</h3>
          <dl>
            {#each section.items as [keys, description] (keys)}
              <dt><kbd>{keys}</kbd></dt>
              <dd>{description}</dd>
            {/each}
          </dl>
        </section>
      {/each}
    </div>
    <p class="note">検索欄などの入力中はショートカットは効きません（Escで入力欄から抜けます）。</p>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    background: rgba(28, 27, 24, 0.35);
  }
  .dialog {
    width: min(640px, 100%);
    max-height: 100%;
    overflow-y: auto;
    box-sizing: border-box;
    padding: 20px 24px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 18px 40px -16px rgba(28, 27, 24, 0.45);
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  .close {
    border: none;
    background: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 8px 24px;
    margin-top: 12px;
  }
  h3 {
    margin: 8px 0 6px;
    font-size: 12px;
    color: var(--text-muted);
    font-weight: 600;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 12px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    white-space: nowrap;
  }
  dd {
    margin: 0;
    color: var(--text-secondary);
  }
  kbd {
    display: inline-block;
    padding: 1px 6px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    background: var(--surface-subtle);
    font-family: inherit;
    font-size: 12px;
  }
  .note {
    margin: 16px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
</style>
