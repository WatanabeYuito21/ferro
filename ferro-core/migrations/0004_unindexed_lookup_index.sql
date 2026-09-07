-- 検索インデックス未投入メッセージを拾い直す`list_unindexed`は、95000件規模の
-- 実データでは既存インデックスに乗らずSCANになっていた可能性が高い（未検証だったが、
-- Tantivy IndexWriterクラッシュの多発でこのクエリの呼び出し頻度が増えたため対策する）。
-- クエリの述語(is_deleted=0 AND fts_indexed_at IS NULL)とORDER BY idにそのまま対応する
-- 部分インデックス。
CREATE INDEX idx_messages_unindexed ON messages (id) WHERE fts_indexed_at IS NULL AND is_deleted = 0;
