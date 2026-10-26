-- スレッド表示。`messages.thread_id`（0001から列だけあって未使用だった）に、同じやり取りに
-- 属するメッセージで共通の値（そのスレッドで最初に受信したメッセージのid）を入れる
-- （`ferro_core::threads`参照）。

-- スレッドのメッセージ一覧・件数を引く用。
CREATE INDEX idx_messages_thread ON messages (thread_id) WHERE thread_id IS NOT NULL;

-- まだスレッドを判定していないメッセージ（このマイグレーション以前に受信した分）を
-- バックグラウンドで少しずつ埋めるための目印。埋め終われば空になるので、起動のたびに
-- 「残りがあるか」を確かめても全件を舐めずに済む。
CREATE INDEX idx_messages_thread_pending ON messages (id) WHERE thread_id IS NULL;
