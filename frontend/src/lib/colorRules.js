// 特定の文字列を含むメッセージの一覧表示を色分けするルールのマッチ判定。
// 実際のルール定義(パターン/色)はcolor_rules.tomlが正の情報源で、Rust側の
// `get_color_rules`/`add_color_rule`/`remove_color_rule`がそれを読み書きするだけ
// （App.svelte参照）。マッチ判定自体はここ(フロント)で行う。仮想スクロールで
// 画面に見えている行だけを判定すればよいため、メール件数に関わらず軽い。

// 件名・差出人（表示名・アドレス）・本文プレビューのいずれかに、ルールの
// パターンが(大文字小文字を区別せず)部分一致で含まれていればマッチとみなす。
// 複数のルールが該当する場合は、先頭から見て最初にマッチしたものの色を使う
// （color_rules.tomlに書かれた順序 = 優先順位）。
export function matchColor(rules, message) {
  if (!rules || rules.length === 0 || !message) return null

  const haystack = [message.subject, message.from_name, message.from_addr, message.preview]
    .filter(Boolean)
    .join(' ')
    .toLowerCase()
  if (!haystack) return null

  for (const rule of rules) {
    if (rule.pattern && haystack.includes(rule.pattern.toLowerCase())) {
      return rule.color
    }
  }
  return null
}
