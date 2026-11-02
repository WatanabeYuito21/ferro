// サイドバーの幅（右端をドラッグして変える）。折りたたみと同じく、この端末だけの見た目の好みなので
// 設定ファイルではなくlocalStorageに覚える。読めない・壊れている値なら既定の幅に戻すだけ。
export const SIDEBAR_DEFAULT_WIDTH = 220
export const SIDEBAR_MIN_WIDTH = 160
export const SIDEBAR_MAX_WIDTH = 480

const STORAGE_KEY = 'ferro.sidebarWidth'

export function clampSidebarWidth(width) {
  if (!Number.isFinite(width)) return SIDEBAR_DEFAULT_WIDTH
  return Math.round(Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width)))
}

export function loadSidebarWidth() {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    return saved === null ? SIDEBAR_DEFAULT_WIDTH : clampSidebarWidth(Number(saved))
  } catch {
    return SIDEBAR_DEFAULT_WIDTH
  }
}

export function saveSidebarWidth(width) {
  try {
    localStorage.setItem(STORAGE_KEY, String(width))
  } catch {
    // 保存できない環境では次の起動で既定の幅に戻るだけ
  }
}
