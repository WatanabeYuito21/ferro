// テーマ/アクセントカラー/フォント/文字サイズ設定を実際の見た目に反映する。
// 設定の永続化はDB(ferro_core::db::settings::Settings)側だが、適用そのもの
// （<html>のdata属性とCSSカスタムプロパティの書き換え）はここでまとめて行う。
// App.svelte(起動時・設定画面から戻った時)とSettingsView.svelte(設定変更の
// 都度、即座にプレビューさせるため)の両方から呼ぶ。

// フォントは固定候補ではなく、OSにインストール済みのフォント一覧
// (`list_installed_fonts`コマンド、SettingsView.svelte参照)から選ぶ。
// 指定されたフォントが読み込めない/存在しない場合に備え、常にこの
// フォールバックチェーンを末尾に付ける。
const DEFAULT_FONT_FAMILY = 'Noto Sans JP'
const FONT_FALLBACK_CHAIN = `'Noto Sans JP', system-ui, 'Segoe UI', Roboto, sans-serif`

function quoteFontName(name) {
  return `'${String(name).replace(/'/g, "\\'")}'`
}

export const THEME_OPTIONS = [
  { value: 'light', label: 'ライト' },
  { value: 'dark', label: 'ダーク' },
  { value: 'system', label: 'システムに合わせる' },
]

export const FONT_SIZE_OPTIONS = [
  { value: 'small', label: '小' },
  { value: 'medium', label: '中' },
  { value: 'large', label: '大' },
]

// アクセントカラーのプリセット（自由入力の色ではなく、既存のラベル色プリセットと
// 同じ考え方でいくつかの色系統から選ぶ方式。任意の16進値だと--surfaceとの
// コントラストが悪くなる組み合わせを選ばれてしまうリスクがあるため）。
export const ACCENT_COLOR_OPTIONS = [
  { value: '#3f6b5c', label: '深緑（既定）' },
  { value: '#2f5d8a', label: '青' },
  { value: '#6a4c93', label: '紫' },
  { value: '#a13d3d', label: '赤' },
  { value: '#b5722a', label: 'オレンジ' },
]

export function applyAppearance(settings) {
  if (!settings || typeof document === 'undefined') return
  const root = document.documentElement

  if (settings.theme === 'light' || settings.theme === 'dark') {
    root.dataset.theme = settings.theme
  } else {
    delete root.dataset.theme
  }

  root.style.setProperty('--accent', settings.accent_color || ACCENT_COLOR_OPTIONS[0].value)
  const fontName = settings.font_family || DEFAULT_FONT_FAMILY
  root.style.setProperty('--font-sans', `${quoteFontName(fontName)}, ${FONT_FALLBACK_CHAIN}`)
  root.dataset.fontSize = settings.font_size || 'medium'
}
