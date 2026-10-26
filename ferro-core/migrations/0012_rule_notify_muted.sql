-- 受信ルールの「通知しない」（`label_rules::RuleActions::skip_notification`）が当たった新着の印。
-- 新着通知（`new_mail::since`）はこれが立っているメッセージを数えない。通知は同期の直後に
-- 「同期前の最大idより後」のメッセージから組み立てるため、ルールの判定結果をその時点まで
-- 持ち越す場所が要る（ルールを通知の時点で判定し直すと、本文条件のためにMaildirを
-- 読み直すことになるうえ、受信時の判定と食い違いうる）。
-- 既存の行は0のまま（ALTER TABLE ... ADD COLUMNは既存行を書き換えないので1000万件でも一瞬）。
ALTER TABLE messages ADD COLUMN notify_muted INTEGER NOT NULL DEFAULT 0;
