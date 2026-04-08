use jenkins_api::build::BuildStatus;

use crate::{
    db::{Queryable, Upsertable},
    read_value, schema, write_value,
};

/// [JobBuild] in [super::Database]
#[derive(Clone)]
pub struct JobBuild {
    /// Build url
    pub url: String,

    /// Build status
    pub status: Option<BuildStatus>,

    /// Build number
    pub number: u32,

    /// Build timestamp
    pub timestamp: u64,

    /// ID of associated [super::Job]
    pub job_id: i64,
}

schema! {
    builds for JobBuild {
        id          INTEGER PRIMARY KEY,
        url         TEXT NOT NULL UNIQUE,
        status      TEXT,
        number      INTEGER NOT NULL,
        timestamp   INTEGER NOT NULL,
        job_id      INTEGER NOT NULL REFERENCES jobs(id)
    }
}

impl Queryable for JobBuild {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            JobBuild {
                url: row.get(1)?,
                status: read_value!(row, 2),
                number: row.get(3)?,
                timestamp: row.get(4).map(i64::cast_unsigned)?,
                job_id: row.get(5)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            &self.url,
            write_value!(self.status),
            self.number,
            self.timestamp.cast_signed(),
            self.job_id,
        ))
    }
}

impl Upsertable for JobBuild {
    async fn upsert(self, db: &super::Database) -> rusqlite::Result<super::InDatabase<Self>> {
        let number = self.number;
        let job_id = self.job_id;

        db.call(move |conn| {
            conn.prepare_cached(
                "
                INSERT INTO builds (
                    url,
                    status,
                    number,
                    timestamp,
                    job_id
                ) VALUES (?, ?, ?, ?, ?)
                    ON CONFLICT(url) DO UPDATE SET
                        status = excluded.status,
                        number = excluded.number,
                        timestamp = excluded.timestamp,
                        job_id = excluded.job_id
                ",
            )?
            .execute(self.as_params()?)
        })
        .await?;

        Self::select_one_by_job(db, job_id, number).await
    }
}

impl JobBuild {
    /// Get a [JobBuild] from [super::Database] by [super::Job] id and build number
    pub async fn select_one_by_job(
        db: &super::Database,
        job_id: i64,
        number: u32,
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM builds
                WHERE job_id = ?
                AND number = ?
                ORDER BY number DESC
                ",
            )?
            .query_one((job_id, number), Self::map_row)
        })
        .await
    }

    /// Get all [JobBuild] from [super::Database] by [super::Job]
    pub async fn select_all_by_job(
        db: &super::Database,
        job_id: i64,
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM builds
                WHERE job_id = ?
                ORDER BY number DESC
                ",
            )?
            .query_map((job_id,), Self::map_row)?
            .collect()
        })
        .await
    }

    /// Remove all [JobBuild]s which aren't referenced by [super::Job] from [super::Database]
    pub async fn delete_all_orphan(db: &super::Database) -> rusqlite::Result<()> {
        db.call(|conn| {
            conn.execute_batch(
                "
                BEGIN;
                DELETE FROM similarities WHERE similarity_hash IN (
                    SELECT DISTINCT similarities.similarity_hash FROM similarities
                    JOIN issues ON issues.id = similarities.issue_id
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE number < last_build
                );
                DELETE FROM issues WHERE id IN (
                    SELECT issues.id FROM issues
                    JOIN runs ON runs.id = issues.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE number < last_build
                );
                DELETE FROM artifacts WHERE id IN (
                    SELECT artifacts.id FROM artifacts
                    JOIN runs ON runs.id = artifacts.run_id
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE number < last_build
                );
                DELETE FROM runs WHERE id IN (
                    SELECT runs.id FROM runs
                    JOIN builds ON builds.id = runs.build_id
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE number < last_build
                );
                DELETE FROM builds WHERE id IN (
                    SELECT builds.id FROM builds
                    JOIN jobs ON jobs.id = builds.job_id
                    WHERE number < last_build
                );
                COMMIT;
                ",
            )
        })
        .await
    }
}
