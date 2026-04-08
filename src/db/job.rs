use arcstr::ArcStr;
use futures::{StreamExt, TryStreamExt, stream};

use crate::{
    db::{Queryable, Upsertable},
    schema,
};

/// [Job] stored in [super::Database]
#[derive(Clone)]
pub struct Job {
    /// Unique name of [Job]
    pub name: ArcStr,

    /// [Job] url
    pub url: ArcStr,

    /// Number of last [super::JobBuild]
    pub last_build: Option<u32>,
}

schema! {
    jobs for Job {
        id          INTEGER PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        url         TEXT NOT NULL,
        last_build  INTEGER
    }
}

impl Queryable for Job {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            Job {
                name: row.get::<_, String>(1)?.into(),
                url: row.get::<_, String>(2)?.into(),
                last_build: row.get(3)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((self.name.as_str(), self.url.as_str(), self.last_build))
    }
}

impl Upsertable for Job {
    async fn upsert(self, db: &super::Database) -> rusqlite::Result<super::InDatabase<Self>> {
        let name = self.name.clone();
        db.call(move |conn| {
            conn.prepare_cached(
                "
                INSERT INTO jobs (
                    name,
                    url,
                    last_build
                ) VALUES (?, ?, ?)
                    ON CONFLICT(name) DO UPDATE SET
                        last_build = excluded.last_build
                ",
            )?
            .execute(self.as_params()?)
        })
        .await?;

        // get the job as a second query in-case of an insert conflict
        Self::select_one_by_name(db, name).await
    }
}

impl Job {
    /// Get a [Job] from [super::Database] by name
    pub async fn select_one_by_name(
        db: &super::Database,
        name: ArcStr,
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM jobs
                WHERE name = ?
                ",
            )?
            .query_one((name.as_str(),), Self::map_row)
        })
        .await
    }

    /// Remove all [Job]s from [super::Database] by name
    pub async fn delete_all_by_blocklist<I>(
        db: &super::Database,
        names: I,
    ) -> rusqlite::Result<usize>
    where
        I: IntoIterator<Item = String>,
    {
        stream::iter(names)
            .then(|name| async {
                db.call(move |conn| {
                    let mut tx = conn.unchecked_transaction()?;
                    tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

                    // delete similarities first
                    tx.execute(
                        "
                        DELETE FROM similarities WHERE similarity_hash IN (
                            SELECT DISTINCT similarities.similarity_hash FROM similarities
                            JOIN issues ON issues.id = similarities.issue_id
                            JOIN runs ON runs.id = issues.run_id
                            JOIN builds ON builds.id = runs.build_id
                            JOIN jobs ON jobs.id = builds.job_id
                            WHERE name = ?
                        )
                        ",
                        (&name,),
                    )?;

                    // then issues
                    tx.execute(
                        "
                        DELETE FROM issues WHERE id IN (
                            SELECT issues.id FROM issues
                            JOIN runs ON runs.id = issues.run_id
                            JOIN builds ON builds.id = runs.build_id
                            JOIN jobs ON jobs.id = builds.job_id
                            WHERE name = ?
                        )
                        ",
                        (&name,),
                    )?;

                    // then artifacts
                    tx.execute(
                        "
                        DELETE FROM artifacts WHERE id IN (
                            SELECT artifacts.id FROM artifacts
                            JOIN runs ON runs.id = artifacts.run_id
                            JOIN builds ON builds.id = runs.build_id
                            JOIN jobs ON jobs.id = builds.job_id
                            WHERE name = ?
                        );
                        ",
                        (&name,),
                    )?;

                    // then runs
                    tx.execute(
                        "
                        DELETE FROM runs WHERE id IN (
                            SELECT runs.id FROM runs
                            JOIN builds ON builds.id = runs.build_id
                            JOIN jobs ON jobs.id = builds.job_id
                            WHERE name = ?
                        );
                        ",
                        (&name,),
                    )?;

                    // then builds
                    tx.execute(
                        "
                        DELETE FROM builds WHERE id IN (
                            SELECT builds.id FROM builds
                            JOIN jobs ON jobs.id = builds.job_id
                            WHERE name = ?
                        );
                        ",
                        (&name,),
                    )?;

                    // finally the job
                    tx.execute("DELETE FROM jobs WHERE name = ?", (&name,))
                })
                .await
            })
            .try_fold(0, |acc, x| async move { Ok(acc + x) })
            .await
    }
}
