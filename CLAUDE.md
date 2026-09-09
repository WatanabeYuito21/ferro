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
  検索・全件再構築、日本語対応のCJKバイグラムトークナイザ自前実装）、アカウント作成・削除
  （keyring連携、失敗時ロールバック）、アカウント設定ファイル（`accounts.toml`）との同期
  （`account_config::reconcile`）、メール本文/添付パース（`mail-parser`ベース）
- `ferro-cli`: `account add/list/remove/reload-config`, `sync`, `list`, `search`, `reindex`, `bench`
  （開発用のスループット/レイテンシ計測、`$TMPDIR/ferro-bench`に専用データを生成し本番データには
  触れない）、`read`/`flag`/`delete`（既読・フラグ・論理削除）
- `src-tauri` + `frontend`（Svelte）: アカウント管理、同期、メッセージ一覧（自前仮想スクロール）、
  全文検索、メッセージ詳細表示（本文プレーンテキスト・添付一覧・保存ダイアログ）、
  フォルダ/ラベルによる整理、設定画面、色分けルール（`color_rules.toml`。件名/差出人/
  本文プレビューに特定文字列を含むメッセージの一覧表示を色分けする。マッチ判定は
  フロント側の`colorRules.js`が行う。`ferro_core::color_rules`参照。マッチ対象の
  `preview`はsync時に本文冒頭から最大500文字まで切り詰めて保存したもの
  （`mail::parse::make_preview`）で全文ではないため、それより後ろにしか
  現れない文字列を狙ったルールは効かない。以前は120文字だったため一部のルールが
  効かないという形で実際に踏み、500文字に広げると同時に`reindex_all`が
  Maildirから読み直した内容で既存メッセージの`preview`/`attachment_count`も
  遡って更新するようにした）、外観設定（テーマ: ライト/ダーク/システム追従、
  アクセントカラー、フォント、文字サイズ。`ferro_core::db::settings::Settings`に
  永続化し、フロント側の`frontend/src/lib/appearance.js`が`<html>`の
  data属性とCSSカスタムプロパティ（`app.css`）を書き換えて反映する。
  `--accent-hover`/`--accent-soft-bg`は固定値ではなく`color-mix()`で
  `--accent`から動的導出するため、アクセントカラーを変えても一貫した
  hover色・薄い背景色が得られる。当初「モックアップ自体がライトテーマのみの
  設計」としてダークモード非対応だったが、外観設定の一部として追加した）。
  フォントは当初4種→8種の固定候補だったが、「PCにインストールされてる
  フォントから選べるといい」という要望を受け、`font-kit`crate
  （`src-tauri`のみに追加。Windowsでは追加のネイティブライブラリ無しで
  DirectWriteバックエンドが使われる）でOS側のフォント一覧を列挙する
  `list_installed_fonts`コマンドに置き換えた。`Settings::font_family`は
  固定候補ではなく任意の文字列（実際のフォント名）を受け付け、フロント側
  （`appearance.js`）が常に`'指定名', 'Noto Sans JP', system-ui, sans-serif`
  というフォールバックチェーンを付けて適用するため、存在しないフォント名が
  入っていても表示が壊れることはない。
  メールの保持期間（`Settings::retention_days`。日数。0が既定＝無期限で、
  既存ユーザーが不意にメールを失わないようにしている）も設定できる。
  `src-tauri`の`spawn_retention_cleanup`が6時間ごとにバックグラウンドで
  `message_actions::purge_expired_batch`を呼び、`date_header`が保持期間より
  古いメッセージをDB行・Maildirファイル・検索インデックスの全てから完全に
  削除する（`set_deleted`の論理削除とは別物。ディスクを実際に回収する）。
  スター付き(is_flagged=1)は保持期間に関わらず対象外にする安全策を入れている
  （`idx_messages_retention`部分インデックス）。設定画面の「今すぐ整理する」
  ボタン(`purge_expired_messages`コマンド)で即座に実行することもできる。
  `list_older_than`は常に「date_headerが古い順のN件」を返すため、特定の
  バッチのTantivy投入が繰り返し失敗した場合に後続の期限切れメッセージへ
  永久に手が届かなくなる、という検索キャッチアップで踏んだのと同じ罠を
  避けるため、`purge_expired_batch`も同じ「バッチ全体失敗→1件ずつ
  フォールバック」の設計にしてある。
- `ferro-tui`: `ratatui`+`crossterm`によるTUI版（neomutt的な使い方を想定。
  CLAUDE.md冒頭の「既存のneomutt」への言及どおり）。GUIと同じ`ferro-core`を
  土台にし、同じDB/Maildir/検索インデックス/`accounts.toml`/`color_rules.toml`を
  共有する。3ペイン構成（フォルダ/ラベル｜メッセージ一覧｜本文）、既読/スター/
  アーカイブ/削除/スヌーズ、検索（`/`キー、Tantivyへの都度クエリで即時反映）、
  全アカウント手動同期を実装済み。バックグラウンドスレッド（同期・検索
  キャッチアップ・保持期間クリーンアップ）は`src-tauri`の対応する関数の移植
  （Tauriの`AppHandle::emit`の代わりに`std::sync::mpsc`でメインループへ通知する。
  `ferro-tui/src/background.rs`参照）。設定画面は現状「表示のみ」（トグル/数値の
  編集、色分けルールの追加/削除、アカウント管理フォームは未実装。フォーム入力が
  要るこれらは今後のフェーズで対応予定）。
- Windowsインストーラー（MSI/NSIS）のビルドも確認済み。`cargo install tauri-cli --version "^2"`で
  `cargo tauri`コマンドを導入した上で`cargo tauri build`を実行する（WiX/NSISは未導入でも
  tauri-bundlerが自動取得する）。成果物は`target/release/bundle/{msi,nsis}/`
  （実測: MSI約7.7MB、NSIS約4.5MB。Electron系との比較で軽量という当初方針どおり）
- **リリース自動化**（`.github/workflows/release.yml`）: `main`へのpush毎に起動するが、
  実際にビルド・GitHub Releaseを作るのはルート`Cargo.toml`の`workspace.package.version`
  に対応するタグ（`vX.Y.Z`）がまだ`origin`に存在しない場合のみ（`git ls-remote --tags origin`で
  判定）。単なるコミットの積み重ねでリリースが増殖しないようにするための制御で、
  バージョンを上げる操作自体がリリースのトリガーになる設計。Windows/Linuxの2ランナーで
  `cargo build --release -p ferro-cli -p ferro-tui`（CLI/TUIバイナリ）と`cargo tauri build`
  （GUIインストーラー: Windows MSI/NSIS、Linux deb/AppImage）の両方を行い、
  CLI/TUIバイナリはzip（Windows）/tar.gz（Linux）にまとめて、インストーラー成果物と
  一緒に同じGitHub Releaseへ添付する。初回リリースは0.0.1
  （既存の0.1.0から意図的に下げた。「本格的なリリースの開始点」として0.0.1から
  振り直したいというユーザーの意向による）。
- 未着手/既知の課題: 下記のTantivy/Windows信頼性の既知の問題

## 要件

- 受信は **POP3のみ**。SMTP送信機能は不要（受信専用ツール）
- 対象規模: メール**1000万件**でも実用的な起動速度・検索速度を維持する
- 起動時に毎回全件スキャンするような設計は禁止（既存メーラーの遅さの主因）

## 技術スタック方針

- **アプリフレームワーク**: Tauri（Chromiumを内包しないため軽量。バックエンドはRust）。
  Cargoワークスペース構成（`ferro-core`=POP3/DB/Maildir/検索ロジックのライブラリ、`ferro-cli`=CLI、
  `ferro-tui`=ratatui製TUI、`src-tauri`=Tauriアプリ）とし、
  CLI・TUI・GUIが同じDB/Maildir/検索インデックスのパスを共有できるようにする
- **フロントエンド**: Svelte（Vite）を採用予定。仮想スクロールは自前実装（固定行高ウィンドウイング + 無限スクロールページング）とする
- **メタデータDB**: SQLite（From/To/Subject/Date/フラグ/スレッドIDなどを構造化して保持。一覧・ソート・フィルタ用）。
  一覧取得はOFFSETではなくキーセットページネーション（`date_header`カーソル）を使う方針
- **アカウント設定は`accounts.toml`（`paths::accounts_config_path`、アプリデータディレクトリ直下）が
  正の情報源**。パスワードを除くname/host/port/username/use_tlsをここに保持し、
  CLI/GUIの起動時（および明示的な`ferro account reload-config`/GUIの「Reload config」ボタン）に
  `account_config::reconcile`でDBの`accounts`テーブルへ反映する。`name`をキーに既存行と照合し、
  見つかれば内部idを維持したまま設定を上書き（keyring/messages/Maildirとの紐付けを保つため）、
  見つからなければ新規作成する。**ファイルから消えたアカウントは自動削除しない**
  （削除するとメッセージも消える破壊的操作のため、意図せぬファイル編集でメールを失わないように。
  削除は`ferro account remove`/GUIのRemoveボタンで明示的に行う。この操作は設定ファイル側の
  該当エントリも合わせて取り除く）。CLI/GUIの「アカウント追加」操作は直接DBには書き込まず、
  `accounts.toml`に追記→保存→reconcileという経路を通る（手編集と同じ土俵に乗せるため）。
  パスワードはこのファイルに含めず、従来どおりkeyringのみで管理する。
- **全文検索インデックス**: **Tantivy**（Rust製、純Rust実装）を採用する。
  notmuch（Xapian/C実装）はWindows配布が実質困難なためTauriアプリの配布方針と合わず不採用。
  検索エンジンはSQLiteと疎結合（`messages.id`をdoc idとして流用、`fts_indexed_at`/`fts_doc_version`で連携）とし、
  Subject/From（表示名・アドレス）/本文（Maildirから読み直したプレーンテキスト）を索引化する。
  トークナイザはTantivy標準の空白区切り（"default"）ではなく、`ferro_core::search::cjk_tokenizer`で
  自前実装したバイグラム（2文字ずつ重複ありで分割）を使う。当初は英数字は通常の単語区切り
  （単語全体の完全一致）にしていたが、「検索が単語検索っぽい」という指摘のとおり利用者の直感に
  反していたため、英数字もCJKと同じくバイグラム化して部分一致検索にした（CJK文字種と英数字とで
  runの区切りは分ける。地続きにすると"テスト123"の"ト"と"1"のような無意味な文字種またぎの
  バイグラムができてしまうため）。
  クエリの構築は`QueryParser`を使わず`SearchIndex::search`/`build_word_query`で自前で組み立てる。
  空白区切りの「単語」ごとに件名/差出人/本文フィールド用のトークン列を作り、
  `tantivy::query::PhrasePrefixQuery`（最後のトークンだけを前方一致で展開するフレーズクエリ）を
  組み立てる。単語間はAND、1単語に対する3フィールドはOR。位置(position)込みのフレーズなので、
  バイグラムが単に文書内のどこかに散らばっているだけでは誤ってヒットしない（実際に元の文字列が
  部分文字列として連続する場合だけヒットする）。最後のトークンを前方一致にしているのは、
  「srv-jpp-w02」を「srv-jpp-w」で検索してもヒットしない、という指摘への対応:
  バイグラムトークナイザは2文字未満の断片（クエリ末尾の1文字「w」等）をそのまま1トークンとして
  扱うが、文書側の対応する語("w02")はバイグラム("w0","02")になっているため、単純な完全一致の
  フレーズクエリだとクエリが単語の途中で終わっている場合に一致しなかった（最初は当初`QueryParser`
  のAND結合のみで実装していたが、この理由で不十分と判明し書き直した）。
  形態素解析（lindera等）は辞書同梱で数十MB単位のバイナリ増になり、notmuch不採用の理由（軽量な
  デスクトップ配布を優先）と同じ動機で見送った。バイグラム方式は形態素解析ほど精密ではないが
  （分かち書き単位ではなく機械的な部分文字列一致になる）、辞書更新が要らず実装・検証も単純。
  検索結果の並び順は関連度スコアではなく常に受信日時(`date_header`、FASTフィールドとして
  索引に持たせ`order_by_u64_field`で並べ替える)の新しい順（「検索しても新しい順に並ばない」
  という指摘への対応）。単語間のANDはどの文書がマッチするかの絞り込みにのみ関わり、
  並び順には関与しない。
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
4. **新着メールのインデックス投入は非同期バックグラウンドタスク** — UIをブロックしない。
   実装済み: GUI(`src-tauri`)は起動時に`spawn_background_sync`で専用OSスレッドを立ち上げ、
   設定画面で変更可能な間隔（`db::settings::sync_interval_minutes`、デフォルト5分）で
   全アカウントを自動同期し、結果を`background-sync`イベントでフロントエンドに通知する。
   `use_tls=false`（平文専用）のアカウントも自動同期の対象に含める
   （`allow_plaintext`は`!account.use_tls`から決める。平文専用という選択自体が
   アカウント作成時のユーザーの明示的opt-inなので、それを自動同期でも尊重する）。
   CLIには同等のスケジューラはない（`ferro sync`を`cron`/タスクスケジューラ等の
   外部から定期実行する運用を想定）。

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
- **検索インデックス(Tantivy)はCLI/TUI/GUIの間で同時に1つのプロセスからしか開けない**:
  `SearchIndex::open_or_create`は読み取り専用の用途でも内部で`IndexWriter`を作る
  （`from_index`参照）ため、別のFerroプロセスが同じ`search_index`ディレクトリを
  既に開いていると`LockFailure(LockBusy, ...)`で失敗する。DB(SQLite/WAL)や
  `accounts.toml`と違い、複数プロセスからの同時アクセスを想定していない。
  さらに、プロセスが正常終了せず強制終了された場合（実際にTUIの動作確認中に
  `timeout`コマンドで強制終了させて踏んだ）、ロックファイル
  （`search_indexディレクトリ`内の`*.lock`）が残ったままになり、以後**無関係な
  別のFerroプロセス（例: GUI）まで**同じエラーで起動できなくなることがある。
  生のTantivyエラーだと原因（本当に別プロセスが起動中なのか、単に前回の
  ロックファイルが残っているだけなのか）が区別できず対処のしようがないため、
  `SearchError::WriterLockBusy`に読み替えて両方の可能性と対処法（他のFerro
  プロセスが起動していないか確認する／それでも直らなければ`*.lock`ファイルか
  `search_index`ディレクトリ自体を削除する。SQLite/Maildirから再構築可能な
  派生キャッシュなので安全）を案内するようにしてある。
- **`messages.account_id`は`accounts.id`への外部キーだが`ON DELETE CASCADE`を付けていない**ため、
  `foreign_keys=ON`の下では、メッセージが1件でも残っているアカウントを`db::accounts::delete`
  単体で消そうとすると外部キー制約違反で失敗する（実際に踏んだ罠。アカウント削除機能を
  最初に実装した時点ではメッセージが1件も無かったため気づかなかった）。
  `account_setup::remove`が「そのアカウントの全メッセージ（DB行・Maildirファイル・検索
  インデックス）を先に消してからアカウント行を消す」処理を担うので、アカウント削除は
  必ずこちらを通すこと（`db::accounts::delete`を直接呼ばない）。
- **RETRのパイプライン化**は実サーバー次第で接続を切断されることがありうるため、
  「セッションが切れたら再接続して続きから再開する」仕組みを最初から設計に入れておくとよい
  （UIDL差分方式なら再開しても安全＝既に保存済みのメッセージは再取得されない）。実装済み
  （`ferro_core::sync`のセッションループ、`Pop3Error::ConnectionClosed`）。
- **Tantivyの`IndexWriter`がWindows実機で断続的に死ぬ**: `commit()`が
  `"An error occurred in a thread: 'An index writer was killed...'"`で失敗することがある
  （`ferro bench`で1000〜数千件規模の索引投入を繰り返すと、体感1〜2割の頻度で再現。実データ
  9万件超の環境でも実際に再現し、`fts_indexed_at`未投入が6万件超まで積み上がったことがある）。
  原因はマージ/GCワーカースレッドの異常終了とみられ、アンチウイルス/EDR（Microsoft Defender
  for Endpoint等）のリアルタイムスキャンによるファイルI/O競合が疑わしいが未確定。一度死んだ
  `IndexWriter`はそのDirectoryに対する以後の`add_document`/`commit`が全て同じエラーで
  失敗し続ける。対策として`SearchIndex`は`commit`失敗時に新しい`IndexWriter`を作り直す
  自己修復（`recover_writer`）を持つが、**古いwriterを先にdrop（ロックファイル解放）してから
  新しいwriterを作らないと`LockFailure`になる**点に注意（一度実装を誤り、直後に修正した）。
  ワーカースレッド数が多いほどファイルI/Oの同時発生量が増えて衝突機会も増えると考えられるため、
  `index.writer_with_num_threads(1, ...)`でシングルスレッドに固定している
  （スループットよりも信頼性を優先。それでも根絶はできていない）。
  `reindex_all`/`catch_up_unindexed`/`ferro bench`はバッチ単位でこの自己修復＋指数
  バックオフ付きリトライ（最大8回、合計最大約8.4秒）を行うが、それでも救えないバッチが残る。
  以前はそのバッチ全体を諦めて呼び出し元にエラーを伝播していたため、
  「常に同じ先頭からN件」を返す`list_unindexed`と組み合わさると、そのバッチに永久に
  ブロックされ、後ろにある未投入メッセージへ一生手が届かないという実害があった（実際に
  9万件超のメールボックスで踏んだ）。現在は`reindex.rs`の`index_batch_with_fallback`が
  バッチ全体のcommitが尽きた後1件ずつの投入にフォールバックし、`db::messages::list_unindexed`
  も`after_id`カーソルを取るようになったため、特定のメッセージが投入不能でもその後続には
  前進できる（`src-tauri`の`spawn_initial_search_catchup`/`step_search_catchup`がこの
  カーソルを回す）。それでも解決しない場合の実用上の回避策は変わらず、プロセスを再起動する
  （`SearchIndex::open_or_create`を最初からやり直す）か、`search_index`ディレクトリを
  削除して再構築する（完全にSQLite/Maildirから再構築可能な派生キャッシュなので安全）こと。
  なお`SearchIndex::create_in_ram`（テスト専用）はこの問題を踏まない。
- **Linux対応（GUI/TUI共通）で判明した注意点**: 開発機がWindowsのため実機検証はできておらず、
  以下は依存クレートのソース/ドキュメント調査に基づく静的な対応。
  - `src-tauri/capabilities/default.json`の`opener:allow-open-path`が長らく`$APPDATA/ferro/**`
    のみを許可していたが、これは実は一度も機能していなかった（Tauriの`$APPDATA`は
    `dirs::data_dir()`に**バンドル識別子**（`tauri.conf.json`の`identifier`＝
    `"com.ferro-mail.desktop"`）を結合したパスであり、`ferro_core::paths::app_data_dir()`が
    使う`dirs::data_dir().join("ferro")`とは一致しない。実際に添付ファイルの「保存先を開く」等が
    Windowsで動いていたのは、併記していたWindows決め打ちの絶対パスパターンのおかげ）。
    Tauriの`$DATA`変数はバンドル識別子を挟まない素の`dirs::data_dir()`であり、
    こちらが`app_data_dir()`の構成と全プラットフォームで一致するため、`$DATA/ferro/**`を
    追加した（既存の`$APPDATA`/Windows決め打ちパターンは無害なので残置）。
  - `keyring`crateはv4.2.0のデフォルトフィーチャに`zbus-secret-service-keyring-store`
    （純Rust実装のD-Bus Secret Service連携）が既に含まれており、Linuxパスワード保存は
    Cargo.toml変更なしで動く見込み（WSL等でSecret Serviceが無い場合の既知の問題は
    上記の別項を参照）。
  - `font-kit`（フォント一覧取得、`list_installed_fonts`コマンドで使用）はLinuxでは
    Fontconfigバックエンドが自動選択される（Cargoフィーチャでの切り替え不要）が、
    ビルド時に`libfontconfig1-dev`＋pkg-configがシステムに必要（README参照）。
  - `native-tls`はLinux/BSDではOpenSSLに依存するため、システムのOpenSSL開発ヘッダ
    （`libssl-dev`等）を要求しないよう`vendored`フィーチャを有効化し、ソースから
    ビルド・静的リンクする方針にした（Windows/macOSは各OS標準機構を使うため無関係）。
    代わりにビルド機にCコンパイラと`perl`が必要になるが、Tauri/GTK系の他のネイティブ
    依存と同程度の要件なので追加負担にはならない見込み。
  - `rusqlite`は元々`bundled`フィーチャでシステムSQLiteに依存しないため変更不要。

## 未決定・要検討事項

- 上記のTantivy/IndexWriter信頼性問題への根本対応（現状はバッチ単位リトライ＋
  自己修復で緩和のみ。真因はWindows実機でしか再現しておらず未特定）
- Linux対応は開発機がWindowsのため実機でのビルド・動作確認ができていない
  （上記の対応は静的なコード/依存クレート調査のみ）。実機での検証待ち
- CJKバイグラムトークナイザは実データでの検索体感（ノイズヒットの多さ等）を見て、
  必要なら形態素解析への切替を再検討する余地あり
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
