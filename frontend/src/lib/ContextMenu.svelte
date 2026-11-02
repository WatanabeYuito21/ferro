<script>
  // 右クリック（またはShift+F10/アプリケーションキー）で出すメニュー。App.svelteに1つだけ置き、
  // 何を出すかは`contextMenus.js`が組み立てた`items`で渡す。
  //
  // 項目: { label, hint?, disabled?, danger?, checked?, onSelect?, submenu? } または
  // { separator: true } / { heading: '…' }（選べない見出し）。`checked`がtrue/falseの項目は
  // チェックの付け外しとして出す（undefinedなら普通の項目）。`submenu`は1段だけ。
  //
  // 開いている間はキー入力をここで止める（一覧のj/kやe等のショートカットが裏で効かないように）。
  import { onMount, tick } from 'svelte'

  let { x = 0, y = 0, items = [], onClose = () => {} } = $props()

  const MARGIN = 4

  let menuEl = $state(null)
  let subEl = $state(null)
  // 位置はonMountで大きさを測ってから決める（描画前に済むので左上に一瞬出ることはない）。
  // 別の場所で開き直す時は、App.svelteが`{#key}`で作り直す。
  let left = $state(0)
  let top = $state(0)
  // キーボードで選んでいる項目（`items`の添字）と、開いているサブメニュー。
  let active = $state(-1)
  let openSub = $state(-1)
  let subActive = $state(-1)
  let subLeft = $state(0)
  let subTop = $state(0)
  // 開く前にフォーカスがあった要素（閉じたら戻す。キーボードで開いた時に行へ戻れるように）。
  let previousFocus = null

  const selectable = (item) => item && !item.separator && !item.heading && !item.disabled

  onMount(() => {
    previousFocus = document.activeElement
    // 画面の外にはみ出すなら内側へ寄せる（右端・下端で開いた時）。
    const rect = menuEl.getBoundingClientRect()
    left = Math.max(MARGIN, Math.min(x, window.innerWidth - rect.width - MARGIN))
    top = Math.max(MARGIN, Math.min(y, window.innerHeight - rect.height - MARGIN))
    menuEl.focus()

    const onPointerDown = (event) => {
      if (!inside(event.target)) close()
    }
    // メニューの外のスクロール（一覧を動かした等）で位置がずれるので閉じる。
    const onScroll = (event) => {
      if (!inside(event.target)) close()
    }
    // メニューの上でもう一度右クリックした時にブラウザ標準のメニューを出さない。
    const onContextMenu = (event) => {
      if (inside(event.target)) event.preventDefault()
    }
    window.addEventListener('pointerdown', onPointerDown, true)
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('contextmenu', onContextMenu, true)
    window.addEventListener('keydown', onKeydown, true)
    window.addEventListener('resize', close)
    window.addEventListener('blur', close)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true)
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('contextmenu', onContextMenu, true)
      window.removeEventListener('keydown', onKeydown, true)
      window.removeEventListener('resize', close)
      window.removeEventListener('blur', close)
    }
  })

  // サブメニューは別の要素として出すので、両方を見る。
  function inside(target) {
    return Boolean(menuEl?.contains(target) || subEl?.contains(target))
  }

  function close() {
    onClose()
    if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus()
  }

  function choose(item) {
    if (!selectable(item)) return
    if (item.submenu) return
    close()
    item.onSelect?.()
  }

  async function showSub(index, focusFirst = false) {
    if (openSub === index) {
      if (focusFirst) subActive = nextIndex(items[index].submenu, -1, 1)
      return
    }
    openSub = index
    subActive = focusFirst ? nextIndex(items[index].submenu, -1, 1) : -1
    await tick()
    const row = menuEl?.querySelector(`[data-index="${index}"]`)
    if (!row || !subEl) return
    const rowRect = row.getBoundingClientRect()
    const subRect = subEl.getBoundingClientRect()
    // 右に収まらなければ左側に出す。下にはみ出すなら上へずらす。
    const rightSide = rowRect.right - 2
    subLeft =
      rightSide + subRect.width + MARGIN <= window.innerWidth
        ? rightSide
        : Math.max(MARGIN, rowRect.left - subRect.width + 2)
    subTop = Math.max(MARGIN, Math.min(rowRect.top - 5, window.innerHeight - subRect.height - MARGIN))
  }

  function hideSub() {
    openSub = -1
    subActive = -1
  }

  // `list`の中で`from`から`step`方向にある、次に選べる項目の添字（端で折り返す）。
  function nextIndex(list, from, step) {
    const n = list.length
    for (let i = 1; i <= n; i++) {
      const index = (((from + step * i) % n) + n) % n
      if (selectable(list[index])) return index
    }
    return -1
  }

  function onKeydown(event) {
    // 開いている間は、ここで扱わないキーもアプリのショートカットへ渡さない。
    event.stopPropagation()
    const inSub = openSub !== -1 && subActive !== -1
    const list = inSub ? items[openSub].submenu : items
    const current = inSub ? subActive : active
    const setCurrent = (index) => (inSub ? (subActive = index) : (active = index))

    switch (event.key) {
      case 'ArrowDown':
        setCurrent(nextIndex(list, current, 1))
        break
      case 'ArrowUp':
        setCurrent(nextIndex(list, current === -1 ? 0 : current, -1))
        break
      case 'Home':
        setCurrent(nextIndex(list, -1, 1))
        break
      case 'End':
        setCurrent(nextIndex(list, 0, -1))
        break
      case 'ArrowRight':
        if (!inSub && items[active]?.submenu) showSub(active, true)
        break
      case 'ArrowLeft':
        if (inSub || openSub !== -1) hideSub()
        break
      case 'Enter':
      case ' ': {
        const item = list[current]
        if (!inSub && item?.submenu) showSub(current, true)
        else if (item) choose(item)
        break
      }
      case 'Escape':
        if (openSub !== -1) hideSub()
        else close()
        break
      case 'Tab':
        close()
        break
      default:
        return
    }
    event.preventDefault()
  }
</script>

{#snippet entry(item, index, isActive, onEnter)}
  {#if item.separator}
    <div class="separator" role="separator"></div>
  {:else if item.heading}
    <div class="heading">{item.heading}</div>
  {:else}
    <button
      type="button"
      class="item"
      class:active={isActive}
      class:danger={item.danger}
      class:open={item.submenu && openSub === index}
      role={item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
      aria-checked={item.checked === undefined ? undefined : item.checked}
      aria-haspopup={item.submenu ? 'menu' : undefined}
      aria-disabled={item.disabled ? 'true' : undefined}
      disabled={item.disabled}
      tabindex="-1"
      data-index={index}
      onmouseenter={onEnter}
      onclick={() => (item.submenu ? showSub(index, false) : choose(item))}
    >
      <span class="check" aria-hidden="true">{item.checked ? '✓' : ''}</span>
      {#if item.color}<span class="dot" style="background: {item.color};"></span>{/if}
      <span class="label">{item.label}</span>
      {#if item.hint}<span class="hint">{item.hint}</span>{/if}
      {#if item.submenu}<span class="arrow" aria-hidden="true">›</span>{/if}
    </button>
  {/if}
{/snippet}

<div
  class="menu"
  role="menu"
  tabindex="-1"
  bind:this={menuEl}
  style="left: {left}px; top: {top}px;"
>
  {#each items as item, index (index)}
    {@render entry(item, index, active === index, () => {
      active = selectable(item) ? index : -1
      if (item.submenu && !item.disabled) showSub(index)
      else hideSub()
    })}
  {/each}
</div>

{#if openSub !== -1}
  <div
    class="menu submenu"
    role="menu"
    tabindex="-1"
    bind:this={subEl}
    style="left: {subLeft}px; top: {subTop}px;"
  >
    {#each items[openSub].submenu as item, index (index)}
      {@render entry(item, index, subActive === index, () => (subActive = selectable(item) ? index : -1))}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 1000;
    display: flex;
    flex-direction: column;
    min-width: 200px;
    max-width: 320px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 10px 28px -10px rgba(28, 27, 24, 0.45);
    font-size: 12.5px;
    outline: none;
    user-select: none;
  }
  .submenu {
    max-height: calc(100vh - 8px);
    overflow-y: auto;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px 6px 4px;
    border: none;
    border-radius: 5px;
    background: none;
    color: var(--text-secondary);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .item.active,
  .item.open {
    background: var(--surface-muted);
  }
  .item:disabled {
    color: var(--text-faint);
    cursor: default;
  }
  .item.danger {
    color: var(--danger);
  }
  .check {
    flex: none;
    width: 14px;
    text-align: center;
    color: var(--accent);
  }
  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 3px;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    flex: none;
    margin-left: 12px;
    color: var(--text-faint);
    font-size: 11px;
  }
  .arrow {
    flex: none;
    margin-left: 8px;
    color: var(--text-faint);
  }
  .separator {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
  .heading {
    padding: 4px 10px 4px 24px;
    color: var(--text-muted);
    font-size: 11.5px;
    font-weight: 600;
  }
</style>
