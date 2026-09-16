-- 保持期間切れで完全削除した(`message_actions::purge_expired_batch`)メッセージの
-- UIDLを記録する墓標テーブル。Ferroの同期はUIDL差分方式で「DBに無いUIDL=新着」
-- と判定するが、POP3にDELEを送らずサーバー側にメールを残す設計（CLAUDE.md参照）
-- のため、messagesテーブルの行を消しただけでは次回syncが「未取得」と誤判定し、
-- 保持期間切れのメールを際限なく再取得しては次のクリーンアップで再度消す、という
-- 無駄な往復が発生してしまう（「今すぐ整理する」実行後のsyncで件数が異常に多い、
-- として実際に踏んだ）。
--
-- accounts.idへのON DELETE CASCADEは、このテーブルが実データを一切持たない
-- 純粋な目印であるため付けている（messagesテーブルに意図的にCASCADEを
-- 付けていないのとは事情が異なる。account_setup::removeがアカウント削除時に
-- messages/Maildir/検索インデックスを個別に掃除する必要があるのに対し、
-- こちらは単にuidl文字列の記録が不要になるだけで、消し忘れても実害はない
-- ただの掃除の手間を省くためのCASCADE）。
CREATE TABLE purged_uidls (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    uidl        TEXT NOT NULL,
    purged_at   INTEGER NOT NULL,
    PRIMARY KEY (account_id, uidl)
);
