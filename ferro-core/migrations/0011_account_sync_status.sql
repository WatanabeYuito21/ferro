-- アカウントごとの直近の同期結果（`db::sync_status`）。`sync::sync_account_with_limit`が
-- 成功・失敗のたびに上書きする。それまで同期の結果はGUIの一時的な表示か`ferro.log`にしか
-- 残らず、調子の悪いアカウント（UIDL非対応のレンタルサーバー等）がいつから失敗しているのかを
-- ログを開かずには確認できなかった。
--
-- `purged_uidls`と同じく実データを持たない記録なので、アカウント削除時はCASCADEで消す。
CREATE TABLE account_sync_status (
    account_id           INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    last_attempt_at      INTEGER NOT NULL,
    last_success_at      INTEGER,
    -- 直近の同期が失敗していればそのエラー。成功したらNULLに戻す。
    last_error           TEXT,
    -- 直近に成功した同期の結果。
    last_fetched         INTEGER NOT NULL DEFAULT 0,
    last_remaining       INTEGER NOT NULL DEFAULT 0,
    last_ended_early     INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0
);
