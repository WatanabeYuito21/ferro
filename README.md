# Ferro

[![Release](https://github.com/WatanabeYuito21/ferro/actions/workflows/release.yml/badge.svg)](https://github.com/WatanabeYuito21/ferro/actions/workflows/release.yml)
[![Latest Release](https://img.shields.io/github/v/release/WatanabeYuito21/ferro)](https://github.com/WatanabeYuito21/ferro/releases/latest)

1000万件規模のメールでも軽快に動く、POP3受信専用のメーラーです。Rust + Tauri + Svelteで書かれており、CLI・TUI・GUIのいずれからでも使えます。

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
- 外観設定（テーマ: ライト/ダーク/システム追従、アクセントカラー、フォント（PCにインストール
  済みのフォントから選択可能）、文字サイズ）
- 件名/差出人/本文プレビューに特定の文字列を含むメッセージを一覧上で色分けする色分けルール
  （設定画面から追加/削除でき、`color_rules.toml`を直接編集して反映することもできる）
- メールの保持日数を設定でき、期限切れメールを6時間ごとに（または設定画面の「今すぐ整理する」で
  即座に）ローカルから完全に削除する。既定は無期限（自動削除しない）で、スター付きメールは
  保持日数に関わらず削除されない
- アカウント設定は`accounts.toml`で管理し、直接編集して反映できる（パスワードはOSの資格情報マネージャーに保存）。
  GUIの設定画面からファイルを直接開くこともできる
- GUIでは自動バックグラウンド同期（間隔は設定画面から変更可能。デフォルト5分）
- 添付ファイルの一覧表示・保存
- CLI/TUI/GUIどれからでも同じデータ（DB・Maildir・検索インデックス）を扱える
- TUI版（`ferro-tui`）: 3ペイン構成（フォルダ/ラベル｜メッセージ一覧｜本文）のターミナルUI。
  既読/スター/アーカイブ/削除/スヌーズ、検索、全アカウント手動同期、バックグラウンド同期・
  検索キャッチアップ・保持期間クリーンアップに対応。設定画面は現状「表示のみ」で、
  トグル/数値の編集や色分けルール・アカウントの追加フォームは今後対応予定

## 構成

Cargoワークスペースになっています。

| クレート | 役割 |
| --- | --- |
| `ferro-core` | POP3クライアント・DB・Maildir・検索・アカウント管理などのコアロジック（ライブラリ） |
| `ferro-cli` | コマンドラインインターフェース（バイナリ名: `ferro`） |
| `ferro-tui` | ratatui製のターミナルUI（バイナリ名: `ferro-tui`） |
| `src-tauri` | Tauriデスクトップアプリのバックエンド |
| `frontend` | Svelte（Vite）製のGUIフロントエンド |

CLI・TUI・GUIは同じDB/Maildir/検索インデックス/アカウント設定ファイルを共有します（保存場所は後述）。

## 必要なもの

- Rust（2024 edition、[rustup](https://rustup.rs/)推奨）
- Node.js / npm（GUIのフロントエンドビルド用）
- Windowsでインストーラーをビルドする場合: `cargo install tauri-cli --version "^2"`
  （WiX/NSIS自体はtauri-bundlerが自動取得するので別途インストール不要）
- **Linuxでビルドする場合**、上記に加えて以下のシステムパッケージが必要です
  （GUI/TUIどちらをビルドする場合も、依存クレート経由で必要になります）
  - Tauri公式の[Linux向け前提パッケージ](https://v2.tauri.app/start/prerequisites/#linux)一式
    （`webkit2gtk-4.1`、`libgtk-3-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`、
    `build-essential`、`curl`、`wget`、`file`等。GUI (`src-tauri`) 専用）
  - `libfontconfig1-dev`（`pkg-config`込み。フォント選択機能（`font-kit`）がLinuxでは
    Fontconfig経由でインストール済みフォント一覧を取得するため。GUI専用）
  - Cコンパイラ（gcc/clang）と`perl`（TLS通信に使う`native-tls`がLinux/BSDでは
    `vendored`フィーチャでOpenSSLをソースからビルドするため。これにより`libssl-dev`等の
    システムOpenSSL開発ヘッダは不要。CLI/TUI/GUI共通）
  - macOS/WindowsはTLSにOS標準の機構（Security.framework/schannel）を使うため
    上記のOpenSSL関連は無関係です

  なお実際のLinux実機でのビルド・動作確認はこのプロジェクトの開発環境（Windows）からは
  行えていないため、上記は依存クレートのドキュメント調査に基づく想定です。
  問題があれば教えてください。

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

## 使い方（TUI）

```sh
cargo run -p ferro-tui
```

起動すると3ペイン（フォルダ/ラベル｜メッセージ一覧｜本文）が表示されます。主なキー操作:

| キー | 動作 |
| --- | --- |
| `Tab` / `Shift+Tab` | ペイン切替（フォルダ→一覧→本文） |
| `↑`/`k`, `↓`/`j` | カーソル移動 |
| `Enter` | フォルダ/ラベルを選択 |
| `/` | 検索（`Enter`で確定、`Esc`でキャンセル） |
| `r` / `s` / `a` / `d` | 既読切替 / スター切替 / アーカイブ切替 / 削除 |
| `z` → `1`/`3`/`7` | スヌーズ（1日後/3日後/1週間後） |
| `S` | 全アカウントを今すぐ同期 |
| `,` | 設定画面の表示切替（現状は表示のみ） |
| `?` | ヘルプ表示 |
| `q` | 終了 |

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

MSIは英語(`Ferro_x.y.z_x64_en-US.msi`)と日本語(`Ferro_x.y.z_x64_ja-JP.msi`)の2つが生成されます（WiXは言語ごとに別ファイルになる仕様）。NSIS(`setup.exe`)は1つのファイルに両言語が入っており、インストール開始時に言語選択ダイアログが出ます。

## 設定ファイル

Ferroの設定はすべてTOMLファイルとしてアプリデータディレクトリ直下に保存されます（正の情報源。DBは補助的なキャッシュに過ぎません）。CLI/GUI/TUIから編集した内容もこれらのファイルに反映されるので、直接編集してもGUIから変更しても同じように扱われます。

保存場所は環境ごとに以下のようになります。

- Windows: `%APPDATA%\ferro\`
- Linux: `~/.local/share/ferro/`
- macOS: `~/Library/Application Support/ferro/`

### `accounts.toml`（アカウント設定）

パスワードを除くアカウント設定を保持します。直接編集して、CLI/GUIの起動時か `ferro account reload-config` / GUIの「Reload config」ボタンで反映できます。

```toml
[[account]]
name = "Work"
host = "pop.example.com"
port = 995
username = "alice"
use_tls = true
```

ファイルから削除したエントリのメッセージは自動では消えません（削除は`ferro account remove`で明示的に行います）。パスワードはこのファイルには含まれず、OSの資格情報マネージャー（Windows資格情報マネージャー / secret-service等）に保存されます。

### `color_rules.toml`（色分けルール）

特定の文字列を含むメッセージの一覧表示を色分けするルールです。GUIの設定画面から追加/削除できるほか、直接編集して反映することもできます。

```toml
[[rule]]
pattern = "critical"
color = "#b00020"
```

### `settings.toml`（表示・動作設定）

一覧のプレビュー行/差出人アイコン表示、既読にするまでの遅延、バックグラウンド自動同期の間隔、外観（テーマ/アクセントカラー/フォント/文字サイズ）、メールの保持日数を保持します。GUI/TUIの設定画面から変更するのが基本ですが、直接編集して反映することもできます。

```toml
show_preview_line = true
show_sender_avatar = false
mark_read_delay = true
sync_interval_minutes = 5
theme = "light"
accent_color = "#3f6b5c"
font_family = "Noto Sans JP"
font_size = "medium"
retention_days = 0
```

v0.0.6より前はSQLiteのDBに保存していましたが、他の設定ファイルと一貫させるためファイルベースに移行しました。既存ユーザーの設定は初回起動時に自動でこのファイルへ引き継がれます。

### 診断ログ

同期エラー（サーバーとの接続が切れた場合など）は`ferro.log`に記録されます。GUIでは設定画面の「診断」セクションから開けます。不具合を報告する際はこのファイルの内容を添えてもらえると助かります（パスワード等は記録されません）。

## 開発

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets

# 1000万件規模の性能検証（本番データには触れない専用の一時ディレクトリを使う）
ferro bench 10000000
```

開発の背景・設計判断・既知の問題などは[CLAUDE.md](./CLAUDE.md)により詳しくまとまっています。

## リリース

ルートの`Cargo.toml`の`workspace.package.version`（バージョンの正）を上げて`main`にpushすると、
GitHub Actions（`.github/workflows/release.yml`）が同じバージョンのタグ（`vX.Y.Z`）がまだ
`origin`に無いことを確認した上で、Windows/Linux向けにビルドしてGitHub Releaseへ以下を配置します。

- GUIインストーラー（Windows: MSI/NSIS、Linux: deb/AppImage/rpm）
- CLI（`ferro`）・TUI（`ferro-tui`）バイナリをまとめたアーカイブ（Windows: zip、Linux: tar.gz）

バージョンを上げていないpushではリリースは作られません（同じタグの重複作成を防ぐため）。

最新版は[Releasesページ](https://github.com/WatanabeYuito21/ferro/releases/latest)からダウンロードできます。`v0.0.1`が最初のリリースです。

## 既知の制限

- Tantivyの`IndexWriter`がWindows環境で断続的に落ちることがある未解決の問題があります（詳細は[CLAUDE.md](./CLAUDE.md)参照）。自動リトライで大半は吸収されますが、完全な解決には至っていません。
- 検索の日本語対応は形態素解析ではなくCJKバイグラム（機械的な2文字ずつの部分一致）によるものです。
- SMTP送信機能はありません（受信専用）。
