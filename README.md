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
  受信日時の新しい順で表示される）。GUIの設定画面「検索」セクションで、検索できる件数
  （投入済み / 全件）と索引の文書数を確認できる
- 詳細検索: `from:` `subject:` `body:` `has:attachment` `is:unread` `is:starred` `in:archive`
  `label:` `after:2026-01-01` `before:` などの条件を検索欄に書ける（`-`で否定、日本語のキー名も可）。
  GUIでは検索欄横の「詳細」から入力フォームでも指定できる（後述の「検索の書式」参照）
- 既読/フラグ/削除といったメッセージ状態の管理（GUIでは複数選択して一括操作も可能）
- GUIではフォルダ（受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱）と色付きラベルによる整理、
  設定画面（表示・同期間隔・検索インデックス再構築・外観）
- 外観設定（テーマ: ライト/ダーク/システム追従、アクセントカラー、フォント（PCにインストール
  済みのフォントから選択可能）、文字サイズ）
- 件名/差出人/本文プレビューに特定の文字列を含むメッセージを一覧上で色分けする色分けルール
  （設定画面から追加/削除でき、`color_rules.toml`を直接編集して反映することもできる）
- 差出人/件名/本文への部分文字列一致・添付ファイルの有無を条件に、受信時に自動でラベルを
  付与したり、既読にする・アーカイブする・スターを付ける・新着通知を出さない、を行う受信ルール
  （GUI設定画面・CLI`ferro label-rule`の両方で追加/削除でき、`label_rules.toml`を直接編集して
  反映することもできる。既存の受信済みメールにも遡って適用する「今すぐ適用」も用意。定期的な
  正常終了の通知を自動で既読・アーカイブにする、といった使い方ができる）
- メールの保持日数を設定でき、期限切れメールを6時間ごとに（または設定画面の「今すぐ整理する」で
  即座に）ローカルから完全に削除する。既定は無期限（自動削除しない）で、スター付きメールは
  保持日数に関わらず削除されない
- アカウント設定は`accounts.toml`で管理し、直接編集して反映できる（パスワードはOSの資格情報マネージャーに保存）。
  GUIの設定画面からファイルを直接開くこともできる
- GUIでは自動バックグラウンド同期（間隔は設定画面から変更可能。デフォルト5分）
- アカウントごとの同期状態（最後に同期した時刻・取得件数、失敗が続いていればその回数・最後に
  成功した時刻・エラー内容、UIDL非対応かどうか）を、GUI/TUIの設定画面と`ferro account list`で表示する
- GUIの新着メールのデスクトップ通知（特定のラベルが付いた新着だけに絞れる。受信時の自動ラベル
  付けルールと組み合わせると「重要なアラートだけ通知」ができる）と、システムトレイ常駐
  （トレイアイコンから表示・今すぐ同期・終了。「閉じたらトレイに常駐」をオンにすると、
  ウィンドウを閉じても同期と通知を続ける）
- スレッド表示（GUI）: 返信のやり取り（`In-Reply-To`/`References`で判定。件名が同じだけのメールは
  まとめない）を、一覧では件数バッジで、本文表示では「このスレッドのメッセージ」欄で示す。
  スレッド表示を入れる前に受信していたメールは、アップデート後の初回起動時にバックグラウンドで
  判定する（Maildirを読み直すので、件数によっては数分〜十数分かかる）
- 添付ファイルの一覧表示・保存
- メッセージを`.eml`ファイル（受信した生メールそのまま）として書き出す（GUIの本文表示の
  「.emlとして保存…」、CLIの`ferro export`）。他のメーラーで開いたり、ヘッダーを確認したりできる
- 保存した検索（スマートフォルダ）: 検索欄の条件（`from:監視 is:unread -subject:復旧`のような
  書式も含む）に名前を付けて保存し、サイドバーから1回で開ける。未読数も表示する（GUIは検索欄の
  「保存」、TUIはサイドバーの「保存した検索」。`saved_searches.toml`を直接編集してもよい）
- 生のヘッダー・ソースの表示（GUIは本文表示の「本文/ヘッダー/ソース」タブか`v`キー、TUIは`H`キー）。
  経路上のゲートウェイが付けたヘッダーや`Received`の連なりを、ファイルに書き出さずに確認できる
  （ソースは大きなメールなら先頭512KBまで）
- CLI/TUI/GUIどれからでも同じデータ（DB・Maildir・検索インデックス）を扱える
- TUI版（`ferro-tui`）: 3ペイン構成（フォルダ/ラベル｜メッセージ一覧｜本文）のターミナルUI。
  既読/スター/アーカイブ/削除/スヌーズ、検索、全アカウント手動同期、バックグラウンド同期・
  検索キャッチアップ・保持期間クリーンアップに対応。設定画面では各設定の変更、色分けルールと
  アカウントの追加・削除ができる（GUIと同じ設定ファイルを読み書きする）

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

# メッセージ一覧・検索（検索の書式は後述。CLI/TUIでは日付をUTCの0時で区切る）
ferro list
ferro search "検索語"
ferro search "from:alice has:attachment after:2026-01-01"

# 既読/フラグ/削除
ferro read <message_id>
ferro flag <message_id>
ferro delete <message_id>

# メッセージを.eml（保存済みの生メールそのまま）として書き出す（-o省略時は件名から作ったファイル名）
ferro export <message_id> -o message.eml

# 全メッセージから検索インデックスを作り直す（端末では進捗を表示）
ferro reindex

# UIDL非対応アカウントの重複メッセージを検出（既定は件数表示のみ。--applyで削除。GUI/TUIを閉じて実行）
ferro dedupe
ferro dedupe --apply

# ラベル一覧（作成/削除はGUI限定）
ferro label list

# 受信ルール（自動ラベル付け・既読/アーカイブ/スター/通知しない）
ferro label-rule add --label 領収書 --subject 領収書 --has-attachment true
ferro label-rule add --subject "[OK]" --mark-read --archive
ferro label-rule list
ferro label-rule remove <index>
ferro label-rule apply   # 既存の受信済みメールにも遡って適用する
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
| `/` | 検索（`Enter`で確定して結果の中を移動、`Esc`で元の一覧に戻る） |
| `r` / `s` / `a` / `d` | 既読切替 / スター切替 / アーカイブ切替 / 削除 |
| `z` → `1`/`3`/`7` | スヌーズ（1日後/3日後/1週間後） |
| `H` | 本文 / 生のヘッダーの表示切替 |
| `S` | 全アカウントを今すぐ同期 |
| `,` | 設定画面の表示切替 |
| `?` | ヘルプ表示 |
| `q` | 終了 |

保存した検索（`saved_searches.toml`）はサイドバーのラベルの下に未読数付きで並び、`Enter`で開きます。

設定画面（`,`）では`j`/`k`で項目を選び、`Enter`（または`Space`）で切り替え・入力、`←`/`→`で選択肢の切り替えや数値の増減、`d`で色分けルール/アカウントの削除をします。「＋ 色分けルールを追加」「＋ アカウントを追加」を選んで`Enter`を押すと入力フォームが開きます（`Tab`で欄を移動、`Enter`で次の欄へ・最後の欄で確定、`Esc`で取り消し）。変更はすぐに設定ファイルへ保存されます。アカウントの削除はそのアカウントのメールも消えるため、`y`で確認してから行います。

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

設定画面（View > Settings…）の上部の検索欄で、設定項目を絞り込めます（空白区切りでAND、全角/半角・大文字小文字は区別しません。Escでクリア）。

メッセージ画面はキーボードでも操作できます（`?`で一覧を表示。検索欄などの入力中は効きません）:

| キー | 動作 |
| --- | --- |
| `j`/`↓`, `k`/`↑` | 次 / 前のメッセージ（選ぶと本文を表示） |
| `Enter` / `o` | 先頭のメッセージを開く |
| `Esc` | 本文を閉じる / 検索をやめる（入力欄からは抜ける） |
| `/` | 検索欄へ移動 |
| `g` → `i`/`s`/`z`/`a`/`t` | 受信箱 / スター付き / スヌーズ / アーカイブ / ゴミ箱へ |
| `e` / `s` / `u` | アーカイブ切替 / スター切替 / 既読・未読切替 |
| `#` / `Delete` | 削除 |
| `b` / `l` | スヌーズ / ラベルのメニュー |
| `v` | 本文 / ヘッダー / ソースの表示切替 |
| `x` | 開いているメッセージを選択 / 解除（複数選択） |

検索欄に条件を入れると「保存」ボタンが出て、名前を付けてサイドバーの「保存した検索」に登録できます（同じ名前で保存し直すと条件を置き換えます。行にマウスを乗せると出る×で削除）。開くたびにその時点の内容で検索し直し、サイドバーには未読の件数を出します。検索結果は新しい方から200件まで表示します。

アーカイブや削除などで表示中のフォルダから外れたメッセージは一覧から消え、自動的に次のメッセージが開きます（一覧は作り直さないので、スクロール位置はそのまま）。

一覧の行頭のチェックボックス、`Ctrl`+クリック（1件ずつ）、`Shift`+クリック（範囲）で複数のメッセージを選ぶと、一覧の上に操作バーが出て、既読/未読・スター・アーカイブ・スヌーズ・ラベルの付け外し・削除をまとめて行えます。選択中は`e`/`s`/`u`/`#`も選択中のすべてに効きます（`Esc`で選択解除）。

## 検索の書式

検索欄（GUI/TUI）と`ferro search`では、普通の検索語に加えて次の書式で絞り込めます。空白で区切った条件はすべて満たすもの（AND）が出ます。

| 書式 | 意味 |
| --- | --- |
| `請求書` | 件名・差出人・本文のどこかに含む（部分一致） |
| `from:alice` / `差出人:alice` | 差出人（表示名・アドレス）に含む |
| `subject:障害` / `件名:障害` | 件名に含む |
| `body:手順` / `本文:手順` | 本文に含む |
| `has:attachment` / `has:添付` | 添付ファイルあり |
| `is:unread` / `is:read` / `is:starred` | 未読 / 既読 / スター付き（`is:未読`等も可） |
| `in:inbox` / `in:archive` / `in:trash` など | フォルダ（`in:受信箱`等も可。指定しないとゴミ箱以外のすべて） |
| `label:仕事` / `ラベル:仕事` | ラベル（名前の完全一致。大文字小文字は区別しない） |
| `after:2026-01-01` / `before:2026-02-01` | 受信日（afterはその日を含み、beforeは含まない） |

- 先頭に`-`を付けると否定です（`-from:noreply`、`-has:attachment`）。
- 空白を含む値は`"`で囲みます（`subject:"週次 レポート"`）。
- キー名が分からない語・値が解釈できない語（`before:あした`等）は普通の検索語として扱います。
- 検索語と、添付・既読・フォルダ・ラベルの条件を組み合わせた場合は、検索語に一致する新しい方から最大5000件の中で絞り込みます。一致が非常に多い語で古いメールを探すときは、`after:`/`before:`で期間も指定してください（期間は全件から絞り込めます）。

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

### `label_rules.toml`（受信ルール）

差出人/件名/本文への部分文字列一致・添付ファイルの有無を条件に、受信時に自動でラベルを
付与したり、既読にする等の動作を行うルールです。指定した条件はすべて満たした場合にのみ
当たります（AND。「いずれか」を表したい場合は同じ動作のルールを複数書きます）。`label_name`は
既存のラベル名（`labels.name`）を指し、ラベルが削除/リネームされて解決できなくなった
ルールはラベルの付与だけが静かに無視されます。`label_name`を省略すると、ラベルは付けずに
動作だけを行います。GUIの設定画面・`ferro label-rule`の両方から追加/削除でき、
直接編集して反映することもできます。

| 動作 | 意味 |
| --- | --- |
| `mark_read = true` | 既読にする（既読のメールは新着通知も出ない） |
| `archive = true` | アーカイブする（受信箱に出さない） |
| `star = true` | スターを付ける |
| `skip_notification = true` | 未読のまま、新着のデスクトップ通知だけ出さない |

「今すぐ適用」はラベル・既読・アーカイブ・スターを既存のメールにも当てます（立てるだけで、
外すことはしません）。

```toml
[[rule]]
label_name = "領収書"
subject_contains = "領収書"
has_attachment = true

[[rule]]
subject_contains = "[OK]"
mark_read = true
archive = true
```

### `saved_searches.toml`（保存した検索）

サイドバーの「保存した検索」です。`query`は検索欄と同じ書式です。

```toml
[[search]]
name = "未対応の障害"
query = "subject:障害 is:unread -subject:復旧"
```

### `settings.toml`（表示・動作設定）

一覧のプレビュー行/差出人アイコン表示、既読にするまでの遅延、バックグラウンド自動同期の間隔、外観（テーマ/アクセントカラー/フォント/文字サイズ）、メールの保持日数、更新の自動確認、新着通知・トレイ常駐を保持します。GUI/TUIの設定画面から変更するのが基本ですが、直接編集して反映することもできます。

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
# サーバー側(POP3)のメールを、この日数より古くなったら同期時にDELEで削除する。0は削除しない。
# ローカルに保存済みのメールだけが対象（UIDL非対応アカウントは対象外）。
server_delete_days = 0
# GUIで1日1回、新しいバージョンを確認してバナーで知らせるか。
auto_update_check = true
# GUIのバックグラウンド同期で新着があったらデスクトップ通知を出すか（ウィンドウを見ている間は出さない）。
notify_new_mail = true
# 通知の対象にするラベル名。空なら全ての新着を通知する。
notify_labels = []
# GUIのウィンドウを閉じてもシステムトレイに常駐して同期を続けるか（終了はトレイのメニューから）。
close_to_tray = false
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
バージョンを上げる際は`Cargo.toml`・`src-tauri/tauri.conf.json`・`frontend/package.json`の3か所を揃えます
（インストーラー名は`tauri.conf.json`の値で決まるため、ずれているとワークフローがエラーで止まります）。

### 自動アップデート（GUI版）

設定画面の「アップデート」→「更新を確認」（またはメニューバーの View > Check for Updates…）で、最新のGitHub Releaseの`latest.json`を見て、
新しいバージョンがあれば署名を検証した上でダウンロード・インストールし、Ferroを再起動します
（Tauri公式の`tauri-plugin-updater`。Windows: NSIS、Linux: AppImageが対象で、MSI/debは対象外）。
また、1日1回自動で新しいバージョンを確認し、あれば画面上部のバナーで知らせます（「今すぐ更新」で更新、
「後で」で閉じる。翌日の確認でまた表示されます）。自動確認は知らせるだけで、勝手にダウンロード・
インストールはしません。設定画面の「1日1回、更新を自動確認」でオフにできます（`settings.toml`の
`auto_update_check`）。CLI/TUIは対象外です。

更新の署名には専用の鍵ペアを使います（インストーラーのコード署名とは別物）。

1. `cargo tauri signer generate -w $HOME/.tauri/ferro.key`で鍵を作る（PowerShellでは`~`が展開されずカレントに`~`フォルダができるので`$HOME`を使う）
2. 公開鍵を`src-tauri/tauri.conf.json`の`plugins.updater.pubkey`に貼る
3. 秘密鍵の中身を、GitHubリポジトリのSecrets `TAURI_SIGNING_PRIVATE_KEY`に登録する
   （パスワードを付けたなら`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`も）

秘密鍵を失うと、既存ユーザーへ更新を配れなくなります。

最新版は[Releasesページ](https://github.com/WatanabeYuito21/ferro/releases/latest)からダウンロードできます。`v0.0.1`が最初のリリースです。

## 既知の制限

- 検索インデックスの再構築中も一覧・本文表示などはそのまま操作できますが、再構築が終わるまでは検索結果が不完全になります（進捗は設定画面に表示されます）。
- Tantivyの`IndexWriter`がWindows環境で断続的に落ちることがある未解決の問題があります（新しい索引ファイルの作成が「アクセスが拒否されました」で失敗する。詳細は[CLAUDE.md](./CLAUDE.md)参照）。落ちたら作り直して自動リトライするので大半は吸収されますが、原因自体は解決できていません。発生すると`ferro.log`に`search:`で始まる行が記録されます。
- 検索の日本語対応は形態素解析ではなくCJKバイグラム（機械的な2文字ずつの部分一致）によるものです。
- SMTP送信機能はありません（受信専用）。
- RFC2047でエンコードされず生のlegacy charset（Shift_JIS/EUC-JP/ISO-2022-JP）バイト列が
  直接入っているヘッダーの文字化けは、件名(Subject)のみ復元処理を入れています。
  差出人表示名（From）等の他のヘッダーで同じ原因の文字化けが起きる可能性は残っています。
