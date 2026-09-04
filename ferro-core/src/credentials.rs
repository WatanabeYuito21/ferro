use keyring::Entry;

/// keyring上でのサービス名。OSの資格情報マネージャー内でferroのエントリを
/// 他アプリと区別するためのキー。
const SERVICE_NAME: &str = "ferro-mail";

fn entry(account_id: i64) -> keyring::Result<Entry> {
    Entry::new(SERVICE_NAME, &account_id.to_string())
}

pub fn set_password(account_id: i64, password: &str) -> keyring::Result<()> {
    entry(account_id)?.set_password(password)
}

pub fn get_password(account_id: i64) -> keyring::Result<String> {
    entry(account_id)?.get_password()
}

pub fn delete_password(account_id: i64) -> keyring::Result<()> {
    entry(account_id)?.delete_credential()
}
