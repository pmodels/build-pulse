//! [rusqlite] based ORM to cache build results.
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::thread;

use rusqlite::{Connection, Params, Result, Row};
use tokio::sync::{mpsc, oneshot};

mod artifact;
mod build;
mod issue;
mod job;
mod run;
mod similarity;
mod stats;
mod tag;

pub use {artifact::*, build::*, issue::*, job::*, run::*, similarity::*, stats::*, tag::*};

/// Read [serde] serialized value from `row` and `idx`
#[macro_export]
macro_rules! read_value {
    ($row:ident, $idx:literal) => {
        serde_json::from_value($row.get($idx)?).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure($idx, rusqlite::types::Type::Text, e.into())
        })?
    };
}

/// Write as [serde] serializable value
#[macro_export]
macro_rules! write_value {
    ($val:expr) => {
        serde_json::to_value($val).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?
    };
}

/// Generate [Schema] for struct
#[macro_export]
macro_rules! schema {
    ($($table:tt for $model:ident$(<$($generic:tt),+>)? {
        $($schema:tt)+
    });+) => {
        $(impl$(<$($generic),+>)? $crate::db::Schema for $model$(<$($generic),+>)? {
            $crate::schema!(@create_table $table $($schema)+);
            $crate::schema!(@insert $table $($schema)+);
            $crate::schema!(@select_one $table);
            $crate::schema!(@select_all $table);
            $crate::schema!(@delete_all $table);
        })+
    };

    (@create_table $table:tt $($schema:tt)+) => {
        const CREATE_TABLE: &'static str = concat!(
            "CREATE TABLE IF NOT EXISTS ",
            stringify!($table),
            " (",
            $crate::schema!(@column_def $($schema)+),
            ") STRICT"
        );
    };

    (@column_def $col:tt $type:tt, $($rest:tt)+) => {
        concat!(stringify!($col), " ", stringify!($type), ",", $crate::schema!(@column_def $($rest)+))
    };
    (@column_def $col:tt $type:tt $($rest:tt)*) => {
        concat!(stringify!($col), " ", stringify!($type), $crate::schema!(@column_constraint $($rest)*))
    };

    (@column_constraint $constraint:tt, $($rest:tt)+) => {
        concat!(" ", stringify!($constraint), ",", $crate::schema!(@column_def $($rest)+))
    };
    (@column_constraint $constraint:tt $($rest:tt)*) => {
        concat!(" ", stringify!($constraint), $crate::schema!(@column_constraint $($rest)*))
    };
    (@column_constraint) => { "" };

    (@insert $table:tt $($schema:tt)+) => {
        const INSERT: &'static str = concat!(
            "INSERT INTO ",
            stringify!($table),
            " (",
            $crate::schema!(@insert_col $($schema)+),
            ") VALUES (",
            $crate::schema!(@repeat_vars $($schema)+),
            ")"
        );
    };

    (@insert_col id INTEGER PRIMARY KEY, $($rest:tt)+) => {
        $crate::schema!(@insert_col $($rest)+) // skip id
    };
    (@insert_col $col:tt $_type:tt, $($rest:tt)+) => {
        concat!(stringify!($col), ",", $crate::schema!(@insert_col $($rest)+))
    };
    (@insert_col $col:tt $_type:tt $($rest:tt)*) => {
        concat!(stringify!($col), $crate::schema!(@insert_col_const $($rest)*))
    };

    (@insert_col_const $_constraint:tt, $($rest:tt)+) => {
        concat!(",", $crate::schema!(@insert_col $($rest)*))
    };
    (@insert_col_const $_constraint:tt $($rest:tt)*) => {
        $crate::schema!(@insert_col_const $($rest)*)
    };
    (@insert_col_const) => { "" };

    (@repeat_vars id INTEGER PRIMARY KEY, $($rest:tt)+) => {
        $crate::schema!(@repeat_vars $($rest)+) // skip id
    };
    (@repeat_vars $_col:tt $_type:tt, $($rest:tt)+) => {
        concat!("?,", $crate::schema!(@repeat_vars $($rest)+))
    };
    (@repeat_vars $_col:tt $_type:tt $($rest:tt)*) => {
        concat!("?", $crate::schema!(@repeat_vars_constraint $($rest)*))
    };

    (@repeat_vars_constraint $_constraint:tt, $($rest:tt)+) => {
        concat!(",", $crate::schema!(@repeat_vars $($rest)*))
    };
    (@repeat_vars_constraint $_constraint:tt $($rest:tt)*) => {
        $crate::schema!(@repeat_vars_constraint $($rest)*)
    };
    (@repeat_vars_constraint) => { "" };

    (@select_one $table:tt) => {
        const SELECT_ONE: &'static str = concat!(
            "SELECT * FROM ",
            stringify!($table),
            " WHERE id = ?"
        );
    };

    (@select_all $table:tt) => {
        const SELECT_ALL: &'static str = concat!(
            "SELECT * FROM ",
            stringify!($table)
        );
    };

    (@delete_all $table:tt) => {
        const DELETE_ALL: &'static str = concat!(
            "DELETE FROM ",
            stringify!($table)
        );
    };
}

/// Convenience macro to run method on all types
macro_rules! for_all {
    ([$type:ty, $($rest:ty),*] => $($method:tt)+) => {
        <$type>::$($method)+;
        for_all!([$($rest),*] => $($method)+)
    };

    ([$type:ty] => $($method:tt)+) => {
        <$type>::$($method)+;
    };

    ($($method:tt)+) => {
        for_all!([SimilarityInfo, IssueInfo, Artifact, Run, JobBuild, Job, TagInfo] => $($method)+)
    };
}

/// Messages the [Database] object passes
trait Message: Send {
    /// Call closure on background thread with [Connection] and send back result
    fn call(self: Box<Self>, conn: &mut Connection);
}

impl<F, R> Message for (F, oneshot::Sender<R>)
where
    F: FnOnce(&mut Connection) -> R + Send,
    R: Send,
{
    fn call(self: Box<Self>, conn: &mut Connection) {
        let (closure, tx) = *self;
        tx.send(closure(conn));
    }
}

/// Database object
#[derive(Clone)]
pub struct Database {
    /// Sender to connection background thread
    tx: mpsc::UnboundedSender<Box<dyn Message>>,
}

/// Represents an item `T` in [Database]
#[derive(Debug, Copy, Clone)]
pub struct InDatabase<T> {
    /// Row ID of `item`
    pub id: i64,

    /// Item itself
    item: T,
}

impl<T> InDatabase<T> {
    /// Wrap item in [InDatabase] with new `id` from [Database]
    fn new(id: i64, item: T) -> Self {
        InDatabase { id, item }
    }

    pub fn item(self) -> T {
        self.item
    }
}

// Hash only considers the id property for [InDatabase]
impl<T> Hash for InDatabase<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

// Implicit deref to `T` from [InDatabase]
impl<T> Deref for InDatabase<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

// Implicit deref_mut to `T` from [InDatabase]
impl<T> DerefMut for InDatabase<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

// Establish ordering by the `id` primary key
impl<T> Ord for InDatabase<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl<T> PartialOrd for InDatabase<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for InDatabase<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for InDatabase<T> {}

impl Database {
    /// Open or create an `sqlite3` database at `path` returning [Database]
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Box<dyn Message>>();
        let (res_tx, res_rx) = oneshot::channel();

        // spawn a background thread to access the database
        let path = path.as_ref().to_owned();
        thread::spawn(move || {
            // enable regex, then try to open existing file, otherwise create a new one
            let mut conn = match rusqlite_regex::enable_auto_extension()
                .and_then(|_| Connection::open(path))
            {
                Ok(c) => {
                    res_tx.send(Ok(())).unwrap();
                    c
                }
                Err(e) => {
                    res_tx.send(Err(e)).unwrap();
                    return;
                }
            };

            // Handle calls
            while let Some(msg) = rx.blocking_recv() {
                msg.call(&mut conn);
            }
        });

        // ensure successful
        let db = res_rx.await.unwrap().map(|_| Self { tx })?;

        // create the necessary tables
        for_all!(create_table(&db).await?);

        Ok(db)
    }

    /// Get an async context to the [Database]'s [Connection]
    pub async fn call<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Box::new((f, tx))).unwrap();

        rx.await.unwrap()
    }

    /// Purge all rows (but not tables) from [Database]
    pub async fn purge_cache(&self) -> Result<()> {
        for_all!(delete_all(self).await?);
        Ok(())
    }
}

pub trait Schema: Send + Sized {
    const CREATE_TABLE: &'static str;
    const INSERT: &'static str;
    const SELECT_ONE: &'static str;
    const SELECT_ALL: &'static str;
    const DELETE_ALL: &'static str;

    /// Creates the table in [Database]
    async fn create_table(db: &Database) -> Result<usize> {
        db.call(|conn| conn.execute(Self::CREATE_TABLE, ())).await
    }
}

pub trait Queryable: Schema
where
    Self: 'static,
{
    /// Convert [Row] to `Self` with params
    fn map_row(row: &Row) -> Result<InDatabase<Self>>;

    /// Convert `self` to [Params] with `params`
    fn as_params(&self) -> Result<impl Params>;

    /// Insert `self` to [Database] with `params`
    async fn insert(self, db: &Database) -> Result<InDatabase<Self>> {
        db.call(|conn| {
            conn.prepare_cached(Self::INSERT)?
                .execute(self.as_params()?)?;
            Ok(InDatabase::new(conn.last_insert_rowid(), self))
        })
        .await
    }

    /// Select one of `Self` from [Database] by `id`
    async fn select_one(db: &Database, id: i64) -> Result<InDatabase<Self>> {
        db.call(move |conn| {
            conn.prepare_cached(Self::SELECT_ONE)?
                .query_one((id,), Self::map_row)
        })
        .await
    }

    /// Select all of `Self` from [Database]
    async fn select_all(db: &Database) -> Result<Vec<InDatabase<Self>>> {
        db.call(|conn| {
            conn.prepare_cached(Self::SELECT_ALL)?
                .query_and_then((), Self::map_row)?
                .collect()
        })
        .await
    }

    /// Delete all of `Self` from [Database]
    async fn delete_all(db: &Database) -> Result<usize> {
        db.call(|conn| conn.execute(Self::DELETE_ALL, ())).await
    }
}

pub trait Upsertable: Queryable {
    /// Upsert `self` to [Database]
    async fn upsert(self, db: &Database) -> Result<InDatabase<Self>>;
}
