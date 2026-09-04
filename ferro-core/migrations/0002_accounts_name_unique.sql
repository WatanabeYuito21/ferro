-- アカウント設定ファイル（accounts.toml）はnameをキーに既存アカウントと照合するため、
-- DB側でも一意性を保証しておく（ファイル以外の経路で重複が生まれるのを防ぐ防御的な制約）。
CREATE UNIQUE INDEX idx_accounts_name ON accounts(name);
