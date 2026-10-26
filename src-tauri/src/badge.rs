//! トレイアイコン・タスクバーに出す未読数の小さなバッジを、RGBAの画素として描く。
//!
//! 画像処理のクレートを足すほどではないので、3×5ドットの数字を拡大して赤い角丸の
//! 札の上に白で描くだけの素朴な実装にしている（アンチエイリアスは無い）。

/// 3×5ドットの字形（上の行から、各行の下位3ビットが左→右）。
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
];

const BADGE_COLOR: [u8; 4] = [0xD9, 0x30, 0x25, 0xFF];
const TEXT_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// バッジに書く文字（100件以上は「99+」）。
pub fn label(count: u64) -> String {
    if count > 99 { "99+".to_string() } else { count.to_string() }
}

fn glyph(c: char) -> &'static [u8; 5] {
    &GLYPHS.iter().find(|(g, _)| *g == c).expect("badge labels only use digits and '+'").1
}

struct Canvas<'a> {
    rgba: &'a mut [u8],
    width: u32,
    height: u32,
}

impl Canvas<'_> {
    fn put(&mut self, x: i64, y: i64, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.rgba[i..i + 4].copy_from_slice(&color);
    }

    /// 角を丸めた矩形（角の半径は高さの半分＝両端が半円の札）。
    fn pill(&mut self, x: i64, y: i64, w: i64, h: i64, color: [u8; 4]) {
        let r = h as f64 / 2.0;
        for py in 0..h {
            for px in 0..w {
                // 両端の半円の外側は塗らない。
                let cx = if (px as f64) < r { r } else if (px as f64) >= w as f64 - r { w as f64 - r } else { px as f64 + 0.5 };
                let (dx, dy) = (px as f64 + 0.5 - cx, py as f64 + 0.5 - r);
                if dx * dx + dy * dy <= r * r {
                    self.put(x + px, y + py, color);
                }
            }
        }
    }

    fn text(&mut self, x: i64, y: i64, text: &str, scale: i64, color: [u8; 4]) {
        for (n, c) in text.chars().enumerate() {
            let left = x + n as i64 * 4 * scale;
            for (row, bits) in glyph(c).iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                self.put(left + col * scale + sx, y + row as i64 * scale + sy, color);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 文字の幅（ドット数。字間1ドット込み）。
fn text_width(text: &str, scale: i64) -> i64 {
    (text.chars().count() as i64 * 4 - 1) * scale
}

/// `rgba`（`width`×`height`）の右下に`count`のバッジを重ねる。高さはアイコンの半分ほど。
pub fn draw_on(rgba: &mut [u8], width: u32, height: u32, count: u64) {
    let text = label(count);
    let scale = (height as i64 / 14).max(1);
    let pad = scale;
    let h = 5 * scale + 2 * pad;
    let w = (text_width(&text, scale) + 2 * pad).max(h).min(width as i64);
    let (x, y) = (width as i64 - w, height as i64 - h);
    let mut canvas = Canvas { rgba, width, height };
    canvas.pill(x, y, w, h, BADGE_COLOR);
    canvas.text(x + (w - text_width(&text, scale)) / 2, y + pad, &text, scale, TEXT_COLOR);
}

/// バッジだけの正方形の画像（Windowsのタスクバーのオーバーレイ用。OSが16×16に縮める）。
pub fn standalone(count: u64, size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let text = label(count);
    let scale = (size as i64 / (text_width(&text, 1) + 2)).max(1);
    let h = 5 * scale + 2 * scale;
    let mut canvas = Canvas { rgba: &mut rgba, width: size, height: size };
    // 文字数に関わらず正方形いっぱいの丸にする（タスクバーのオーバーレイは小さいため）。
    canvas.pill(0, 0, size as i64, size as i64, BADGE_COLOR);
    let top = (size as i64 - h) / 2 + scale;
    canvas.text((size as i64 - text_width(&text, scale)) / 2, top, &text, scale, TEXT_COLOR);
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn labels_cap_at_99_plus() {
        assert_eq!(label(7), "7");
        assert_eq!(label(99), "99");
        assert_eq!(label(100), "99+");
    }

    #[test]
    fn draws_the_badge_in_the_bottom_right_and_leaves_the_rest() {
        let (w, h) = (32, 32);
        let mut rgba = vec![0x11u8; (w * h * 4) as usize];
        draw_on(&mut rgba, w, h, 5);
        assert_eq!(pixel(&rgba, w, 0, 0), [0x11; 4]);
        // 札の中央付近は文字（白）か札（赤）のどちらか。
        let center = pixel(&rgba, w, w - 6, h - 5);
        assert!(center == BADGE_COLOR || center == TEXT_COLOR, "{center:?}");
        assert!((0..w).any(|x| (0..h).any(|y| pixel(&rgba, w, x, y) == TEXT_COLOR)));
    }

    #[test]
    fn standalone_badge_fits_three_characters() {
        let size = 32;
        let rgba = standalone(1234, size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        assert_eq!(pixel(&rgba, size, 0, 0)[3], 0, "corners stay transparent");
        let white = (0..size).flat_map(|x| (0..size).map(move |y| (x, y))).filter(|&(x, y)| pixel(&rgba, size, x, y) == TEXT_COLOR);
        let xs: Vec<u32> = white.map(|(x, _)| x).collect();
        assert!(xs.iter().min().unwrap() > &0 && xs.iter().max().unwrap() < &(size - 1));
    }
}
