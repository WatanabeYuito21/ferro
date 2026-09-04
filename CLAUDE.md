# CLAUDE.md — Ferro

このファイルはClaude Codeがこのプロジェクトで作業する際のコンテキストです。

## プロジェクト概要

**Ferro** は、1000万件規模のメールでも検索・起動が高速な自作メーラーです。
既存の neomutt（TUI）や Thunderbird（Electron/Chromium系）が大量メールで重くなる問題を、
アーキテクチャレベルで解決することを目指します。

- バイナリ名: `ferro`
- リポジトリ/crate名: `ferro-mail`
- 開発者: 個人プロジェクト（インフラ/クラウド運用エンジニアが趣味で開発）

## 現在の実装状況

コアの一連の機能（POP3受信→保存→検索→閲覧）はCLI・GUIともに一通り実装済み。

- `ferro-core`: POP3クライアント（USER/PASS/STAT/LIST/UIDL/RETR/DELE/QUIT/STLS、暗黙的TLS、
  RETRパイプライン化＋切断時の再接続再開）、SQLiteスキーマ（accounts/messages、マイグレーション、
  キーセットページネーション）、Maildir保存（ハッシュ分散、FNV-1a）、Tantivy全文検索（索引投入・
  検索・全件再構築）、アカウント作成・削除（keyring連携、失敗時ロールバック）、メール本文/添付
  パース（`mail-parser`ベース）
- `ferro-cli`: `account add/list/remove`, `sync`, `list`, `search`, `reindex`, `bench`（開発用の
  スループット/レイテンシ計測、`$TMPDIR/ferro-bench`に専用データを生成し本番データには触れない）
- `src-tauri` + `frontend`（Svelte）: アカウント管理、同期、メッセージ一覧（自前仮想スクロール）、
  全文検索、メッセージ詳細表示（本文プレーンテキスト・添付一覧・保存ダイアログ）
- 未着手/既知の課題: 日本語対応トークナイザ（Tantivyのデフォルトは空白区切り前提で日本語の
  分かち書きに向かない）、Windowsパッケージング仕上げ、下記のTantivy/Windows信頼性の既知の問題

## 要件

- 受信は **POP3のみ**。SMTP送信機能は不要（受信専用ツール）
- 対象規模: メール**1000万件**でも実用的な起動速度・検索速度を維持する
- 起動時に毎回全件スキャンするような設計は禁止（既存メーラーの遅さの主因）

## 技術スタック方針

- **アプリフレームワーク**: Tauri（Chromiumを内包しないため軽量。バックエンドはRust）。
  Cargoワークスペース構成（`ferro-core`=POP3/DB/Maildir/検索ロジックのライブラリ、`ferro-cli`=CLI、`src-tauri`=Tauriアプリ）とし、
  CLIとGUIが同じDB/Maildir/検索インデックスのパスを共有できるようにする
- **フロントエンド**: Svelte（Vite）を採用予定。仮想スクロールは自前実装（固定行高ウィンドウイング + 無限スクロールページング）とする
- **メタデータDB**: SQLite（From/To/Subject/Date/フラグ/スレッドIDなどを構造化して保持。一覧・ソート・フィルタ用）。
  一覧取得はOFFSETではなくキーセットページネーション（`date_header`カーソル）を使う方針
- **全文検索インデックス**: **Tantivy**（Rust製、純Rust実装）を採用する。
  notmuch（Xapian/C実装）はWindows配布が実質困難なためTauriアプリの配布方針と合わず不採用。
  検索エンジンはSQLiteと疎結合（`messages.id`をdoc idとして流用、`fts_indexed_at`/`fts_doc_version`で連携）とし、
  Subject/From（表示名・アドレス）/本文（Maildirから読み直したプレーンテキスト）を索引化する
- **生メール保存**: Maildir形式（1ファイル1メッセージ、ハッシュ分散で`cur/xx/yy/<account_id>-<uidl>.eml`に格納）とする。
  SQLite BLOB案は不採用（将来の切替に備えて`messages.raw_storage_kind`/`raw_blob`カラムは設計上想定しておく）
- **POP3クライアント**: 自前実装する（USER/PASS/STAT/LIST/UIDL/RETR/DELE/QUIT/STLS）。
  接続モードは`accounts.use_tls`（アカウント作成時にユーザーが明示的に選ぶ）とポート番号で決める:
  `use_tls=false`（`--allow-plaintext`/GUIで明示選択時のみ）は完全平文、
  `use_tls=true`かつポート995は暗黙的TLS（POP3S）、それ以外はSTLS(STARTTLS)アップグレード、という設計にする。
  平文接続の明示的opt-inを用意するのは、暗黙的TLSもSTARTTLSも使えない平文専用サーバーが実在するため。
  TLSは`native-tls`を使う。メールパースは`mail-parser`crateを利用する（`full_encoding`フィーチャ必須）。
  パスワードは`keyring`crateでOSの資格情報マネージャーに保存する方針。
  RETRはパイプライン化する（複数件まとめて送信してから順に応答を読む）ことで速度向上を狙う。
  POP3に正式なパイプライン拡張はないが、TCP上の厳密に順序付けられた1コマンド=1応答ストリームであることを利用する。

## 設計上の重要な原則

1. **起動時に何も舐めない** — 起動時はインデックス（DB/検索エンジン）を開くだけにする
2. **検索はインデックス済みデータを引くだけ** — grep的な全文走査は禁止
3. **メッセージ一覧は仮想スクロール必須** — 1000万件を素朴にレンダリングしない
4. **新着メールのインデックス投入は非同期バックグラウンドタスク** — UIをブロックしない

## メール詳細表示（設計方針）

- 本文はプレーンテキストのみ表示（HTML本文はタグを剥がしたテキストとして表示。`<pre>`にSvelteのテキスト補間で描画すればHTMLとして解釈されることはない）
- 添付ファイルは一覧表示し、`tauri-plugin-dialog`の保存ダイアログでユーザーが選んだ場所にローカル保存できるようにする

## 実装時に注意すべき既知の落とし穴

- **`mail-parser`の`full_encoding`フィーチャ**: cargo addで依存クレート名と同名のダミーフィーチャが
  黙って生成されることがある。「encoding_rs」のような名前を指定しても実際には何も有効化されず、
  日本語Shift_JIS件名などが文字化けする原因になりうる。フィーチャ名が実際に効いているかは要確認。
- **`keyring`crate（Linux/secret-service）**: WSL等でSecret Service D-Busサービスが動いていないと
  `org.freedesktop.secrets was not provided`のようなエラーになる。また`default`という名前の
  コレクションエイリアスが未設定だと保存時に`no result found`になりうる（通常はログイン時にPAM経由で
  作られるが、WSLのような環境では作られないことがある）。
- **ポート995での接続失敗**: 暗黙的TLS前提でポート995に繋いでも、サーバーが実際にはTLSを話さない
  ケースがある（`wrong version number`エラー）。ポート110で平文接続し`CAPA`応答に`STLS`があるか
  確認してから接続方式を判断する設計にしておくと安全。
- **SQLiteのクエリで`(?1 IS NULL OR col = ?1)`のようなOR分岐を書くと**、インデックスを使った
  範囲検索（SEARCH）ではなくCOVERING SCAN + 一時B-treeソートにフォールバックしやすい。
  新しいクエリを書く際は`EXPLAIN QUERY PLAN`で`SCAN`ではなく`SEARCH`になっているか確認すること。
- **RETRのパイプライン化**は実サーバー次第で接続を切断されることがありうるため、
  「セッションが切れたら再接続して続きから再開する」仕組みを最初から設計に入れておくとよい
  （UIDL差分方式なら再開しても安全＝既に保存済みのメッセージは再取得されない）。実装済み
  （`ferro_core::sync`のセッションループ、`Pop3Error::ConnectionClosed`）。
- **Tantivyの`IndexWriter`がWindarows実機で断続的に死ぬ**: `commit()`が
  `"An error occurred in a thread: 'An index writer was killed...'"`で失敗することがある
  （`ferro bench`で1000〜数千件規模の索引投入を繰り返すと、体感1〜2割の頻度で再現）。
  原因はマージ/GCワーカースレッドの異常終了とみられ、アンチウイルスのリアルタイムスキャン等に
  よるファイルI/O競合が疑わしいが未確定。一度死んだ`IndexWriter`はそのDirectoryに対する
  以後の`add_document`/`commit`が全て同じエラーで失敗し続ける。
  対策として`SearchIndex`は`commit`失敗時に新しい`IndexWriter`を作り直す自己修復
  （`recover_writer`）を持つが、**古いwriterを先にdrop（ロックファイル解放）してから
  新しいwriterを作らないと`LockFailure`になる**点に注意（一度実装を誤り、直後に修正した）。
  `reindex_all`/`ferro bench`はバッチ単位でこの自己修復＋指数バックオフ付きリトライ
  （最大5回）を行うが、それでも救えないケース（同一プロセス内で該当ディレクトリに対して
  繰り返し失敗する）が残っている。真の恒久対策ではないため、`ferro reindex`等が
  この種のエラーで失敗し続ける場合はプロセスを再起動する（`SearchIndex::open_or_create`を
  最初からやり直す）か、`search_index`ディレクトリを削除して再構築する
  （完全にSQLite/Maildirから再構築可能な派生キャッシュなので安全）のが実用上の回避策。
  なお`SearchIndex::create_in_ram`（テスト専用）はこの問題を踏まない。

## 未決定・要検討事項

- Tantivyの日本語トークナイザ対応（未着手、上記「現在の実装状況」参照）
- 上記のTantivy/IndexWriter信頼性問題への根本対応（現状はバッチ単位リトライ＋
  自己修復で緩和のみ。真因はWindows実機でしか再現しておらず未特定）
- 次に着手するテーマは都度相談して決める

## Git運用ルール

- ソースはgitで管理する。デフォルトブランチは`main`
- **1機能1ブランチ**で開発する（例: `feature/pop3-client`, `feature/maildir-storage`）
- **マージ後もブランチは削除せず残す**。マージは`git merge --no-ff`でfast-forwardさせず、
  ブランチの存在と履歴が`git log --graph`等で追えるようにする
- ブランチを削除する操作（`git branch -d/-D`、`git push --delete`等）は明示的に指示されない限り行わない

## 開発者の背景（参考情報）

- インフラ/クラウド運用エンジニア（AWS/Azure、CDN、WAF、DB管理、Windows Server等が本業）
- Vim/Neovim、tmuxを使った開発ワークフローに慣れている
- Rust/Tauriでのデスクトップアプリ開発は本プロジェクトが実践の場
