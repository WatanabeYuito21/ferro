-- UIDLコマンドに対応していないPOP3サーバー(実在する。レンタルサーバー宛の
-- アカウントで実際に踏んだ: CONNECT/USER/PASSは成功するがUIDLの直後だけ
-- 接続を切断される)向けのフォールバック判定を記録する。
-- NULL = 未判定（次回同期時にまずUIDLを試す）
-- 1    = 対応している（通常のUIDL差分方式で同期する）
-- 0    = 対応していない（RETR後の内容ハッシュを代替uidlとして使う）
ALTER TABLE accounts ADD COLUMN uidl_supported INTEGER;
