use arcstr::{ArcStr, Substr};

use crate::{
    config::Severity,
    db::{Artifact, Queryable, Run},
    schema, write_value,
};

/// [Issue] stored in [super::Database]
pub struct IssueInfo {
    pub snippet_start: usize,
    pub snippet_end: usize,
    pub run_id: i64,
    pub artifact_id: Option<i64>,
    pub tag_id: i64,
    pub duplicates: u64,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Issue {
    /// String snippet from [Run]
    pub snippet: Substr,

    /// [crate::parse::Tag] associated with [Issue]
    pub tag_id: i64,

    /// Number of duplicate emits in the same [Run]
    pub duplicates: u64,
}

schema! {
    issues for IssueInfo {
        id              INTEGER PRIMARY KEY,
        snippet_start   INTEGER NOT NULL,
        snippet_end     INTEGER NOT NULL,
        run_id          INTEGER NOT NULL REFERENCES runs(id),
        artifact_id     INTEGER REFERENCES artifacts(id),
        tag_id          INTEGER NOT NULL REFERENCES tags(id),
        duplicates      INTEGER NOT NULL
    }
}

impl Queryable for IssueInfo {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            Self {
                snippet_start: row.get(1).map(isize::cast_unsigned)?,
                snippet_end: row.get(2).map(isize::cast_unsigned)?,
                run_id: row.get(3)?,
                artifact_id: row.get(4)?,
                tag_id: row.get(5)?,
                duplicates: row.get(6).map(i64::cast_unsigned)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            self.snippet_start.cast_signed(),
            self.snippet_end.cast_signed(),
            self.run_id,
            self.artifact_id,
            self.tag_id,
            self.duplicates.cast_signed(),
        ))
    }
}

impl IssueInfo {
    /// Get all [Issue]s from [super::Database] by [Run]
    pub async fn select_all_by_run(
        db: &super::Database,
        run: &super::InDatabase<Run>,
        include_metadata: bool,
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let id = run.id;
        if include_metadata {
            db.call(move |conn| {
                conn.prepare_cached(
                    "
                    SELECT
                        id,
                        snippet_start,
                        snippet_end,
                        run_id,
                        artifact_id,
                        tag_id,
                        duplicates
                    FROM issues
                    WHERE run_id = ?
                    ",
                )?
                .query_map((id,), Self::map_row)?
                .collect()
            })
            .await
        } else {
            db.call(move |conn| {
                conn.prepare_cached(
                    "
                    SELECT
                        issues.id,
                        snippet_start,
                        snippet_end,
                        run_id,
                        artifact_id,
                        tag_id,
                        duplicates
                    FROM issues
                    JOIN tags ON tags.id = issues.tag_id
                    WHERE issues.run_id = ?
                    AND tags.severity != ?
                    ",
                )?
                .query_map((id, write_value!(Severity::Metadata)), Self::map_row)?
                .collect()
            })
            .await
        }
    }

    /// Remove all [Issue]s with an outdated [crate::parse::TagSet] schema from [super::Database]
    pub async fn delete_all_invalid_by_tag_schema(
        db: &super::Database,
        current_schema: u64,
    ) -> rusqlite::Result<usize> {
        db.call(move |conn| {
            let mut tx = conn.transaction()?;
            tx.set_drop_behavior(rusqlite::DropBehavior::Commit);

            // delete similarities first
            tx.execute(
                "
                DELETE FROM similarities WHERE similarity_hash IN (
                    SELECT DISTINCT similarities.similarity_hash FROM similarities
                    JOIN issues ON issues.id = similarities.issue_id
                    JOIN runs ON runs.id = issues.run_id
                    WHERE runs.tag_schema != ?
                )
                ",
                (current_schema.cast_signed(),),
            )?;

            // then issues
            tx.execute(
                "
                DELETE FROM issues WHERE id IN (
                    SELECT i.id FROM issues i
                    JOIN runs r ON i.run_id = r.id
                    WHERE r.tag_schema != ?
                )
                ",
                (current_schema.cast_signed(),),
            )?;

            // also set the run tag_schema to NULL to indicate an unparsed run
            tx.execute(
                "UPDATE runs SET tag_schema = NULL WHERE tag_schema != ?",
                (current_schema.cast_signed(),),
            )
        })
        .await
    }
}

impl super::InDatabase<IssueInfo> {
    pub fn into_issue(self, field: &ArcStr) -> super::InDatabase<Issue> {
        let super::InDatabase {
            id,
            item:
                IssueInfo {
                    snippet_start,
                    snippet_end,
                    tag_id,
                    duplicates,
                    ..
                },
        } = self;
        super::InDatabase::new(
            id,
            Issue {
                snippet: field.substr(snippet_start..snippet_end),
                tag_id,
                duplicates,
            },
        )
    }
}

impl Issue {
    pub fn into_issue_info(
        self,
        run: &super::InDatabase<Run>,
        artifact: Option<&super::InDatabase<Artifact>>,
    ) -> IssueInfo {
        let core::ops::Range::<_> { start, end } = self.snippet.range();
        IssueInfo {
            snippet_start: start,
            snippet_end: end,
            run_id: run.id,
            artifact_id: artifact.map(|a| a.id),
            tag_id: self.tag_id,
            duplicates: self.duplicates,
        }
    }
}
