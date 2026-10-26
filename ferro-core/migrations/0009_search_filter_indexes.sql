-- 詳細検索（`message_search`）で、全文検索の語を伴わない`has:attachment`/`is:unread`だけの
-- 検索は「条件に合うものを受信日時の新しい順に50件」をSQLiteだけで探す。該当が少ないと
-- date_header順のインデックスを末尾まで舐めることになり（実データ7万件で約0.3秒、
-- 1000万件規模では数十秒）、要件の規模に耐えないため、条件ごとの部分インデックスを置く。
-- スター付き(`is:starred`)は0003の`idx_messages_starred`がそのまま使える。
-- `message_search::filter_in_db`は、これらが使われるよう条件を`?`ではなくリテラルで書く
-- （SQLiteは部分インデックスの条件をバインド変数から推論しない）。
CREATE INDEX idx_messages_with_attachments
    ON messages (date_header)
    WHERE attachment_count > 0 AND is_deleted = 0;
CREATE INDEX idx_messages_unread
    ON messages (date_header)
    WHERE is_read = 0 AND is_deleted = 0;
