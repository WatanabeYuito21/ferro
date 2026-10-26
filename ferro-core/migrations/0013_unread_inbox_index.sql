-- 受信箱の未読数（GUIのトレイアイコン・タスクバーのバッジ。`messages::count_unread_inbox`）を
-- インデックスだけで数えるための部分インデックス。`idx_messages_unread`（0009）は条件が
-- is_read/is_deleted だけなので、受信箱の判定（アーカイブ・スヌーズ）のために行を1件ずつ
-- 読みに行くことになり、実データ（未読7.2万件）で約0.3秒かかった。こちらは0.003秒。
-- 条件はクエリ側もリテラルで同じに書く（SQLiteはバインド変数から部分インデックスの条件を推論しない）。
CREATE INDEX idx_messages_unread_inbox
    ON messages (snoozed_until)
    WHERE is_read = 0 AND is_deleted = 0 AND is_archived = 0;
