//! Tantivyベースの全文検索インデックス。Subject/From(表示名・アドレス)/本文
//! (Maildirから読み直したプレーンテキスト)を索引化する。
//!
//! SQLiteとは疎結合で、`messages.id`をdoc idとして流用するだけの
//! 完全にSQLite/Maildirから再構築可能な派生キャッシュとして扱う
//! （スキーマが変わって既存インデックスと不一致になった場合は削除して作り直す）。

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, PhrasePrefixQuery, Query};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{LowerCaser, RemoveLongFilter, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, Order, ReloadPolicy, Term, doc};

mod cjk_tokenizer;
use cjk_tokenizer::CjkBigramTokenizer;

/// このトークナイザ名でスキーマとインデックスの両方に登録する
/// （`build_word_query`もクエリ文字列を索引投入時と同じトークナイザで分割するため、
/// フィールドに設定した名前がインデックスの`TokenizerManager`に登録されている必要がある）。
const TOKENIZER_NAME: &str = "ferro_cjk_bigram";

/// 検索結果の並び順に使う日付フィールドの名前（`order_by_u64_field`はField
/// ハンドルではなくスキーマ上の名前で指定するAPIのため文字列定数にしておく）。
const DATE_HEADER_FIELD_NAME: &str = "date_header";

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("search index writer lock was poisoned")]
    LockPoisoned,
    #[error("search index writer is unavailable (a previous recovery attempt failed)")]
    WriterUnavailable,
}

pub type Result<T> = std::result::Result<T, SearchError>;

/// commit失敗時のリトライ回数上限。
/// 元は5回・base 100ms（合計最大1秒待機）だったが、実機で依然として
/// リトライを使い切ってエラーが表面化するケースが確認されたため、
/// アンチウイルス/EDR（Microsoft Defender for Endpoint等）のリアルタイム
/// スキャン・振る舞い監視が数秒かかる想定に合わせて広げた（合計最大約8.4秒）。
/// 読み取り系コマンドは別接続(`read_conn`)なので、この待機はsync/reindexの
/// 呼び出し自体を長引かせるだけでアプリ全体をブロックしない。
const MAX_COMMIT_ATTEMPTS: u32 = 8;
/// リトライ間隔のベース（試行回数に比例させる簡易的な指数バックオフ）。
const RETRY_BASE_DELAY: Duration = Duration::from_millis(300);

/// Windows実機で断続的に起きるTantivyの`IndexWriter`クラッシュ
/// （`SearchIndex::commit`のドキュメント参照。アンチウイルスのリアルタイムスキャン等
/// によるファイルI/O競合が疑わしいが未確定）を吸収するための共通リトライヘルパー。
/// `reindex_all`と`sync`の両方の呼び出し元で使う。
///
/// `f`は「索引投入からcommitまでの一連の処理」を丸ごと再実行可能な形で渡すこと。
/// `commit`が失敗すると`recover_writer`で新しいwriterに差し替わり、直前の
/// `add_document`分は失われるため、呼び出し側は同じ内容を最初からやり直す必要がある。
pub fn with_commit_retry<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last_err = None;
    for attempt in 0..MAX_COMMIT_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_BASE_DELAY * attempt);
        }
        match f() {
            Ok(value) => return Ok(value),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("loop runs MAX_COMMIT_ATTEMPTS >= 1 times"))
}

struct Fields {
    id: Field,
    subject: Field,
    from: Field,
    body: Field,
    date_header: Field,
}

/// 日本語（分かち書きしない言語）を含むテキスト用のフィールドオプション。
/// 標準の"default"トークナイザ（空白・記号区切り）ではなく、
/// `cjk_tokenizer`のCJKバイグラム＋英数字単語分割を使う。
fn cjk_text_options(stored: bool) -> TextOptions {
    let indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let options = TextOptions::default().set_indexing_options(indexing);
    if stored { options.set_stored() } else { options }
}

fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let id = builder.add_u64_field("id", INDEXED | STORED);
    let subject = builder.add_text_field("subject", cjk_text_options(true));
    let from = builder.add_text_field("from", cjk_text_options(true));
    let body = builder.add_text_field("body", cjk_text_options(false));
    // 検索結果は関連度ではなく常に受信日時の新しい順で返す（`search`のドキュメント
    // 参照）。ソートにしか使わないのでFASTのみで十分（INDEXED/STOREDは不要）。
    let date_header = builder.add_u64_field(DATE_HEADER_FIELD_NAME, FAST);
    let schema = builder.build();
    (
        schema,
        Fields { id, subject, from, body, date_header },
    )
}

fn register_tokenizer(index: &Index) {
    index.tokenizers().register(
        TOKENIZER_NAME,
        TextAnalyzer::builder(CjkBigramTokenizer)
            .filter(RemoveLongFilter::limit(40))
            .filter(LowerCaser)
            .build(),
    );
}

/// 新規/更新するメッセージの索引化対象データ。
pub struct IndexableMessage<'a> {
    pub id: i64,
    pub subject: &'a str,
    /// 表示名・アドレス両方をまとめて検索対象にする（例: "Alice alice@example.com"）。
    pub from: &'a str,
    pub body: &'a str,
    /// 検索結果の並び順(新しい順)に使う。Unixエポック秒。負の値は想定していない
    /// （`messages.date_header`は実質常に1970年以降の値）。
    pub date_header: i64,
}

pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    // Some(_)が通常状態。recover_writer中、古いwriterをdropして新しいwriterを
    // 作り直す間だけ一時的にNoneになりうる（tantivyは1つのDirectoryにつき
    // IndexWriterを同時に1つしか持てないため、drop前に新しいものは作れない）。
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
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
        register_tokenizer(&index);
        // `index.writer(...)`はCPUコア数に応じて複数(最大8)のマージ/インデックス
        // ワーカースレッドを立ち上げるが、Windows実機で断続的に起きるIndexWriter
        // クラッシュ（`commit`のドキュメント参照）はこれらワーカースレッドの異常終了が
        // 疑われている。同時に動くスレッドが多いほどファイルI/Oの同時発生量が増え、
        // アンチウイルス/EDRのリアルタイムスキャンと衝突する機会も増えると考えられる
        // ため、スループットより信頼性を優先してシングルスレッドに固定する。
        let writer: IndexWriter = index.writer_with_num_threads(1, 50_000_000)?;
        // OnCommitWithDelayは別スレッドでの非同期リロードなので、commit直後に
        // searchしても反映されているとは限らない。呼び出し側がcommit()の都度
        // 明示的にreloadするManualの方が、索引投入直後に検索したいこのアプリの
        // 用途（sync直後の一覧更新等）には向いている。
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        // クエリの構築は`QueryParser`を使わず`search`内で自前で組み立てる
        // （`build_word_query`のドキュメント参照。末尾の片方が部分一致
        // (`PhrasePrefixQuery`)になるようにするため）。

        Ok(SearchIndex {
            index,
            reader,
            writer: Mutex::new(Some(writer)),
            fields,
        })
    }

    /// `commit`が失敗した後の復旧用。Windows実機で断続的に観測される、
    /// tantivyのマージ/GCワーカースレッドが異常終了して以後そのIndexWriterの
    /// 全操作が失敗し続ける状態（`commit`のエラーメッセージに
    /// "index writer was killed"のようなことが出る）から立て直すために、
    /// 新しいIndexWriterを作り直して差し替える。
    ///
    /// tantivyは1つのDirectoryにつきIndexWriterを同時に1つしか持てない
    /// （ロックファイルで強制される）ため、古いwriterを先にdropしてロックを
    /// 解放してから新しいwriterを作らなければならない。順序を逆にすると
    /// `LockFailure`で新規作成自体が失敗する。
    ///
    /// 直前のcommit未確定分のドキュメントはこの新しいwriterには引き継がれない
    /// ため、呼び出し側は復旧後にそのバッチのindex_message/commitをやり直すこと。
    fn recover_writer(&self) -> Result<()> {
        let mut slot = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        *slot = None; // 古いwriterをここでdropし、ロックファイルを解放する
        // `from_index`と同じ理由でシングルスレッドに固定する。
        *slot = Some(self.index.writer_with_num_threads(1, 50_000_000)?);
        Ok(())
    }

    /// メッセージを索引化する。既に同じidの文書があれば置き換える
    /// （tantivyには更新という概念がないため削除してから追加する）。
    /// 呼び出し側でバッチ単位に`commit`をまとめること。
    pub fn index_message(&self, message: &IndexableMessage) -> Result<()> {
        let slot = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        let writer = slot.as_ref().ok_or(SearchError::WriterUnavailable)?;
        writer.delete_term(Term::from_field_u64(self.fields.id, message.id as u64));
        writer.add_document(doc!(
            self.fields.id => message.id as u64,
            self.fields.subject => message.subject,
            self.fields.from => message.from,
            self.fields.body => message.body,
            self.fields.date_header => message.date_header as u64,
        ))?;
        Ok(())
    }

    /// メッセージを索引から取り除く（削除時用。再追加はしない）。
    /// `index_message`同様、呼び出し側で`commit`すること。
    pub fn delete_message(&self, id: i64) -> Result<()> {
        let slot = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
        let writer = slot.as_ref().ok_or(SearchError::WriterUnavailable)?;
        writer.delete_term(Term::from_field_u64(self.fields.id, id as u64));
        Ok(())
    }

    /// commit失敗時、以後の呼び出しが復旧できるようwriterを作り直してから
    /// (ベストエフォート。作り直し自体の失敗は無視する) 元のエラーを返す。
    /// このバッチの未確定分は失われているため、呼び出し側は復旧後に
    /// 同じ`index_message`群を再実行してから`commit`をやり直すこと。
    pub fn commit(&self) -> Result<()> {
        let result = (|| -> Result<()> {
            let mut slot = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
            let writer = slot.as_mut().ok_or(SearchError::WriterUnavailable)?;
            writer.commit()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.reader.reload()?;
                Ok(())
            }
            Err(e) => {
                let _ = self.recover_writer();
                Err(e)
            }
        }
    }

    /// 全メッセージの索引を消す（`reindex_all`が最初に呼ぶ）。
    pub fn clear(&self) -> Result<()> {
        let result = (|| -> Result<()> {
            let mut slot = self.writer.lock().map_err(|_| SearchError::LockPoisoned)?;
            let writer = slot.as_mut().ok_or(SearchError::WriterUnavailable)?;
            writer.delete_all_documents()?;
            writer.commit()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.reader.reload()?;
                Ok(())
            }
            Err(e) => {
                let _ = self.recover_writer();
                Err(e)
            }
        }
    }

    /// クエリにマッチする`messages.id`を受信日時の新しい順で返す。
    ///
    /// 以前は関連度スコア順だったが、「検索しても新しい順に並ばない」という
    /// 指摘を受けて日付順に変更した。並び替えは`TopDocs`の収集段階
    /// (`order_by_u64_field`)で行うため、AND結合でマッチした文書が`limit`件を
    /// 超える場合でも、関連度が高いが古いものに押し出されて新しいものが
    /// 一覧から漏れる、ということが起きない（マッチした全文書のうち常に
    /// 一番新しいlimit件を返す）。
    ///
    /// クエリは空白区切りの「単語」ごとに`build_word_query`でフレーズ・
    /// プレフィックスクエリを組み立て、単語間はAND（`QueryParser`時代と同じ、
    /// 「検索精度が低い」対策）、1単語に対する件名/差出人/本文はOR（いずれかの
    /// フィールドで一致すればよい）で結合する。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<i64>> {
        let searcher = self.reader.searcher();

        let mut word_queries: Vec<Box<dyn Query>> = Vec::new();
        for word in query.split_whitespace() {
            let field_queries: Vec<(Occur, Box<dyn Query>)> = [self.fields.subject, self.fields.from, self.fields.body]
                .into_iter()
                .filter_map(|field| self.build_word_query(field, word))
                .map(|q| (Occur::Should, q))
                .collect();
            if !field_queries.is_empty() {
                word_queries.push(Box::new(BooleanQuery::new(field_queries)));
            }
        }
        if word_queries.is_empty() {
            return Ok(Vec::new());
        }
        let combined: Box<dyn Query> = if word_queries.len() == 1 {
            word_queries.into_iter().next().expect("checked len == 1 above")
        } else {
            Box::new(BooleanQuery::new(
                word_queries.into_iter().map(|q| (Occur::Must, q)).collect(),
            ))
        };

        let top_docs = searcher.search(
            combined.as_ref(),
            &TopDocs::with_limit(limit).order_by_u64_field(DATE_HEADER_FIELD_NAME, Order::Desc),
        )?;

        top_docs
            .into_iter()
            .map(|(_date_header, doc_address)| {
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

    /// クエリ文字列の1単語(空白区切り)を、指定フィールド用のトークン列に変換し、
    /// フレーズ・プレフィックスクエリを組み立てる。索引投入時と同じトークナイザ
    /// (`register_tokenizer`)でトークン化するため、大文字小文字や区切りの扱いは
    /// 一致する。単語が(記号のみ等で)1トークンにもならない場合は`None`。
    ///
    /// 最後のトークンだけは完全一致ではなく前方一致（`PhrasePrefixQuery`）にする。
    /// これが要る理由: 「srv-jpp-w02」を「srv-jpp-w」で検索してもヒットしない、
    /// という指摘への対応。バイグラムトークナイザは2文字未満の断片（クエリ末尾の
    /// 1文字「w」等）をそのまま1トークンとして扱うが、文書側の対応する語
    /// ("w02")はバイグラム("w0","02")になっているため、単純な完全一致の
    /// フレーズクエリだとクエリの最後の断片が長い単語の途中で終わっている場合に
    /// 一致しない。`PhrasePrefixQuery`は最後の項だけを前方一致で展開する
    /// （1トークンしか無い場合は単純な前方一致クエリにフォールバックする。
    /// tantivyの実装参照）ため、これで解決する。
    fn build_word_query(&self, field: Field, word: &str) -> Option<Box<dyn Query>> {
        let mut analyzer = self
            .index
            .tokenizers()
            .get(TOKENIZER_NAME)
            .expect("tokenizer is registered in from_index/register_tokenizer");
        let mut token_stream = analyzer.token_stream(word);
        let mut terms = Vec::new();
        while token_stream.advance() {
            terms.push(Term::from_field_text(field, &token_stream.token().text));
        }
        if terms.is_empty() {
            None
        } else {
            Some(Box::new(PhrasePrefixQuery::new(terms)))
        }
    }

    /// 現在インデックスに入っている文書数。GUI起動時に、DBの`fts_indexed_at`は
    /// 「投入済み」を指しているのにインデックス自体は空、という不整合
    /// （スキーマ変更での自己修復や、`search_index`ディレクトリの手動削除
    /// ―CLAUDE.md記載の回避策―の直後に起きうる。`fts_indexed_at`はSQLite側の
    /// 列でTantivy側の状態とは独立しているため、インデックスだけが消えても
    /// この列は古い値のままになり、自動キャッチアップの対象からも外れて
    /// しまう。実際にこの不整合で検索が壊れたままになる不具合を踏んだ）を
    /// 検知するために使う（`src-tauri`の起動処理参照）。
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
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
