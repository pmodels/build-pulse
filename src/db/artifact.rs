use crate::{db::Queryable, schema};

/// [Artifact] stored in [super::Database]
pub struct Artifact {
    /// Path of the [Artifact]
    pub path: String,

    /// Byte contents of [Artifact]
    pub contents: Vec<u8>,

    /// [super::Run] associated with [Artifact]
    pub run_id: i64,
}

/// File type of an [Artifact]'s blob/contents
pub enum BlobFormat {
    Png,
    Svg,
    Unknown,
    Utf8,
    Null,
}

schema! {
    artifacts for Artifact {
        id              INTEGER PRIMARY KEY,
        path            TEXT NOT NULL,
        contents        BLOB NOT NULL,
        run_id          INTEGER NOT NULL REFERENCES runs(id)
    }
}

impl Queryable for Artifact {
    fn map_row(row: &rusqlite::Row) -> rusqlite::Result<super::InDatabase<Self>> {
        Ok(super::InDatabase::new(
            row.get(0)?,
            Artifact {
                path: row.get(1)?,
                contents: row.get(2)?,
                run_id: row.get(3)?,
            },
        ))
    }

    fn as_params(&self) -> rusqlite::Result<impl rusqlite::Params> {
        Ok((&self.path, &self.contents, self.run_id))
    }
}

impl Artifact {
    /// Get all [Artifact] from [super::Database] by [super::Run]
    pub async fn select_all_by_run(
        db: &super::Database,
        run_id: i64,
    ) -> rusqlite::Result<Vec<super::InDatabase<Self>>> {
        db.call(move |conn| {
            conn.prepare_cached(
                "
                SELECT * FROM artifacts
                WHERE run_id = ?
                ",
            )?
            .query_map((run_id,), Self::map_row)?
            .collect()
        })
        .await
    }

    /// Gets the [BlobFormat] of the [Artifact]
    pub fn blob_format(&self) -> BlobFormat {
        match self.contents[..] {
            [] => BlobFormat::Null,
            [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] => BlobFormat::Png, // PNG magic
            _ => match std::str::from_utf8(&self.contents) {
                Ok(blob) if blob.contains("<svg") => BlobFormat::Svg, // SVG XML data
                Ok(_) => BlobFormat::Utf8,
                Err(_) => BlobFormat::Unknown,
            },
        }
    }
}
