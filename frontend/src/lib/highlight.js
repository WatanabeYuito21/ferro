// 検索語に一致した箇所の強調（検索結果の一覧と本文表示）。強調する語は
// `highlight_terms`コマンド（Rust側の`search::highlight::terms`）から受け取り、
// 一致の規則は同じ`search::highlight::ranges`に揃える: 大文字小文字を無視した
// 部分一致で、同じ位置から複数の語が一致するときは長い方。

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

// `field`（'from' | 'subject' | 'body'）の欄に当てる語。キー無しの語（'any'）はどの欄にも当てる。
export function needlesFor(terms, field) {
  return terms.filter((t) => t.field === 'any' || t.field === field).map((t) => t.text)
}

// `text`を { text, match } の並びに分ける（matchなら強調する）。
export function highlightSegments(text, needles) {
  const source = text ?? ''
  const unique = [...new Set(needles.filter((n) => n))].sort((a, b) => b.length - a.length)
  if (!source || unique.length === 0) return [{ text: source, match: false }]
  // 正規表現の選択肢は左から試されるので、長い語を先に並べれば長い方が勝つ。
  const pattern = new RegExp(unique.map(escapeRegExp).join('|'), 'giu')
  const segments = []
  let last = 0
  for (const m of source.matchAll(pattern)) {
    if (m.index > last) segments.push({ text: source.slice(last, m.index), match: false })
    segments.push({ text: m[0], match: true })
    last = m.index + m[0].length
  }
  if (last < source.length) segments.push({ text: source.slice(last), match: false })
  return segments
}
