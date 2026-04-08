use std::{
    collections::{HashMap, HashSet},
    str::from_utf8,
};

use arcstr::{ArcStr, Substr};
use futures::{TryFutureExt, TryStreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::{
    config::Field,
    db::{Artifact, InDatabase, IssueInfo, Queryable, Run, TagInfo},
    schema,
};

pub struct SimilarityInfo {
    pub similarity_hash: u64,
    pub issue_id: i64,
}

/// List of similar [Run]s by [TagInfo] in [super::Database]
pub struct Similarity {
    pub tag: InDatabase<TagInfo>,
    pub related: HashSet<i64>,
    pub example: Substr,
}

schema! {
    similarities for SimilarityInfo {
        id              INTEGER PRIMARY KEY,
        similarity_hash INTEGER NOT NULL,
        issue_id        INTEGER NOT NULL REFERENCES issues(id)
    }
}

impl Queryable for SimilarityInfo {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<InDatabase<Self>> {
        Ok(InDatabase::new(
            row.get(0)?,
            Self {
                similarity_hash: row.get(1).map(i64::cast_unsigned)?,
                issue_id: row.get(2)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((self.similarity_hash.cast_signed(), self.issue_id))
    }
}

impl Similarity {
    /// Get all similarities by [crate::parse::Tag] in [super::Database]
    pub async fn query_all(db: &super::Database) -> rusqlite::Result<impl Iterator<Item = Self>> {
        struct InfoSet {
            similarity: SimilarityInfo,
            tag_id: i64,
            run_id: i64,
        }

        // TODO: stream this!
        db.call(|conn| -> rusqlite::Result<UnboundedReceiverStream<_>> {
            let (tx, rx) = mpsc::unbounded_channel();
            conn.prepare_cached(
                "
                SELECT DISTINCT
                    s.similarity_hash,
                    s.issue_id,
                    i.tag_id,
                    i.run_id
                FROM similarities s
                JOIN issues i ON i.id = s.issue_id
                WHERE EXISTS (
                        SELECT 1 FROM similarities
                        JOIN issues ON issues.id = similarities.issue_id
                        JOIN runs ON runs.id = issues.run_id
                        WHERE similarity_hash = s.similarity_hash
                            AND build_id IN (
                                    SELECT id FROM builds
                                    GROUP BY job_id
                                    HAVING MAX(number)
                                )
                    )
                ",
            )?
            .query_and_then((), |row| {
                Ok(InfoSet {
                    similarity: SimilarityInfo {
                        similarity_hash: row.get(0).map(i64::cast_unsigned)?,
                        issue_id: row.get(1)?,
                    },
                    tag_id: row.get(2)?,
                    run_id: row.get(3)?,
                })
            })?
            .try_for_each(|info| {
                tx.send(info)
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
            })?;

            Ok(rx.into())
        })
        .await?
        .try_fold(
            HashMap::new(),
            |mut hm,
             InfoSet {
                 similarity:
                     SimilarityInfo {
                         similarity_hash,
                         issue_id,
                     },
                 tag_id,
                 run_id,
             }| async move {
                let tag = TagInfo::select_one(db, tag_id).await?;
                let field = tag.field;
                hm.entry(similarity_hash)
                    .or_insert({
                        Self {
                            tag,
                            related: HashSet::new(),
                            example: IssueInfo::select_one(db, issue_id)
                                .and_then(|i| async {
                                    match field {
                                        Field::Console => Run::select_one(db, run_id)
                                            .await?
                                            .item()
                                            .log
                                            .map_or(Err(rusqlite::Error::InvalidQuery), |l| {
                                                Ok(i.into_issue(&l))
                                            }),
                                        Field::RunName => Ok(i.into_issue(
                                            &Run::select_one_display_name(db, run_id).await?.into(),
                                        )),
                                        Field::Artifact => {
                                            let artifact_id = i
                                                .artifact_id
                                                .ok_or(rusqlite::Error::InvalidQuery)?;
                                            Ok(i.into_issue(
                                                // TODO: share Vec<u8> contents somehow?
                                                &from_utf8(
                                                    &Artifact::select_one(db, artifact_id)
                                                        .await?
                                                        .contents,
                                                )
                                                .map(ArcStr::from)?,
                                            ))
                                        }
                                    }
                                })
                                .map_ok(|i| i.item().snippet)
                                .await?,
                        }
                    })
                    .related
                    .insert(run_id);

                Ok::<_, rusqlite::Error>(hm)
            },
        )
        .map_ok(|hm| hm.into_values())
        .await
    }
}
