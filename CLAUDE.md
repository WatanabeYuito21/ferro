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
  キーセットページネーション）、生メール保存（v0.0.39まではMaildir、v0.0.40からは別のSQLiteファイル。
  下記「生メール保存」参照）、Tantivy全文検索（索引投入・
  検索・全件再構築、日本語対応のCJKバイグラムトークナイザ自前実装）、アカウント作成・削除
  （keyring連携、失敗時ロールバック）、アカウント設定ファイル（`accounts.toml`）との同期
  （`account_config::reconcile`）、メール本文/添付パース（`mail-parser`ベース）
- `ferro-cli`: `account add/list/remove/reload-config`, `sync`, `list`, `search`, `reindex`, `bench`
  （開発用のスループット/レイテンシ計測、`$TMPDIR/ferro-bench`に専用データを生成し本番データには
  触れない）、`read`/`flag`/`delete`（既読・フラグ・論理削除）、`label list`（一覧表示のみ。
  作成/削除はGUI限定）、`label-rule add/list/remove/apply`（受信時の自動ラベル付けルール）
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
  アクセントカラー、フォント、文字サイズ。`ferro_core::settings::Settings`に
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
  **重大なバグ（v0.0.10で修正）**: `purge_expired_batch`はDB行・Maildir・
  検索インデックスからは消すが、POP3サーバー側にはDELEを送らない設計
  （Ferroは受信専用で、`dele`コマンド自体は`pop3::client`に実装済みだが
  同期経路からは一度も呼んでいない）のため、消したはずのメールがサーバーには
  まだ残っている。これに気づかず`messages`テーブルの行だけを消していたため、
  「今すぐ整理する」実行直後にsyncすると、UIDL差分計算（`messages::exists_by_uidl`）が
  「DBに無い＝未取得の新着」と誤判定し、たった今消したはずのメールを毎回
  再取得しては次の保持期間クリーンアップでまた消す、という無駄な往復が
  発生していた（「整理する実行後Syncすると件数が多すぎる」として実際に踏んだ）。
  `purged_uidls`テーブル（migration 0007。account_id+uidlの墓標。実データを
  持たないためaccounts.idへの外部キーは`ON DELETE CASCADE`を付けている。
  `messages`テーブルに意図的にCASCADEを付けていないのとは事情が異なる）を
  追加し、`purge_expired_batch`がDB行を消すのと同時にそのuidlを記録するように
  した。syncの新着判定（通常のUIDL差分経路の`pending`計算、UIDL非対応
  フォールバック経路の`store_fetched_message`の両方）は`messages::exists_by_uidl`に
  加えて`purged_uidls::is_purged`もチェックし、過去に保持期間切れで消したuidlは
  二度と新着扱いしない。
- **サーバー側メールの期間超過削除**（`Settings::server_delete_days`、v0.0.13）:
  「サーバー側のメールが溜まりすぎる」という要望で追加。上記v0.0.10の記述にある
  「同期経路からDELEを送らない」は、この設定が0（既定）の間だけ成り立つ。
  `sync_account_with_limit`の`server_delete_before`（`Settings::server_delete_cutoff`が
  unix秒で計算）がSomeのとき、UIDL差分経路でサーバーのUIDL一覧のうち
  (a)ローカルに行とMaildirファイルがあり`date_header`が古いもの、
  (b)`purged_uidls`の墓標があるもの（保持期間で消した古いメール）を同一セッションの
  msg_numでDELEしてからQUITする。DELEはQUITで初めて確定するためQUIT失敗時は0件扱いにし
  （切断されても次回syncが再試行するのでべき等）、取得対象(pending)が空でも削除とQUITは走る。
  1セッション500件まで。**UIDL非対応のフォールバック経路は対象外**（RETR前にmsg_numと
  uidl（内容ハッシュ）の対応が取れず、安全に特定できないため）。`retention_days`とは独立。
  Dateヘッダーが古いメールは受信直後でも次回syncで削除対象になる（`date_header`基準）。
- **受信時の自動ラベル付けルール**（`ferro_core::label_rules`/`message_filter`、v0.0.11。
  v0.0.35で既読にする等の動作も付けられるようになった。下記「受信ルールの動作」参照）:
  「詳細検索機能を付けて、その条件に当てはまるものは受信時に自動でラベル付けできる
  ようにしたい」という要望を受けて追加した。差出人/件名/本文への部分文字列一致と
  添付ファイルの有無を条件に、AND結合（Gmailフィルタと同じ。OR相当が欲しい場合は
  同じラベルを指すルールを複数作る運用）でラベルを自動付与する。条件は
  `ferro_core::message_filter::MessageFilter`という共有データ形に切り出してあり、
  将来の詳細検索機能（Tantivyのフィールド指定クエリ）でも同じ形を入力として使う
  想定だが、**判定エンジンはあえて分離した**: ラベルルールは`MessageFilter::matches`
  による大文字小文字無視の完全一致（部分文字列）のみで判定する（Tantivyを介さない）。
  Tantivyのバイグラム＋前方一致は検索の"あいまいさ"を許容する設計であり、もし
  ラベルルールの判定にもそれを使うと、sync時にその場で1件だけ判定する経路と
  「今すぐ適用」による既存メールへの一括バックフィル（`label_rules::apply_all`）とで
  判定結果がズレて「新着では付いたのに今すぐ適用では付かない」という食い違いを
  生みかねない（CLAUDE.mdの他の箇所に記録されている過去の実害と同種の罠）。
  完全一致のみに統一することで両経路が構造的に一致する。
  ルールは`label_rules.toml`（`accounts.toml`/`color_rules.toml`と同じくTOMLが
  正の情報源で、DBテーブルへのreconcileは無い）に保存し、ラベルは`labels.name`
  （migration 0003でUNIQUE制約済み）を自然キーとして参照する。ラベルがリネーム/
  削除されて名前が解決できなくなったルールは、`accounts.toml`のアカウントと違い
  自動削除も警告もせず、単に何も付与しないルールとして静かにスキップする
  （`db::labels::find_by_name`が`None`を返すだけなので実害が無いため）。
  `label_rules::apply_all`（「今すぐ適用」ボタン、`reindex_all`と同じ
  `messages::list_all_for_reindex`のidカーソルでバッチ処理）は、差出人・件名・
  添付有無はDB行だけで判定できるため、どのルールも本文条件(`body_contains`)を
  指定していなければMaildirを一切読まない（1件でも本文条件があれば、そのバッチ
  だけMaildirから読み直す）。sync時の自動判定（`sync::store_fetched_message`）は
  RETR直後の`parse_full`結果をそのまま使うため追加のI/Oは発生しない。
- **設定ファイルへの移行**（`ferro_core::settings`、v0.0.7）: 上記のUI設定は
  当初SQLiteの`settings`テーブル（key/value）に保存していたが、
  「設定ファイルになっていない他の設定も設定ファイルに起こしてほしい」
  という要望を受け、`accounts.toml`/`color_rules.toml`と同じくTOMLファイル
  （`settings.toml`）ベースに移行した。`color_rules.toml`と同じくパスワード等の
  秘匿情報を含まない純粋な表示・動作設定なのでDBテーブルへのreconcileは無く、
  GUI/TUIがこのファイルを直接読み書きする。TOMLとして型が妥当でも意味的に
  不正な値（未知のtheme文字列、負のretention_days等）は`Settings::normalize`が
  安全な既定値へ補正する（旧DB実装のkeyごとのフォールバックと同じ考え方）が、
  構文自体が壊れている場合（型不一致等）は`color_rules::load`と同じく
  `SettingsError::Parse`として伝播する。設定画面のように利用者へエラーを
  見せられる箇所は`load`をそのまま使い、バックグラウンド同期・保持期間
  クリーンアップ等エラーを見せる手段が無い箇所は`load_or_default`
  （読み込み失敗時は安全側の既定値にフォールバックし、処理自体は止めない）を使う。
  既存ユーザーが設定を失わないよう、`migrate_from_db_once`が初回起動時に
  旧DBテーブルの内容を新しい設定ファイルへ一度だけ引き継ぐ（設定ファイルの
  存在自体を「移行済み」の目印にする）。旧`settings`テーブル自体は当面
  DBスキーマに残すが、以後この移行関数以外からは読み書きしない
  （ドロップは将来の掃除タイミングで検討）。
- **バージョン表示**（v0.0.8）: CLI/TUI/GUIそれぞれの実行ファイルから、
  アプリ側でバージョンを確認できるようにした。ワークスペース全体が
  `version.workspace = true`で単一バージョンを共有しているため、各バイナリの
  `env!("CARGO_PKG_VERSION")`（コンパイル時にそのクレート自身のCargo.tomlから
  埋め込まれる）を参照するだけで揃う。`ferro-cli`は`#[command(version)]`で
  clap標準の`--version`/`-V`を有効化。`ferro-tui`はclapを新たに依存に足す
  ほどではないため、`main()`冒頭で`std::env::args()`を素朴に見て
  `--version`/`-V`を検出する簡易実装＋設定画面にも表示する。GUIは
  Tauri組み込みの`app`プラグイン（`getVersion()`）を使うと権限設定
  （capabilities）が別途要るため、他の`*_config_path`コマンドと同じ
  「自前コマンドでビルド時定数を返すだけ」パターン（`app_version`コマンド）
  に揃え、設定画面の見出し横に表示する。
- **診断ログ**（`ferro_core::logging`）: v0.0.2をレンタルサーバー宛のアカウントで
  試した第三者から"connection closed by server"の報告を受けたが、それまで
  Ferroは同期エラーをGUIの一時的なトースト通知（閉じたら消える）やCLI/TUIの
  標準エラー出力にしか出しておらず、後から状況を再現する手段が無かった。
  そこで`<app_data_dir>/ferro.log`に人が読める1行ログを追記するだけの
  最小限の仕組みを追加した（tracing/log crateは個人利用規模には過剰と判断し
  導入していない）。`ferro_core::sync`内の切断検出箇所（ハンドシェイク段階・
  RETRパイプライン中の両方）に仕込んであるため、GUI/CLI/TUIどれから同期しても
  自動的にログされる（呼び出し側の変更は不要）。パスワード等は絶対に書かない。
  GUIの設定画面「診断」セクションから`log_file_path`コマンド経由でこのファイルを
  開けるようにしてある（`accounts_config_path`と同じ`openPath`パターン）。
  実際にこのログで、レンタルサーバー宛のアカウントが「ハンドシェイク段階で
  6回連続・3秒間隔で切断される」ことが判明したが、当初は"handshake"としか
  出しておらずCONNECT/USER/PASS/UIDLのどこで切れているかまでは分からなかった
  ため、`try_or_reconnect!`マクロに段階名（`"CONNECT"`/`"USER"`/`"PASS"`/`"UIDL"`）
  を渡すよう変更し、どのPOP3コマンドの直後に切断されたかがログから直接
  分かるようにした。この段階別ログにより、そのアカウントは**UIDLコマンドの
  直後だけ**必ず切断されることが判明した（CONNECT/USER/PASSは毎回成功）。
  UIDLはPOP3の必須コマンドではなく、対応していないサーバーが実在する
  （本来`-ERR`を返すべきところを、このサーバーは接続切断で非対応を表現して
  いるとみられる）。
- **UIDL非対応サーバーへのフォールバック**（`Account::uidl_supported`/
  `ferro_core::sync`）: Ferroの同期は「UIDL差分方式」（`messages.uidl`を
  キーに前回との差分を取る）を設計の根幹に置いているため、UIDL非対応
  サーバーには本来同期できない。`accounts.uidl_supported`（`NULL`=未判定/
  `1`=対応/`0`=非対応。マイグレーション0006）を追加し、`sync_account_with_limit`が
  UIDLを送って接続切断（`is_disconnect`）を検出したら`0`を記録、以後
  そのアカウントでは二度とUIDLを試さず、`LIST`＋`RETR`した内容のFNV-1a
  ハッシュ（`content_uidl`、`"ferro-hash-v1-"`接頭辞）を代替uidlとして使う
  `fetch_and_store_by_hash`経路に切り替える。この経路は`messages::insert_new`の
  `INSERT OR IGNORE`（`account_id`+`uidl`のユニーク制約）に頼って重複排除する
  ため、UIDLで事前に絞り込めない代わりに**同期のたびに毎回全件RETRし直す**
  （通信量は増えるが、実装がシンプルで済む。UIDL非対応サーバーは小規模な
  アラート用メールボックス等での利用を想定しており実用上許容できると判断）。
  `SyncSummary::fetched`（新規保存できた件数）と`remaining`（未処理件数）の
  意味を保つため、`fetch_and_store_by_hash`は「新規保存できた件数」と
  「RETRを試みた件数」を別々に返す（既知の内容を再RETRした場合は前者に
  数えない）。UIDL対応判定はDBに永続化されるため、アプリを再起動しても
  次回以降は最初からフォールバック経路に入る（再判定はしない）。
  なおv0.0.5でこのフォールバックを入れた後の再テストでは、今度は**LISTでも
  同じパターン（3コマンド目で必ず切断）**が再現し、UIDL固有の問題ではなく
  認証成功後のほぼ全コマンドを拒否している可能性が出てきた。この短時間に
  同一アカウントへ繰り返し接続を試みていたため、サーバー側／手前の
  ファイアウォールが異常な接続パターンとして防御的にブロックし始めている
  懸念があり、それ以上の推測ベースの修正は一旦止め、ユーザーと相談の上
  「しばらく間を置いてから再テスト」する方針にした。あわせて、再接続前に
  待機しない実装（短時間に6回連続接続を試みる格好になっていた）自体が
  行儀の悪いクライアント動作だったため、`RECONNECT_BASE_DELAY`
  （試行回数に比例する簡易的な指数バックオフ、`search::with_commit_retry`と
  同じ考え方）を全ての再接続箇所に追加した（本番2秒刻み、テストビルドは
  1ミリ秒刻みにして試験時間を短く保つ）。
  **重大なバグ（v0.0.9で修正）**: `fetch_and_store_by_hash`が使う共通ヘルパー
  `store_fetched_message`は、`messages::insert_new`のINSERT OR IGNOREで
  DB側の重複排除はできていたが、**Maildirへのファイル書き込み自体は
  重複チェックの前に無条件で実行していた**。通常のUIDL差分経路
  （`fetch_and_store`）では呼び出し側が`pending`を事前に未取得分だけへ
  絞り込んでいるため実害が無かったが、UIDL非対応フォールバック経路は
  RETR前に重複かどうか分からず全件RETRする設計のため、**UIDL非対応
  アカウントは同期のたびに（デフォルト5分ごと）メールボックス全体を
  ディスクへ書き直し続けていた**。ユーザーがタスクマネージャーの
  I/Oバイト数の異常な多さ・ファンの高回転として実際に踏んだ。
  `store_fetched_message`の先頭で`messages::exists_by_uidl`をチェックし、
  既知のuidlならMaildir書き込み自体をスキップするよう修正した
  （`sync_fallback_does_not_rewrite_already_stored_messages_to_disk`
  テストで、ファイルの`mtime`が2回目のsyncで変化しないことを確認している。
  実際に修正前のコードに戻すとこのテストが失敗することも確認済み）。
- `ferro-tui`: `ratatui`+`crossterm`によるTUI版（neomutt的な使い方を想定。
  CLAUDE.md冒頭の「既存のneomutt」への言及どおり）。GUIと同じ`ferro-core`を
  土台にし、同じDB/Maildir/検索インデックス/`accounts.toml`/`color_rules.toml`を
  共有する。3ペイン構成（フォルダ/ラベル｜メッセージ一覧｜本文）、既読/スター/
  アーカイブ/削除/スヌーズ、検索（`/`キー、Tantivyへの都度クエリで即時反映）、
  全アカウント手動同期を実装済み。バックグラウンドスレッド（同期・検索
  キャッチアップ・保持期間クリーンアップ）は`src-tauri`の対応する関数の移植
  （Tauriの`AppHandle::emit`の代わりに`std::sync::mpsc`でメインループへ通知する。
  `ferro-tui/src/background.rs`参照）。設定画面（`ferro-tui/src/settings_screen.rs`、v0.0.32で
  編集に対応）は項目（`Item`）の一覧をカーソルで選び、トグル/選択肢はその場で、数値・文字列・
  色分けルール/アカウントの追加は中央の入力フォーム（`Form`）で変更し、`settings.toml`/
  `color_rules.toml`/`accounts.toml`へすぐ保存する（GUIと同じファイル・同じ`ferro_core`の関数）。
  GUI専用の設定（テーマ・通知等）も同じファイルなのでTUIから変えられる。アカウント削除は
  メールも消えるので`y`の確認（`Confirm`）を挟む。ここでのエラーは`?`で返さずステータス行に出す
  （`keymap::handle_key`のエラーはTUIごと終了させるため）。フォーム・確認が出ている間は
  `keymap`が全キーをそちらへ渡す（`q`での終了等を入力中の文字として飲み込まないように）。
  テストは利用者の本物の設定ファイルを書き換えないよう、描画・カーソル移動・フォームの開閉だけ。
- **GUIのキーボード操作**（v0.0.27）: `App.svelte`の`handleKeydown`（`<svelte:window>`）に
  まとめ、一覧は`KeyboardHelp.svelte`（`?`で表示）。両方を揃えること。入力欄にフォーカスが
  ある間はEsc（入力欄から抜ける）以外は無視、ボタンにフォーカスがある時のEnterは本来の動作に
  任せる。本文に対する操作（`e`/`s`/`u`/`#`/`b`/`l`）は`MessageDetail`が`export`している関数を
  `bind:this`経由で呼ぶ（ボタンと同じ経路にして、本文側の表示状態とずれないようにする）。
  あわせて、1件の操作のたびに一覧を最初のページから作り直していた（スクロール位置が失われ、
  キーボードで次々に処理できない）のをやめ、`MessageDetail`は`onChanged(id, patch)`で
  変わった項目だけを通知し、`MessageList.applyChange`/`messageView.js`の`applyPatch`が
  その行に当てる。表示中のフォルダ/ラベルの条件（`belongsToView`。`db::messages`の
  `list_by_folder`/`list_by_label`のWHERE句と同じ判定。変えるときは両方を揃える）から
  外れた行は取り除き、次の行（末尾なら前の行）を開く。削除・スヌーズ後に本文を閉じていた
  動作も、この「次へ進む」に置き換えた。一覧の再読み込み（`messageListRefreshToken`）は
  「今すぐ適用」のような一括変更でだけ使う。
- **複数選択と一括操作**（v0.0.28、GUIのみ）: 開いているメッセージ(`selectedMessageId`)とは
  別に、チェック済みのidの集合(`checkedIds`)をApp.svelteが持つ。クリックの解釈（修飾キー無しは
  選択解除して開く、Ctrl/⌘は1件ずつ、Shiftは直前にクリックした行からの範囲）は
  `messageView.js`の`clickSelection`に集約し、通常の一覧(`MessageList`)と検索結果の両方で使う。
  操作は`bulk_update`コマンド→`message_actions::apply_bulk`（DBの変更は1トランザクション、
  削除時の検索インデックスからの除去はcommit 1回）。結果は1件の操作と同じく`applyPatch`で
  各行に当て、開いていた本文が対象に含まれていたら閉じる（本文側の表示が古くなるため）。
  `bulk_update`の`action`はserdeの内部タグ付きenum（`{ kind: "label", label_id, assigned }`）で、
  Tauriはコマンド引数のトップレベルしかcamelCase→snake_caseに変換しないので、中のフィールドは
  フロント側でもsnake_caseで書く。
- **詳細検索**（v0.0.29）: 検索欄の文字列を`ferro_core::search::query::parse`が全文検索の語
  （`TextTerm`。フィールド指定・否定付き）と絞り込み条件（日付・添付・既読・スター・フォルダ・
  ラベル）に分解し、`message_search::search`が実行する。全文検索の語と日付はTantivy
  （`SearchIndex::search_terms`。日付は`date_header`のFASTフィールドへの`RangeQuery`）、
  それ以外はSQLite（`filter_in_db`）。添付・既読等を索引に持たせなかったのは、既読にする
  たびに索引を書き換えることになるため。両方ある場合は索引から新しい順に`CANDIDATE_LIMIT`
  （5000）件取ってSQLで絞るので、語の一致が非常に多く条件に合うものが古い方にしか無いと
  取りこぼす（日付は索引側で絞れるので、期間指定で回避できる。README参照）。語が無ければ
  SQLiteだけで探す。その場合に該当の少ない条件（`has:attachment`等）で`date_header`順の
  インデックスを末尾まで舐めないよう、migration 0009で部分インデックスを足し、
  フラグの条件は`?`ではなくリテラルで書いている（SQLiteは部分インデックスの条件を
  バインド変数から推論しない）。解釈できない`キー:値`は普通の検索語に戻す（入力が黙って
  捨てられないように）。`after:`/`before:`の日付はGUIでは利用者のタイムゾーン
  （`search_messages`の`utc_offset_seconds`）、CLI/TUIはUTCの0時で区切る（chrono等を
  足さずにローカルのオフセットを得る手段が無いため）。GUIの「詳細」フォーム
  （`AdvancedSearch.svelte`）は書式の文字列を組み立てて検索欄に入れるだけで、検索の経路は同じ。
- **スレッド表示**（`ferro_core::threads`、v0.0.30）: `messages.thread_id`（0001から列だけあった）に
  スレッドで共通の値（そのスレッドで最初に受信したメッセージのid）を入れる。sync時
  （`store_fetched_message`）に`References`→`In-Reply-To`のMessage-IDを根に近い方から探し、
  同じアカウントの受信済みメッセージが見つかればそのスレッド、無ければ自分のid。件名だけでは
  まとめない（同じ件名の別件が大量にあるアラートメールを1スレッドにしてしまうため）。返信が
  元のメールより先に届いた場合は繋ぎ直さない（返信側の`References`を保存していないため）。
  0010以前の分は`threads::backfill_batch`をGUI起動時の`spawn_thread_backfill`が専用接続で
  500件ずつ回して埋める（Maildirのヘッダーを読み直す。実データ7.2万件で初回は約11分＝
  ほぼファイルを初めて読むI/Oの時間。`reindex`の初回投入と同じ遅さ）。
  `idx_messages_thread_pending`（`thread_id IS NULL`の部分インデックス）は埋め終わると空になるので、
  以後の起動では残りの確認が一瞬で済む。TUI/CLIは後付けをしない（表示もしないため。
  新着はどこから同期してもsync時に判定される）。GUIは一覧の各行に件数（`thread_info`。
  ページ単位で1クエリ）、本文表示に同じスレッドのメッセージ一覧（`list_thread`）を出す。
  一覧をスレッド単位に畳む表示はしていない（keysetページネーションのまま畳むと、ページを
  またいだスレッドや、畳んだ行への一括操作・キーボード操作の意味が曖昧になるため）。
  なお開発者の実データ（主にアラートメール）には返信を含むメールが無く、2件以上の
  スレッドは0件だった。
- **新着通知とシステムトレイ**（v0.0.31、GUIのみ）: `tauri-plugin-notification`と
  Tauri本体の`tray-icon`フィーチャ。`run_background_sync_once`が同期前に
  `new_mail::latest_id`（最大id）を覚え、全アカウントの同期後に`new_mail::since`で
  それより後の新着を数えて1回だけ通知する（`SyncSummary`は件数しか持たず件名が分からない
  ため）。`Settings::notify_labels`が空でなければ、そのどれかのラベルが付いた新着だけ
  （受信時の自動ラベル付けルールで付く）。ウィンドウが表示中かつフォーカスがある間は
  通知しない（画面上の新着バナーで足りる）。手動Syncでは通知しない（自分で押しているため）。
  トレイは左クリックで表示、メニューに「Ferroを開く/今すぐ同期/終了」。
  `Settings::close_to_tray`がオンなら`on_window_event`の`CloseRequested`で閉じずに隠す
  （閉じる時点で設定を読み直す）。手動Syncとの競合回避（`manual_syncs`）で同期を
  打ち切った場合も、それまでに受信した分は通知する（以前は`return`で抜けていたのを`break`に変えた）。
  Linuxのトレイは`libayatana-appindicator3`が要る（CIでは導入済み）。
- **.eml書き出し**（`ferro_core::mail::export`、v0.0.26）: Maildirのファイル自体が既に
  RFC 5322の生メールなので、変換はせずバイト列をそのまま書き出す。GUIは本文表示の
  「.emlとして保存…」（`save_message_eml`コマンド＋`tauri-plugin-dialog`の保存ダイアログ）、
  CLIは`ferro export <id> [-o path]`。既定のファイル名は`suggested_eml_filename`が件名から作る
  （Windowsで使えない文字・制御文字は`_`、予約デバイス名や空になる件名は`message-<id>.eml`。
  別OSへ持っていけるよう全OSでWindowsの規則に揃える）。
- **ディスクの最適化**（`ferro_core::maintenance`、v0.0.39）: 検索インデックスのセグメントを
  1つにマージ（`SearchIndex::optimize`。削除済み文書の領域もここで回収）してからGCし、DBは
  `VACUUM`＋`wal_checkpoint(TRUNCATE)`。`ANALYZE`/`PRAGMA optimize`はしない（統計が変わると
  `EXPLAIN QUERY PLAN`で確かめたインデックスの選び方が変わりうるため）。再構築と同時に走らない
  よう`begin_rebuild`の印を借りる（その間は起動時の拾い直しも止まる）。マージも新しいセグメント
  ファイルを作るので、失敗したら`commit`と同じくwriterを作り直してエラーを返す。GUIは
  `optimize_storage`（`write_conn`を握るので実行中は同期が待つ）、TUIは設定画面、CLIは
  `ferro optimize`。どれも一時的に同じくらいの空き容量が要る。実データのコピー（DB 136MB・
  索引78ファイル）で、DB 136→93MB、索引 78→7ファイル、約2秒。tantivyの掃除は「生きている
  `SegmentMeta`」と「readerが開いているセグメント」のファイルを消さないので、マージ後は
  `SegmentMeta`を手放し、readerを読み直してから`garbage_collect_files`を呼ぶ。なお索引を別の
  場所へコピーして試すときは`.managed.json`も一緒に（`cp *`はドットファイルを拾わない）。
  これが無いとtantivyは既存のファイルを自分の管理下と見なさず、掃除で一切消さない。
- **未読数のバッジ**（v0.0.38、GUIのみ。`src-tauri/src/badge.rs`）: 受信箱の未読数
  （`messages::count_unread_inbox`）を、トレイアイコン（既定のウィンドウアイコンの右下に赤い札を
  重ねた画像＋ツールチップ）とタスクバー（Windowsは`set_overlay_icon`、それ以外は
  `set_badge_count`。WindowsはOSのバッジAPIが無いため）に出す。画像処理のクレートは足さず、
  3×5ドットの数字を拡大して描くだけ。更新はフロントの`refreshFolderCounts`のたび
  （`refresh_unread_badge`）、バックグラウンド同期の後、設定の保存時。未読数のCOUNTは
  `idx_messages_unread_inbox`（migration 0013。受信箱の条件込みの部分インデックス）だけで数える
  （0009の`idx_messages_unread`では行を読みに行くため実データで0.3秒→0.003秒）。受信箱の条件を
  変えるときは`append_folder_condition`・このクエリ・インデックスの3つを揃えること。
  `Settings::show_unread_badge`（既定オン）でオフにできる。
- **すべて既読にする**（v0.0.43）: `messages::mark_folder_read`（条件は`append_folder_condition`。
  受信箱は`idx_messages_unread_inbox`で未読だけを辿ることをテストの`EXPLAIN QUERY PLAN`で確認）と
  `mark_label_read`（`list_by_label`と同じくゴミ箱を除く）。UPDATE 1文なので件数が多くても速い。
  GUIは検索欄の横の「すべて既読」か`R`で確認を出してから`mark_all_read`コマンド（検索中は表示中の
  検索結果のidを`bulk_update`へ）。一覧は作り直さず、読み込み済みの行だけ
  `MessageList.markAllReadLocally`で既読の表示にする。TUIは`R`→`y`、CLIは`ferro read --all`
  （受信箱のみ。CLIにはフォルダの概念が無いため）。
- **選択と本文を開くことの分離**（v0.0.42）: `Settings::open_on_select`（既定オン＝従来どおり
  選ぶと開く）。GUIの`App.svelte`は選んでいる行`selectedMessageId`と本文を開いているか
  `detailOpen`を分け、選択は`selectMessage`、開くのは`openMessage`（Enter/o/ダブルクリック/
  スレッド一覧）、閉じるのは`closeDetail`を通す（直接`selectedMessageId`へ代入しないこと）。
  オフのときは選択を動かすと本文を閉じる（本文はいつも選んでいる行のもの、という不変条件を
  保つため。キーボードの`e`/`s`/`u`等が本文と別の行に効く食い違いを作らない）。本文を開かずに
  選んでいる行への`e`/`s`/`u`/`#`は`runOnSelected`（`bulk_update`に1件だけ渡す）。行のEnterは
  フォーカスのある行ではなく選んでいる行を開く（`j`/`k`ではフォーカスが動かないため）。
  TUIは`App::cursor_changed`がカーソル移動後に本文を読むか閉じるかを決め、`Enter`で開く。
  **v0.0.44で修正**: `App.svelte`の`settings`は設定画面の「← メッセージへ戻る」（`onBack`）で
  しか読み直しておらず、サイドバーのフォルダ等から設定画面を離れると再起動まで古い値のままで、
  「トグルをオフにしたのに反映されない」として実際に踏んだ（`mark_read_delay`等も同じ）。
  `SettingsView`の保存は共通の`save`を通し、保存できたら`onSaved(next)`で`App.svelte`の
  `settings`を置き換える。App側で参照する設定を足すときもこの経路で反映される。
- **サイドバーの折りたたみ**（v0.0.41）: GUIはサイドバー上端の「«」か`[`キーで、開く「»」と
  設定の⚙だけの細い帯にする。状態は設定ファイルではなくlocalStorage（`ferro.sidebarCollapsed`。
  この端末だけの見た目の好みのため。更新確認の時刻と同じ扱い）。`[`は設定画面でも効く。
  TUIは`[`でフォルダのペインを隠し（`App::show_sidebar`、保存はしない）、その間はTabで
  フォルダのペインへ移らない。
- **検索語のハイライト**（`ferro_core::search::highlight`、v0.0.37）: 強調する語は
  `query::parse`の全文検索の語のうち否定でないもの（`terms`。GUIは`highlight_terms`コマンド）。
  一致の規則は検索と同じく大文字小文字を無視した部分一致で、同じ位置からなら長い語を優先
  （Rustの`ranges`＝TUI、JSの`frontend/src/lib/highlight.js`＝GUI。変えるときは両方を揃える）。
  検索そのものはバイグラムのフレーズ一致なので、記号の扱い等で「検索には当たったが強調されない」
  ことはありうる（強調は表示の補助なので許容）。GUIは本文の最初の一致へ`scrollIntoView`する。
- **保存した検索**（`ferro_core::saved_searches`、v0.0.36）: 検索欄の文字列に名前を付けて
  `saved_searches.toml`に保存する（`color_rules.toml`と同じくファイルが正の情報源。同じ名前で
  追加すると条件を置き換える）。結果は保存せず、開くたびに普通の検索として実行する。
  サイドバーの未読数は`unread_count`（条件に`is:unread`を足して`message_search::search`、
  `UNREAD_COUNT_CAP`=1000まで）で、GUIは`refreshFolderCounts`のたびに`list_saved_searches`で
  数え直す。検索結果の上限は50→200件にした。GUIでサイドバーのフォルダ/ラベルを押したら
  検索を解除するようにした（保存した検索から普通の一覧へ戻れるように）。
  TUIでは、検索欄で`Enter`を押しても入力モード（`search_mode`）のままで、j/k等が検索欄に
  入ってしまい結果の中を移動できなかった不具合も直した（`confirm_search`で入力から抜けて
  結果は残す。結果を出している間の`Esc`で元の一覧へ）。
- **受信ルールの動作**（v0.0.35）: 自動ラベル付けルール（`LabelRule`）に`RuleActions`
  （`mark_read`/`archive`/`star`/`skip_notification`）を足し、`label_name`は空でもよくした
  （空ならラベルは付けず動作だけ）。ラベルも動作も無いルールは`add`が`NoEffect`で拒む。
  TOMLは`#[serde(flatten)]`で同じ表に並べ、立てた動作だけを書く（0.0.34以前のファイルも
  そのまま読める）。判定は従来どおり`matching_rules`の完全一致で、sync時と「今すぐ適用」で
  同じ`apply_rules`を通す。動作は立てるだけで外さない（`messages::apply_rule_flags`）。
  「通知しない」は受信時だけ意味があるので、`messages.notify_muted`（migration 0012）に印を
  残し、`new_mail::since`が数えない。あわせて既読のメールも通知しないようにした
  （ルールで既読にした定期通知が通知されないように）。`apply_all`の戻り値は「何かが変わった
  メッセージの件数」に変わった（以前は付与したラベルの数）。
- **アカウントごとの同期状態**（`db::sync_status`、migration 0011、v0.0.34）: `sync_account_with_limit`は
  中身を`sync_account_inner`に移した薄い包みで、成功・失敗のたびに`account_sync_status`へ記録する
  （どこから同期しても残る。記録に失敗しても同期の結果は変えず`ferro.log`に出すだけ）。失敗は
  `consecutive_failures`を数え上げ、直近の成功の時刻・件数は残す。GUIのバックグラウンド同期で
  keyringからパスワードを読めなかった場合も記録する。表示はGUI設定画面のアカウント一覧
  （`AccountsView.svelte`。`list_accounts`の`sync_status`/`uidl_supported`）、TUI設定画面、
  `ferro account list`（CLI/TUIはタイムゾーンを持たないので「5分前」の相対表示＝`describe`）。
- **ヘッダー・ソース表示**（`ferro_core::mail::source`、v0.0.33）: 本文表示の「本文/ヘッダー/ソース」
  タブ（GUI、`v`キー）とTUIの`H`キー。Maildirの生バイト列のヘッダー部（最初の空行まで）か全体を
  そのまま見せる調査用の表示なので、RFC 2047のエンコードもデコードしない。行ごとにUTF-8として
  読めなければ件名の文字化け対策と同じ順（Shift_JIS→EUC-JP→ISO-2022-JP）で読み直す。
  全体表示は`SOURCE_DISPLAY_LIMIT`（512KB）で行の境目に切り詰める（大きな添付付きのメールで
  画面が固まらないように）。GUIはタブを開いた時に`get_message_source`で取りに行く。
- Windowsインストーラー（MSI/NSIS）のビルドも確認済み。`cargo install tauri-cli --version "^2"`で
  `cargo tauri`コマンドを導入した上で`cargo tauri build`を実行する（WiX/NSISは未導入でも
  tauri-bundlerが自動取得する）。成果物は`target/release/bundle/{msi,nsis}/`
  （実測: MSI約7.7MB、NSIS約4.5MB。Electron系との比較で軽量という当初方針どおり）。
  インストーラーの言語は`tauri.conf.json`の`bundle.windows.wix.language`
  （`["en-US", "ja-JP"]`）と`bundle.windows.nsis.languages`
  （`["English", "Japanese"]`、`displayLanguageSelector: true`）で指定している。
  WiX(MSI)は言語ごとに別ファイルが生成される仕様のため`Ferro_x.y.z_x64_en-US.msi`/
  `Ferro_x.y.z_x64_ja-JP.msi`の2つに分かれるが、NSIS(setup.exe)は1ファイルの中に
  両言語を持ち、インストール開始時に選択ダイアログを出す方式。両方とも
  Tauri/WiX/NSISが標準で持つ言語データを使うだけで、追加の.wxl/.nshファイルは
  用意していない。
- **バージョン更新は3か所を揃える**（v0.0.14で対応）: ルート`Cargo.toml`の
  `workspace.package.version`だけを上げて`src-tauri/tauri.conf.json`の`version`を
  上げ忘れると、タグ・Release名はv0.0.13なのにインストーラー（MSI/NSIS/deb/AppImage）の
  ファイル名が0.0.12のままになる（v0.0.13で実際に踏んだ。インストーラー名は
  `tauri.conf.json`のversionから付くため）。公開済みタグは消さない方針のため0.0.14で
  出し直した。`release.yml`の`check-version`に両者の一致チェックを入れ、ずれていれば
  ビルド前にエラーで止まるようにした。`frontend/package.json`も合わせて更新する運用。
- **手動Syncと自動同期の競合**（v0.0.15で対応）: 手動Sync(`sync_account`、`limit: None`で全件)と
  バックグラウンド同期(`BACKGROUND_SYNC_LIMIT`=200件上限)は同じ`write_conn`ロックを待つため、
  手動Syncが終わった直後に上限付きの自動同期が走り、`background-sync`イベントで手動Syncの
  結果表示（取得件数）が上書きされて「少ない件数しか同期されていない」ように見えた。
  `AppState::manual_syncs`（`ManualSyncGuard`のRAIIカウンタ。ロック待ちの間も数える）が
  0より大きい間は`run_background_sync_once`が次のアカウントの同期を始めず次サイクルに回す。
  フロント側も`manualSyncing`中のアカウントについては`background-sync`イベントを無視する。
- **UIDL非対応フォールバックの重複保存と再開不能（v0.0.16で修正）**: 経路上の
  ゲートウェイ（実際に`X-WatchGuard-Spam-ID`ヘッダー）がRETRのたびに内容を付け替える
  サーバー（`hades.sc2.ne.jp`のアラート用アカウント）では、v1の内容ハッシュ
  （メール全体のFNV-1a）が毎回変わり、**同じメールが別物として数千回ずつ保存された**
  （実データで約2千通のMessage-IDが5万行に膨れていた。「fetchedが毎回27通ずつ
  増える」ように見えていたのもこれ）。さらにそのサーバーは1接続あたり約27通で
  必ず切断するため、フォールバックが毎回先頭から全件RETRし直す設計だと、取得済みの
  先頭部分が1接続の許容量を食い潰して新着に手が届かなかった。対策:
  (1) `content_uidl`をv2（`ferro-hash-v2-`）にし、`Message-ID`があればそれだけから、
  無ければDate/From/Subject+本文のバイト列から作る（取得のたびに変わりうる
  ヘッダーを使わない）。重複判定`is_known_by_content`はv2ハッシュに加え
  `Message-ID`でも既知か見る（v1時代の既存行の再取り込みを避ける。マイグレーション
  0008の`idx_messages_account_message_id`）。(2) 同一sync呼び出し内の再接続では
  処理済みの続きから再開し(`fallback_start`)、新しいsync呼び出しの最初に
  「取得済みの境界」を二分探索（1通RETRして既知か判定）で探してそこから取る
  （メールは到着順で先頭から連続して保存される前提。境界の手前
  `RETR_BATCH_SIZE`件は安全マージンとして取り直す）。(3) 既存の重複は
  `ferro dedupe`（既定は件数表示のみ、`--apply`で削除）で整理する。対象は
  `uidl_supported=0`のアカウントだけ（UIDL対応アカウントでは同じMessage-IDの別
  メッセージが正当に存在しうるうえ、行を消すと次回syncが再取得するため）。
  残す行は最小idで、削除する行の既読/スター/ラベルを引き継ぐ。
  ※ GUI等がTantivyインデックスを開いている間はCLIは起動できない
  （上記ロックの項参照）ため、`ferro dedupe`はGUI/TUIを閉じてから実行する。
- **自動アップデート**（v0.0.17、GUIのみ）: `tauri-plugin-updater`＋`tauri-plugin-process`
  （再起動用）。設定画面の「更新を確認」ボタン、またはメニューバーの
  View > Check for Updates…（v0.0.18。Rust側は`check-for-update`イベントを投げるだけで、
  フロントが設定画面の「アップデート」セクションへ移動してから確認を始める）を押したときだけ
  `check()`→`downloadAndInstall()`→`relaunch()`を行う（起動時の自動確認はしない）。
  進捗状態は画面切り替えで作り直される`SettingsView`ではなく
  `frontend/src/lib/updater.svelte.js`のモジュール状態に持たせている。
  v0.0.20で**1日1回の自動確認**を追加（`Settings::auto_update_check`、既定オン）。
  `startDailyUpdateCheck`が起動10秒後と以後1時間ごとに「前回確認から24時間経ったか」を
  見て、経っていれば`check()`だけ行い、新バージョンがあればメイン画面上部のバナー
  （「今すぐ更新」/「後で」）で知らせる。自動確認ではダウンロード・インストールはしない。
  最終確認時刻はlocalStorage（`ferro.lastUpdateCheck`）。通信エラー時は黙って諦め、
  時刻を更新しないので1時間後に再試行する。「後で」で閉じても、翌日の確認でまだ
  新バージョンがあれば再表示する。更新元は
  `https://github.com/WatanabeYuito21/ferro/releases/latest/download/latest.json`
  （publicリポジトリなので認証不要）。`release.yml`のreleaseジョブが各プラットフォームの
  `.sig`（`createUpdaterArtifacts: true`でビルド時に生成）から`latest.json`を`jq`で組み立てて
  Releaseに添付する。対象はWindows NSIS(`*setup.exe`)とLinux AppImageのみ。
  **ビルドにはGitHub Secretsの`TAURI_SIGNING_PRIVATE_KEY`が必須**（未設定だと`cargo tauri build`が
  失敗する）。`tauri.conf.json`の`plugins.updater.pubkey`は鍵生成後に実際の公開鍵へ
  差し替えること（公開鍵は設定済み）。
  秘密鍵を失うと既存インストールへ更新を配れなくなる。
- **設定の検索**（v0.0.19、GUIのみ）: 設定画面上部の検索欄で項目を絞り込む。各項目の
  見出し・説明文は`SettingsView.svelte`の`R`（項目）/`SECTIONS`（セクション）に一元化し、
  テンプレートはそこから描画する（検索対象と表示文言がずれないように）。画面に出ない
  言い換え・英語名は`keywords`に足す。判定は空白区切りAND・NFKC正規化＋小文字化の部分一致で、
  セクション名＋項目の文言をまとめて見る（「同期 間隔」のように語が別々にあっても当たる）。
  セクション名自体が当たればその中の項目を全て出す。アカウント/色分けルール/自動ラベル付け
  ルールは子コンポーネントに中身を任せているため、セクション単位でだけ絞り込む。
  新しい設定項目を足すときは`R`と`SECTIONS.*.rows`にも登録すること（登録漏れは
  検索語が入っている間だけ表示されなくなる）。
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
- **生メール保存**: v0.0.39まではMaildir形式（1ファイル1メッセージ、ハッシュ分散で
  `cur/xx/yy/<account_id>-<uidl>.eml`）。v0.0.40から、メタデータの`ferro.db`とは別のSQLiteファイル
  `<app_data>/maildir/messages.db`の`raw_messages`表（`account_id`+`uidl`がキー、1通1行のBLOB）に
  保存する（`ferro_core::maildir`。モジュール名・関数名・ディレクトリ名は呼び出し側を変えないため
  そのまま）。理由は実データでの計測: 7.6万通のファイルに対しフォルダが6.6万個できて約330MBを
  占め（中身143MB。4KB単位の確保と1通1フォルダ近いハッシュ分散のため）、初めて読むファイルは
  1通約9ミリ秒（108通/秒。毎回のオープンでウイルス対策の検査が走るとみられる）で、再構築や
  スレッドの後付けのように全件を読む処理が10分以上かかっていた。DBに移した5000通の読み込みは
  0.08秒。`ferro.db`に混ぜなかったのは、一覧・検索が使う方を小さいまま保ち、最適化（VACUUM）や
  バックアップが生メールの分まで重くならないようにするため。当初の「SQLite BLOB案は不採用」
  （`messages.raw_storage_kind`/`raw_blob`列は0001から未使用のまま残っている）を覆した。
  接続は`base_dir`ごとにプロセス内で1本を使い回す（テストビルドは一時ディレクトリを消せるよう毎回
  開き直し、WAL・同期も省く）。既存のファイルは`migrate_all_files`がGUI/TUIの起動時に
  バックグラウンドで200個ずつ移し（DBへの登録をcommitしてからファイルを消すので途中で止めても
  安全。実データで約97通/秒＝初めて読む遅さそのもの）、移し終わるまでは`load`/`exists`が
  ファイルも見る。ファイル名の`%XX`エスケープは`%`自身も含むので元のuidlに戻せる。
  `remove`はファイルの削除もDB接続を握ったまま行い、移行の途中の同じメールと競合しない
  （移行側はファイルの読み込みを接続の外で行い、登録直前にファイルがまだあるか確かめる）。
  消したメールの領域は次のメールが使い回し、ファイル自体は「ディスクを最適化」の`vacuum`で縮める。
  CLIは移行を行わない（移す前のファイルもそのまま読めるので困らない）。
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
   設定画面で変更可能な間隔（`settings::Settings::sync_interval_minutes`、デフォルト5分）で
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
  （実際に`Cargo.lock`で`encoding_rs`が解決されていることを確認済み。有効化自体はできていた。）
  **`full_encoding`を有効にしても直らない文字化けが別途実在する（v0.0.12で対策）**:
  `mail-parser`のヘッダー解析（`parsers/fields/unstructured.rs`の`add_token`、`fields/address.rs`の
  表示名パースも同様）は、RFC2047の`=?charset?B/Q?...?=`エンコード済みの部分だけを
  `full_encoding`のcharsetテーブルで復号する。エンコードされていない生のヘッダーバイト列は
  `String::from_utf8_lossy`で無条件にUTF-8として変換されるため、**RFC2047で包まず生の
  Shift_JIS/EUC-JP/ISO-2022-JPバイト列をヘッダーに直接書いてくる（技術的には仕様違反だが、
  古い日本語のcron/監視アラート系メールサーバーで実在する）送信元だと、件名が
  U+FFFD混じりに文字化けする**（実際に自社のセキュリティ監視アラートメールの件名
  「[T-TOMY01]で悪意のあるSQLもしくは決済系テンプレート等の更新を検知」で踏んだ。
  Shift_JISの2バイト文字はしばしば2バイト目がASCII印字可能域に落ちるため、
  「1バイト目がU+FFFDに、2バイト目がそのままASCII文字として残る」という特徴的な
  壊れ方をする）。`mail-parser`は生ヘッダーの元バイト列を取り出すAPIを公開していない
  （一度lossy変換された時点で情報が失われ復元不能）ため、`ferro_core::mail::parse`側で
  `raw_header_value`により生メールバイト列から該当ヘッダー（現状はSubjectのみ。
  折り返し継続行のunfoldingにも対応）を独自に抜き出し直し、`recover_mojibake_header`が
  Shift_JIS→EUC-JP→ISO-2022-JPの順に`encoding_rs`で再デコードを試みて、変換エラーが
  無くU+FFFDも残らない最初の結果を採用する。`mail-parser`の結果にU+FFFDが含まれない
  （＝正常にデコードできている）大多数のケースでは一切関与しないため、既存の
  ASCII/UTF-8/RFC2047な件名を壊す心配は無い。どのcharsetでも再デコードに失敗した
  場合はmail-parserの元の結果（文字化けしたまま）を諦めてそのまま使う（悪化はさせない）。
  From表示名（`address.rs`側）にも同種の根本原因があり得るが、実際に報告されたのは
  件名のみだったため今回はSubjectに限定して対応した。
- **`keyring`crate（Linux/secret-service）**: WSL等でSecret Service D-Busサービスが動いていないと
  `org.freedesktop.secrets was not provided`のようなエラーになる。また`default`という名前の
  コレクションエイリアスが未設定だと保存時に`no result found`になりうる（通常はログイン時にPAM経由で
  作られるが、WSLのような環境では作られないことがある）。
- **ポート995での接続失敗**: 暗黙的TLS前提でポート995に繋いでも、サーバーが実際にはTLSを話さない
  ケースがある（`wrong version number`エラー）。ポート110で平文接続し`CAPA`応答に`STLS`があるか
  確認してから接続方式を判断する設計にしておくと安全。
- **Tauriの`async`でないコマンドはメインスレッドで動く**: `#[tauri::command] fn ...`
  （`async`無し）はウィンドウのイベントループと同じメインスレッドで実行されるため、中で
  `Mutex`のロック待ちが起きるとアプリ全体が「応答なし」で固まる。v0.0.20まで、検索
  インデックス再構築(`reindex_all`)が`read_conn`を処理の最初から最後まで（大きな
  メールボックスでは数分）握り続け、その間に一覧取得・フォルダ件数・本文表示などの
  同期コマンドがメインスレッドで待たされて、「再構築するとアプリが固まる」として
  実際に踏んだ（v0.0.21で修正）。対策は2つ: (1) `reindex_all`は共有の接続を借りず、
  専用の`db::open`接続で動かす（WALなので`write_conn`側と共存できる）。
  (2) `read_conn`を使う読み取り系コマンドは`#[tauri::command(async)]`にして
  メインスレッドから外す。新しくDBやロックに触るコマンドを足すときは、
  `async`にするか`(async)`属性を付けること。長時間かかる処理で`read_conn`/
  `write_conn`を握りっぱなしにしないこと。
  v0.0.22で再構築の**進捗表示**を追加: `reindex::reindex_all_with_progress`が開始直後と
  1000件のページごとに`ReindexProgress { processed, total }`を通知し（`total`は開始時の
  `messages::count_live`。途中のsyncで増えたら`processed`に合わせて引き上げる）、
  GUIは`reindex-progress`イベント→設定画面のプログレスバー、CLIは端末出力時のみ
  `\r`で同じ行を上書き表示する。GUIの進捗状態は`frontend/src/lib/reindex.svelte.js`の
  モジュール状態（設定画面を離れても続く）。同時実行は`AppState::reindex_running`で
  2本目をエラーにする（互いの`clear`で索引を消し合うため）。
  **再構築が0件のまま止まる（v0.0.23で修正）**: 0.0.22で実際に「0 / 72,062件から進まない」を
  踏んだ。DBの`fts_indexed_at`の記録から追うと、(1)直前の再構築が中断されて索引がほぼ空の
  状態で起動→起動時の整合性チェックが全件の`fts_indexed_at`をNULLに戻し、起動時の拾い直し
  スレッド(`spawn_initial_search_catchup`、500件ずつ)が全件の再投入を開始、(2)その最中に
  再構築ボタンが押され、両スレッドが同じTantivy writerへ並行して投入・commitし、
  `clear`(当時は`delete_all_documents`)も他スレッドの投入途中に走った、(3)約8秒後に
  ファイル書き込みが止まりCPUもほぼ0のまま進まなくなった。直接の原因は特定できていないが
  （GUIのリリースビルドはスタックを取れず、この経路はログも出していなかった）、
  最も整合するのは「writerの作り直し(`recover_writer`)に一度失敗するとスロットが`None`の
  まま固定され、以後`index_message`が`WriterUnavailable`で即失敗→作り直しはcommit失敗時
  しか試みないため二度と復帰せず、両スレッドが`with_commit_retry`の待機を延々と繰り返す」
  という状態。対策: (a) `SearchIndex::begin_rebuild`で再構築中フラグを立て、その間
  `catch_up_unindexed`は何もしない（2本目の再構築もエラー。GUI側の独自フラグは廃止）、
  (b) `clear`を`delete_all_documents`（opstampを巻き戻すため並行投入と相性が悪い）から
  `delete_query(AllQuery)`（通常の削除操作としてopstamp順に効く）に変更、(c) スロットが
  `None`なら次の書き込み時に作り直しを再試行する`writer_slot`、(d) commit失敗・writer
  作り直し・再構築の開始/終了/失敗を`ferro.log`に出す。次に同様の停止が起きたら
  まず`ferro.log`の`search:`/`reindex:`行を見ること。
  **0.0.23でも再発し、真因が判明した（v0.0.24で修正）**: 0.0.23で再構築すると
  `reindex: started`の直後から、`search:`の行を1つも出さずに止まった。実データ
  （7.2万件）を一時ディレクトリの索引へ投入する調査用プログラムで再現し、
  tantivyのワーカースレッドが新しいセグメントファイル（`.pos`）の作成で
  `PermissionDenied`（「アクセスが拒否されました」）を受けて死んでいることが分かった
  （下記「`IndexWriter`がWindows実機で断続的に死ぬ」と同じ現象。発生位置は毎回違う）。
  ワーカーが死ぬと以後の`add_document`は"An index writer was killed"で即失敗するが、
  writerの作り直しは`commit`失敗時にしか行っておらず、`index_message`で失敗した
  バッチはcommitまで到達しないため、二度と作り直されなかった。そのうえ
  `with_commit_retry`（8回）が尽きると1件ずつのフォールバック（1件につき8回、
  約8.4秒）に入り、writerが死んだまま1000件分（2時間超）をログも出さずに待ち続けた。
  CPUがほぼ0で進捗も0のままという症状はこれ。0.0.22で`None`スロットを疑ったのは
  外れで、0.0.23の`writer_slot`は無関係だった。対策: (a) `index_message`で
  `add_document`が失敗したら、その場で`commit`を呼んで死んだワーカーの実際のエラーを
  取り出して`ferro.log`に書き、`recover_writer`で作り直す、(b) 1件ずつの
  フォールバックが3件連続で失敗したら（特定のメールではなくwriter自体が使えない）
  バッチを諦めてエラーを返す（`MAX_CONSECUTIVE_FALLBACK_FAILURES`）、(c) バッチが
  フォールバックに入ったこと・諦めたことも`ferro.log`に出す。回帰テストは
  `.pos`の作成を1回だけ失敗させる`FlakyDirectory`で同じ状況を作る
  （`index_message_recreates_the_writer_after_its_worker_dies`）。
  なお`logging::log_line`はテストビルドでは何もしない（テストがわざと失敗経路を
  踏むたびに、利用者の本物の`ferro.log`へ書き込んでいた）。
  教訓: 「失敗したらリトライ」の経路に、失敗をログへ残さず長時間待つ箇所を作らないこと。
  v0.0.25で設定画面「検索」セクションに**インデックスの状態**を追加した
  （`reindex::index_status`→`get_search_index_status`コマンド。全件数`count_live`・
  投入済み`count_indexed`・索引の文書数`num_docs`・再構築中か）。取りこぼしに
  再構築を押す前に気づけるようにするため。全件COUNTを取るので常時ポーリングはせず、
  設定画面を開いた時・「再確認」・再構築完了時にだけ取り直す
  （`frontend/src/lib/reindex.svelte.js`の`indexStatus`）。
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
  **当初はこの再接続がRETRパイプライン中の切断にしか効かず、CONNECT/USER/PASS/UIDLの
  ハンドシェイク段階での切断は即座にエラーを返していた**（v0.0.1をレンタルサーバー宛の
  アカウントで試した第三者が実際に"connection closed by server"のエラーで踏んだ。
  一部のレンタルサーバー/独自POP3実装はハンドシェイク中でも接続を切ることがあると
  みられる）。`sync_account_with_limit`の`loop`内で`try_or_reconnect!`マクロを使い、
  ハンドシェイク段階の切断も同じ`MAX_RECONNECTS`予算でリトライするように修正した
  （ハンドシェイク段階ではまだ何も取得できていないため、予算を使い切った場合は
  `fetch_and_store`側のような`ended_early`付きの部分成功ではなく素直にエラーを返す）。
- **Tantivyの`IndexWriter`がWindows実機で断続的に死ぬ**: `commit()`が
  `"An error occurred in a thread: 'An index writer was killed...'"`で失敗することがある
  （`ferro bench`で1000〜数千件規模の索引投入を繰り返すと、体感1〜2割の頻度で再現。実データ
  9万件超の環境でも実際に再現し、`fts_indexed_at`未投入が6万件超まで積み上がったことがある）。
  原因はマージ/GCワーカースレッドの異常終了とみられ、アンチウイルス/EDR（Microsoft Defender
  for Endpoint等）のリアルタイムスキャンによるファイルI/O競合が疑わしいが未確定。一度死んだ
  `IndexWriter`はそのDirectoryに対する以後の`add_document`/`commit`が全て同じエラーで
  失敗し続ける。実データでの再現では、直接の原因はワーカーが新しいセグメントファイル
  （`.pos`）を作ろうとして受けた`PermissionDenied`（OSエラー5）だった（v0.0.24。
  上記「再構築が0件のまま止まる」参照）。対策として`SearchIndex`は`commit`失敗時と
  `index_message`の`add_document`失敗時（v0.0.24から）に新しい`IndexWriter`を作り直す
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
- レンタルサーバー宛の「アラート用」アカウント（`hades.sc2.ne.jp:110`）で、
  UIDL非対応フォールバック（v0.0.5）を入れた後もLISTで同じパターンの
  切断が再現している。UIDL固有の問題ではなく認証後のほぼ全コマンドが
  拒否されている可能性があり、繰り返しテストしたことによるサーバー側／
  ファイアウォール側の防御的ブロックの懸念もあるため、しばらく間を置いて
  からの再テスト待ち（`RECONNECT_BASE_DELAY`のドキュメント参照）
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
