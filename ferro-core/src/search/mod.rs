//! Tantivyベースの全文検索インデックス。Subject/From(表示名・アドレス)/本文
//! (Maildirから読み直したプレーンテキスト)を索引化する。
//!
//! SQLiteとは疎結合で、`messages.id`をdoc idとして流用するだけの
//! 完全にSQLite/Maildirから再構築可能な派生キャッシュとして扱う
//! （スキーマが変わって既存インデックスと不一致になった場合は削除して作り直す）。

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{QueryParser, QueryParserError};
use tantivy::schema::{Field, INDEXED, STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Query(#[from] QueryParserError),
    #[error("search index writer lock was poisoned")]
    LockPoisoned,
}

pub type Result<T> = std::result::Result<T, SearchError>;

struct Fields {
    id: Field,
    subject: Field,
    from: Field,
    body: Field,
}

fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let id = builder.add_u64_field("id", INDEXED | STORED);
    let subject = builder.add_text_field("subject", TEXT | STORED);
    let from = builder.add_text_field("from", TEXT | STORED);
    let body = builder.add_text_field("body", TEXT);
    let schema = builder.build();
    (schema, Fields { id, subject, from, body })
}

/// 新規/更新するメッセージの索引化対象データ。
pub struct IndexableMessage<'a> {
    pub id: i64,
    pub subject: &'a str,
    /// 表示名・アドレス両方をまとめて検索対象にする（例: "Alice alice@example.com"）。
    pub from: &'a str,
    pub body: &'a str,
}

pub struct SearchIndex {
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: Fields,
    query_parser: QueryParser,
}

impl SearchIndex {
    /// 指定ディレクトリのインデックスを開く。無ければ新規作成する。
    /// スキーマが既存インデックスと一致しない場合は、派生キャッシュとして
    /// 扱い、ディレクトリの中身を削除してから作り直す（マイグレーションはしない）。
    pub fn open_or_create(path: &Path) -> Result<SearchIndex> {
        fs::create_dir_all(path)?;
        let (schema, fields) = build_schema();

        let index = match Index::open_or_create(open_mmap_dir(path)?, schema.clone()) {
            Ok(index) => index,
            Err(tantivy::TantivyError::SchemaError(_)) => {
                clear_dir(path)?;
                Index::open_or_create(open_mmap_dir(path)?, schema)?
            }
            Err(e) => return Err(e.into()),
        };

        Self::from_index(index, fields)
    }

    /// テスト専用のインメモリインデックス。実ファイルI/Oが要らないテストは
    /// こちらを使う。`MmapDirectory`ベースの実装はWindowsで並列テスト実行時に
    /// 一時ディレクトリへの書き込みが断続的に`PermissionDenied`になることがあり
    /// （tantivyのマージ/GCスレッドとファイルシステムの競合とみられる）、
    /// ディスクI/Oが本質的でないテストではこれを避けるため。
    #[cfg(test)]
    pub(crate) fn create_in_ram() -> Result<SearchIndex> {
        let (schema, fields) = build_schema();
        Self::from_index(Index::create_in_ram(schema), fields)
    }

    fn from_index(index: Index, fields: Fields) -> Result<SearchIndex> {
        let writer: IndexWriter = index.writer(50_000_000)?;
        // OnCommitWithDelayは別スレッドでの非同期リロードなので、commit直後に
        // searchしても反映されているとは限らない。呼び出し側がcommit()の都度
        // 明示的にreloadするManualの方が、索引投入直後に検索したいこのアプリの
        // 用途（sync直後の一覧更新等）には向いている。
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let query_parser =
            QueryParser::for_index(&index, vec![fields.subject, fields.from, fields.body]);

        Ok(SearchIndex {
            reader,
            writer: Mutex::new(writer),
            fields,
            query_parser,
        })
    }

    /// メッセージを索引化する。既に同じidの文書があれば置き換える
    /// （tantivyには更新という概念がないため削除してから追加する）。
    /// 呼び出し側でバッチ単位に`commit`をまとめること。
    pub fn index_message(&self, message: &IndexableMessage) -> Result<()> {
        let writer = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        writer.delete_term(Term::from_field_u64(self.fields.id, message.id as u64));
        writer.add_document(doc!(
            self.fields.id => message.id as u64,
            self.fields.subject => message.subject,
            self.fields.from => message.from,
            self.fields.body => message.body,
        ))?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// 全メッセージの索引を消す（`reindex_all`が最初に呼ぶ）。
    pub fn clear(&self) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        writer.delete_all_documents()?;
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// クエリにマッチする`messages.id`を関連度順で返す。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<i64>> {
        let searcher = self.reader.searcher();
        let query = self.query_parser.parse_query(query)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

        top_docs
            .into_iter()
            .map(|(_score, doc_address)| {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_address)?;
                let id = doc
                    .get_first(self.fields.id)
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        tantivy::TantivyError::SchemaError("indexed document missing id field".into())
                    })?;
                Ok(id as i64)
            })
            .collect()
    }
}

fn open_mmap_dir(path: &Path) -> tantivy::Result<MmapDirectory> {
    Ok(MmapDirectory::open(path)?)
}

fn clear_dir(path: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
