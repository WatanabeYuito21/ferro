// テーマ/アクセントカラー/フォント/文字サイズ設定を実際の見た目に反映する。
// 設定の永続化はDB(ferro_core::db::settings::Settings)側だが、適用そのもの
// （<html>のdata属性とCSSカスタムプロパティの書き換え）はここでまとめて行う。
// App.svelte(起動時・設定画面から戻った時)とSettingsView.svelte(設定変更の
// 都度、即座にプレビューさせるため)の両方から呼ぶ。

// 游ゴシック/游明朝/メイリオ/BIZ UDゴシックはWindows同梱フォントで
// Web Fontとしては読み込んでいない（開発対象がWindowsのため）。他OSでは
// 太字のフォールバック(system-ui等)に自動で落ちるだけで、エラーにはならない。
// Noto Sans/Serif JP・M PLUS 1pはGoogle Fontsから読み込む（index.html参照）。
const FONT_STACKS = {
  'noto-sans': "'Noto Sans JP', system-ui, 'Segoe UI', Roboto, sans-serif",
  'noto-serif': "'Noto Serif JP', 'Yu Mincho', serif",
  'yu-gothic': "'Yu Gothic', 'YuGothic', system-ui, 'Segoe UI', sans-serif",
  'yu-mincho': "'Yu Mincho', 'YuMincho', serif",
  meiryo: "Meiryo, 'MS PGothic', system-ui, sans-serif",
  'biz-ud-gothic': "'BIZ UDGothic', 'BIZ UDPGothic', Meiryo, system-ui, sans-serif",
  'm-plus-1p': "'M PLUS 1p', 'Noto Sans JP', system-ui, sans-serif",
  monospace: "'Cascadia Code', Consolas, 'MS Gothic', monospace",
}

export const FONT_FAMILY_OPTIONS = [
  { value: 'noto-sans', label: 'Noto Sans JP（既定）' },
  { value: 'noto-serif', label: 'Noto Serif JP（明朝系）' },
  { value: 'yu-gothic', label: '游ゴシック' },
  { value: 'yu-mincho', label: '游明朝' },
  { value: 'meiryo', label: 'メイリオ' },
  { value: 'biz-ud-gothic', label: 'BIZ UDゴシック' },
  { value: 'm-plus-1p', label: 'M PLUS 1p' },
  { value: 'monospace', label: '等幅' },
]

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
  root.style.setProperty('--font-sans', FONT_STACKS[settings.font_family] || FONT_STACKS['noto-sans'])
  root.dataset.fontSize = settings.font_size || 'medium'
}
