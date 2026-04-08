use jenkins_api::build::BuildStatus;

use crate::{config::Severity, read_value, write_value};

/// Statistics of [super::Issue]s and [super::Run]s in [super::Database]
#[derive(Default)]
pub struct Statistics {
    /// Number of [BuildStatus::Success] [super::Job]s
    pub successful_jobs: u64,

    /// Total number of tracked [super::Job]s
    pub total_jobs: u64,

    /// Successful [super::Run]s
    pub successful: Vec<i64>,

    /// Unstable [super::Run]s
    pub unstable: Vec<i64>,

    /// Failed [super::Run]s
    pub failures: Vec<i64>,

    /// Aborted [super::Run]s
    pub aborted: Vec<i64>,

    /// Not built [super::Run]s
    pub not_built: Vec<i64>,

    /// Total [super::Issue]s found
    pub issues_found: u64,

    /// [super::Run]s with unknown issues
    pub unknown_runs: Vec<i64>,
}

impl Statistics {
    /// Gets [super::Database]'s [Statistics]
    pub async fn query(db: &super::Database) -> rusqlite::Result<Self> {
        // calculate success/failures for latest runs
        let mut stats = db
            .call(|conn| {
                conn.prepare(
                    "
                    SELECT status, id FROM runs
                    WHERE build_id IN (
                            SELECT id FROM builds
                            GROUP BY job_id
                            HAVING MAX(number)
                        )
                    ",
                )?
                .query_map((), |row| Ok((read_value!(row, 0), row.get(1)?)))?
                .try_fold(Statistics::default(), |mut stats, res| {
                    let (status, id) = res?;
                    match status {
                        Some(BuildStatus::Aborted) => stats.aborted.push(id),
                        Some(BuildStatus::Failure) => stats.failures.push(id),
                        Some(BuildStatus::NotBuilt) => stats.not_built.push(id),
                        Some(BuildStatus::Success) => stats.successful.push(id),
                        Some(BuildStatus::Unstable) => stats.unstable.push(id),
                        _ => {}
                    };

                    Ok::<_, rusqlite::Error>(stats)
                })
            })
            .await?;

        stats.successful_jobs = db
            .call(|conn| {
                conn.prepare(
                    "
                    SELECT COUNT(*) FROM jobs
                    WHERE id IN (
                            SELECT job_id FROM builds
                            GROUP BY job_id
                            HAVING MAX(number) AND status = ?
                        )
                    ",
                )?
                .query_one((write_value!(BuildStatus::Success),), |row| row.get(0))
            })
            .await?;

        stats.total_jobs = db
            .call(|conn| {
                conn.prepare("SELECT COUNT(*) FROM jobs")?
                    .query_one((), |row| row.get(0))
            })
            .await?;

        // don't count metadata issues in total
        stats.issues_found = db
            .call(|conn| {
                conn.prepare(
                    "
                    SELECT COUNT(*) FROM issues
                    JOIN tags ON tags.id = issues.tag_id
                    JOIN runs ON runs.id = issues.run_id
                    WHERE tags.severity != ? AND runs.build_id IN (
                            SELECT id FROM builds
                            GROUP BY job_id
                            HAVING MAX(number)
                        )
                    ",
                )?
                .query_one((write_value!(Severity::Metadata),), |row| row.get(0))
            })
            .await?;

        stats.unknown_runs = db
            .call(|conn| {
                conn.prepare(
                    "
                    SELECT r.id FROM runs r
                    WHERE (
                            r.status = ?
                            OR r.status = ?
                            OR r.status = ?
                        ) AND r.build_id IN (
                            SELECT id FROM builds
                            GROUP BY job_id
                            HAVING MAX(number)
                        ) AND NOT EXISTS (
                            SELECT 1 FROM issues
                            JOIN tags ON tags.id = issues.tag_id
                            WHERE
                                issues.run_id = r.id
                                AND tags.severity != ?
                        )
                    ",
                )?
                .query_map(
                    (
                        write_value!(Some(BuildStatus::Failure)),
                        write_value!(Some(BuildStatus::Unstable)),
                        write_value!(Some(BuildStatus::Aborted)),
                        write_value!(Severity::Metadata),
                    ),
                    |row| row.get(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await?;

        Ok(stats)
    }
}
