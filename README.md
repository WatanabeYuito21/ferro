# Ferro

1000万件規模のメールでも軽快に動く、POP3受信専用のメーラーです。Rust + Tauri + Svelteで書かれており、CLIとGUIの両方から使えます。

## なぜ作ったか

既存のメーラー（neomutt、Thunderbirdなど）は、メール件数が数百万件規模になると起動や検索が重くなりがちです。Ferroはアーキテクチャレベルでこれを避けることを目標にしています。

- 起動時に全件スキャンしない（DB/検索インデックスを開くだけ）
- 検索は事前に構築したインデックス（[Tantivy](https://github.com/quickwit-oss/tantivy)）を引くだけで、grep的な全文走査はしない
- メッセージ一覧は自前実装の仮想スクロール（固定行高ウィンドウイング + 無限スクロール）で、1000万件を素朴に描画しない
- Electron/ChromiumではなくTauriを採用し、配布物を軽量に保つ

送信機能はありません（受信専用ツールです）。

## 主な機能

- POP3受信（USER/PASS/STAT/LIST/UIDL/RETR/DELE/QUIT/STLS、暗黙的TLS、RETRのパイプライン化、
  接続が切れた場合の再接続・再開、同期件数の進捗表示）
- SQLiteによるメタデータ管理とキーセットページネーションの一覧表示
- Maildir形式での生メール保存
- Tantivyによる全文検索（自前のバイグラムトークナイザにより、日本語・英数字問わず部分一致で
  検索できる。索引はバックグラウンドで自動的に差分を追いつかせるため、検索インデックスの状態を
  意識する必要はない。検索結果は常に
  受信日時の新しい順で表示される）
- 既読/フラグ/削除といったメッセージ状態の管理
- GUIではフォルダ（受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱）と色付きラベルによる整理、
  設定画面（表示・同期間隔・検索インデックス再構築・外観）
- 外観設定（テーマ: ライト/ダーク/システム追従、アクセントカラー、フォント、文字サイズ）
- 件名/差出人/本文プレビューに特定の文字列を含むメッセージを一覧上で色分けする色分けルール
  （設定画面から追加/削除でき、`color_rules.toml`を直接編集して反映することもできる）
- アカウント設定は`accounts.toml`で管理し、直接編集して反映できる（パスワードはOSの資格情報マネージャーに保存）。
  GUIの設定画面からファイルを直接開くこともできる
- GUIでは自動バックグラウンド同期（間隔は設定画面から変更可能。デフォルト5分）
- 添付ファイルの一覧表示・保存
- CLI/GUIどちらからでも同じデータ（DB・Maildir・検索インデックス）を扱える

## 構成

Cargoワークスペースになっています。

| クレート | 役割 |
| --- | --- |
| `ferro-core` | POP3クライアント・DB・Maildir・検索・アカウント管理などのコアロジック（ライブラリ） |
| `ferro-cli` | コマンドラインインターフェース（バイナリ名: `ferro`） |
| `src-tauri` | Tauriデスクトップアプリのバックエンド |
| `frontend` | Svelte（Vite）製のGUIフロントエンド |

CLIとGUIは同じDB/Maildir/検索インデックス/アカウント設定ファイルを共有します（保存場所は後述）。

## 必要なもの

- Rust（2024 edition、[rustup](https://rustup.rs/)推奨）
- Node.js / npm（GUIのフロントエンドビルド用）
- Windowsでインストーラーをビルドする場合: `cargo install tauri-cli --version "^2"`
  （WiX/NSIS自体はtauri-bundlerが自動取得するので別途インストール不要）

## 使い方（CLI）

```sh
# ビルド
cargo build --release

# アカウントを追加（パスワードは対話プロンプトで入力し、OSの資格情報マネージャーに保存される）
ferro account add --name Work --host pop.example.com --port 995 --username alice --use-tls

# アカウント一覧
ferro account list

# 同期（新着メールを取得。--limitで1回に取得する件数の上限を指定できる）
ferro sync <account_id> --limit 100

# メッセージ一覧・検索
ferro list
ferro search "検索語"

# 既読/フラグ/削除
ferro read <message_id>
ferro flag <message_id>
ferro delete <message_id>

# 全メッセージから検索インデックスを作り直す
ferro reindex
```

`ferro --help` / `ferro <subcommand> --help` で全コマンドとオプションを確認できます。

## 使い方（GUI）

```sh
cd frontend
npm install

# 開発モード（別ターミナルでvite devサーバーが自動起動する）
cd ..
cargo run -p ferro-desktop

# Windowsインストーラー（MSI/NSIS）をビルドする場合
cargo tauri build
```

## アカウント設定ファイル

アカウント設定（パスワードを除く）はアプリデータディレクトリ直下の`accounts.toml`で管理します（正の情報源）。直接編集して、CLI/GUIの起動時か `ferro account reload-config` / GUIの「Reload config」ボタンで反映できます。

```toml
[[account]]
name = "Work"
host = "pop.example.com"
port = 995
username = "alice"
use_tls = true
```

ファイルから削除したエントリのメッセージは自動では消えません（削除は`ferro account remove`で明示的に行います）。パスワードはこのファイルには含まれず、OSの資格情報マネージャー（Windows資格情報マネージャー / secret-service等）に保存されます。

保存場所は環境ごとに以下のようになります。

- Windows: `%APPDATA%\ferro\`
- Linux: `~/.local/share/ferro/`
- macOS: `~/Library/Application Support/ferro/`

## 開発

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets

# 1000万件規模の性能検証（本番データには触れない専用の一時ディレクトリを使う）
ferro bench 10000000
```

開発の背景・設計判断・既知の問題などは[CLAUDE.md](./CLAUDE.md)により詳しくまとまっています。

## 既知の制限

- Tantivyの`IndexWriter`がWindows環境で断続的に落ちることがある未解決の問題があります（詳細は[CLAUDE.md](./CLAUDE.md)参照）。自動リトライで大半は吸収されますが、完全な解決には至っていません。
- 検索の日本語対応は形態素解析ではなくCJKバイグラム（機械的な2文字ずつの部分一致）によるものです。
- SMTP送信機能はありません（受信専用）。
