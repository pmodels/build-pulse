use arcstr::ArcStr;

use crate::{
    config::{ConfigTag, Field, Severity},
    db::{Queryable, Run, Upsertable},
    read_value, schema, write_value,
};

#[derive(PartialEq, Eq, Hash)]
pub struct TagInfo {
    /// Name of [Tag]
    pub name: ArcStr,

    /// Description of [Tag]
    pub desc: ArcStr,

    /// Field of [Tag]
    pub field: Field,

    /// Severity of [Tag]
    pub severity: Severity,
}

schema! {
    tags for TagInfo {
        id              INTEGER PRIMARY KEY,
        name            TEXT NOT NULL UNIQUE,
        desc            TEXT NOT NULL,
        field           TEXT NOT NULL,
        severity        TEXT NOT NULL
    }
}

impl From<ConfigTag> for TagInfo {
    fn from(value: ConfigTag) -> Self {
        Self {
            name: value.name.into(),
            desc: value.desc.into(),
            field: value.from,
            severity: value.severity,
        }
    }
}

impl Queryable for TagInfo {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            Self {
                name: row.get::<_, String>(1)?.into(),
                desc: row.get::<_, String>(2)?.into(),
                field: read_value!(row, 3),
                severity: read_value!(row, 4),
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((
            self.name.as_str(),
            self.desc.as_str(),
            write_value!(self.field),
            write_value!(self.severity),
        ))
    }
}

impl Upsertable for TagInfo {
    async fn upsert(self, db: &super::Database) -> rusqlite::Result<super::InDatabase<Self>> {
        let name = self.name.clone();
        db.call(move |conn| {
            conn.prepare_cached(
                "
                INSERT INTO tags (name, desc, field, severity) VALUES (?, ?, ?, ?)
                    ON CONFLICT(name) DO UPDATE SET
                        desc = excluded.desc,
                        field = excluded.field,
                        severity = excluded.severity
                ",
            )?
            .execute(self.as_params()?)
        })
        .await?;

        Self::select_one_by_name(db, name).await
    }
}

impl TagInfo {
    /// Get a [TagInfo] from [super::Database] by name
    pub async fn select_one_by_name(
        db: &super::Database,
        name: ArcStr,
    ) -> rusqlite::Result<super::InDatabase<Self>> {
        db.call(move |conn| {
            conn.prepare_cached("SELECT * FROM tags WHERE name = ?")?
                .query_one((name.as_str(),), Self::map_row)
        })
        .await
    }

    /// Get all [TagInfo]s from [Run]
    pub async fn select_all_by_run(
        db: &super::Database,
        run: &super::InDatabase<Run>,
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        let id = run.id;
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT DISTINCT tags.id, name, desc, field, severity FROM tags
                JOIN issues ON tags.id = issues.tag_id
                WHERE issues.run_id = ?
                ",
            )?
            .query_map((id,), Self::map_row)?
            .collect()
        })
        .await
    }

    /// Remove all [Tag]s which aren't referenced by [super::Issue]s from [super::Database]
    pub async fn delete_all_orphan(db: &super::Database) -> rusqlite::Result<usize> {
        db.call(|conn| {
            conn.execute(
                "
                DELETE FROM tags WHERE NOT EXISTS (
                    SELECT 1 FROM issues
                    WHERE tags.id = issues.tag_id
                )
                ",
                (),
            )
        })
        .await
    }
}
