//! ferro-core: POP3/DB/Maildir/検索ロジックのライブラリ。
//! ferro-cli と src-tauri から共有される。

pub mod account_config;
pub mod account_setup;
pub mod color_rules;
pub mod credentials;
pub mod db;
pub mod mail;
pub mod maildir;
pub mod message_actions;
pub mod paths;
pub mod pop3;
pub mod reindex;
pub mod search;
pub mod sync;
