-- UIDL非対応サーバー向けフォールバック(`sync::fetch_and_store_by_hash`)の
-- 重複判定は、RETRのたびに経路上のゲートウェイ(WatchGuard等)が
-- `X-WatchGuard-Spam-ID`のようなヘッダーを付け替えるため、メール全体の
-- ハッシュでは同じメールを別物と判定してしまい、同じメールが数千件単位で
-- 重複保存された（実データで約2千通のMessage-IDが5万行に膨れていた）。
-- そのため重複判定にMessage-IDも使う。account_id+message_id_headerで引く
-- ためのインデックス（Message-IDを持たないメッセージは対象外）。
CREATE INDEX idx_messages_account_message_id
    ON messages (account_id, message_id_header)
    WHERE message_id_header IS NOT NULL;
