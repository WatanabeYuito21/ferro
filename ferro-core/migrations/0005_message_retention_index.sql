-- メール保持期間のクリーンアップ（`message_actions::purge_expired_batch`）用。
-- スター付き(is_flagged=1)は保持期間に関わらず自動削除の対象外にするため、
-- is_flagged=0の部分インデックスにしておく（is_deletedでは絞らない。
-- 既にソフト削除済みの古いメッセージも保持期間切れなら完全に削除する対象のため）。
CREATE INDEX idx_messages_retention ON messages (date_header) WHERE is_flagged = 0;
