-- フォルダ(スター/スヌーズ/アーカイブ/ゴミ箱)・ラベル・設定機能の追加。
-- 受信専用(POP3のみ)方針は変えないため、送信/下書き/送信済みに相当する列は追加しない。

ALTER TABLE messages ADD COLUMN is_archived      INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0, 1));
-- スヌーズ解除予定時刻(Unixエポック秒)。NULL=スヌーズしていない。
-- 期限が過ぎたら(snoozed_until <= now)受信箱側のクエリで自動的に戻す(バックグラウンドジョブ不要)。
ALTER TABLE messages ADD COLUMN snoozed_until    INTEGER;
-- 一覧に添付件数チップを出すため、sync/reindex時にMaildirから読んで計算しておく
-- (一覧描画のたびに毎回Maildirを読み直すのはCLAUDE.mdの「起動時に何も舐めない」方針に反するため)。
ALTER TABLE messages ADD COLUMN attachment_count INTEGER NOT NULL DEFAULT 0;
-- 本文冒頭のプレビュー(~120文字)。理由はattachment_countと同じ。
ALTER TABLE messages ADD COLUMN preview          TEXT;

CREATE TABLE labels (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL UNIQUE,
    color      TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE message_labels (
    message_id INTEGER NOT NULL REFERENCES messages(id),
    label_id   INTEGER NOT NULL REFERENCES labels(id),
    PRIMARY KEY (message_id, label_id)
);
CREATE INDEX idx_message_labels_label ON message_labels (label_id);

-- UI設定(一覧プレビュー行の表示可否等)。キー/値ともTEXT、値側はJSON等呼び出し側の解釈に委ねる。
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- フォルダ別一覧用の部分インデックス。CLAUDE.mdの既存方針(list_recentのコメント参照)どおり、
-- (?1 IS NULL OR ...)のようなOR分岐は使わずフォルダごとにSQL文字列を出し分けるので、
-- フォルダごとに素直な部分インデックスを用意すればよい。
CREATE INDEX idx_messages_starred  ON messages (date_header) WHERE is_flagged = 1 AND is_deleted = 0;
CREATE INDEX idx_messages_archived ON messages (date_header) WHERE is_archived = 1 AND is_deleted = 0;
CREATE INDEX idx_messages_snoozed  ON messages (date_header) WHERE snoozed_until IS NOT NULL AND is_deleted = 0;
