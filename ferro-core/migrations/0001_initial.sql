-- POP3アカウント設定。パスワード自体はkeyring crate経由でOS資格情報マネージャーに
-- 保存するため、ここには持たない。
CREATE TABLE accounts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL,
    username    TEXT NOT NULL,
    use_tls     INTEGER NOT NULL CHECK (use_tls IN (0, 1)),
    created_at  INTEGER NOT NULL
);

-- メッセージのメタデータ。生メール本体はMaildir
-- (cur/xx/yy/<account_id>-<uidl>.eml、account_id+uidlから決定的に導出可能)に保存し、
-- ここには一覧・ソート・フィルタ・検索連携に必要な構造化データのみ持つ。
CREATE TABLE messages (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id        INTEGER NOT NULL REFERENCES accounts(id),

    -- POP3のUIDL。sync時の差分計算（既知UIDLかどうか）とMaildirファイル名導出に使う。
    uidl              TEXT NOT NULL,

    message_id_header TEXT,
    subject           TEXT,
    from_name         TEXT,
    from_addr         TEXT,
    to_addr           TEXT,

    -- メール本文のDateヘッダーをパースしたUnixエポック秒。一覧のソート/
    -- キーセットページネーションのカーソルに使う（RFC2822の生文字列ではソート順が壊れるため）。
    date_header       INTEGER NOT NULL,

    size_bytes        INTEGER NOT NULL,

    -- スレッドグルーピング用。当面は未使用（NULL）で、スレッド機能実装時に設定する。
    thread_id         INTEGER,

    is_read           INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    is_flagged        INTEGER NOT NULL DEFAULT 0 CHECK (is_flagged IN (0, 1)),
    -- ソフトデリート。POP3サーバー側のDELEとは独立したローカルの削除フラグ。
    is_deleted        INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0, 1)),

    -- 生メールの保存先。現状は'maildir'のみ運用するが、将来SQLite BLOB保存に
    -- 切り替える可能性に備えて残してある。
    raw_storage_kind  TEXT NOT NULL DEFAULT 'maildir',
    raw_blob          BLOB,

    -- Tantivy検索インデックスとの連携用（messages.idをdoc idとして流用する）。
    fts_indexed_at    INTEGER,
    fts_doc_version   INTEGER NOT NULL DEFAULT 0,

    created_at        INTEGER NOT NULL,

    UNIQUE (account_id, uidl)
);

-- アカウント指定ありの一覧取得（キーセットページネーション: account_id + date_header順）用。
CREATE INDEX idx_messages_account_date
    ON messages (account_id, date_header)
    WHERE is_deleted = 0;

-- account_idを指定しない全アカウント横断の一覧取得用。
CREATE INDEX idx_messages_date_not_deleted
    ON messages (date_header)
    WHERE is_deleted = 0;
