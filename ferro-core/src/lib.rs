//! ferro-core: POP3/DB/Maildir/検索ロジックのライブラリ。
//! ferro-cli と src-tauri から共有される。

pub mod account_config;
pub mod account_setup;
pub mod color_rules;
pub mod credentials;
pub mod db;
pub mod dedupe;
pub mod label_rules;
pub mod logging;
pub mod mail;
pub mod maintenance;
pub mod maildir;
pub mod message_actions;
pub mod message_filter;
pub mod message_search;
pub mod new_mail;
pub mod paths;
pub mod pop3;
pub mod reindex;
pub mod saved_searches;
pub mod search;
pub mod settings;
pub mod sync;
pub mod threads;
