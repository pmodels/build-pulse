//! Structs and methods to interface with Jenkins via the [jenkins_api] crate.
use anyhow::{Error, Result};
use jenkins_api::{
    Jenkins,
    build::{Build, BuildStatus, ShortBuild},
    client::{Path, TreeBuilder},
    job::Job,
};
use serde::Deserialize;

use crate::db::{JobBuild, Run};

/// Represents all jobs pulled from [SparseMatrixProject::pull_jobs]
#[derive(Deserialize)]
pub struct SparseMatrixProject {
    /// [Vec] of [SparseJob]s
    pub jobs: Vec<SparseJob>,
}

/// Represents a job pulled from [SparseMatrixProject::pull_jobs]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseJob {
    /// Name of the job
    pub name: String,

    /// URL of the job
    pub url: String,

    /// Last build of job as a [SparseBuild]
    pub builds: Vec<SparseBuild>,
}

/// Represents a job build pulled from [SparseMatrixProject::pull_jobs]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparseBuild {
    /// Build number
    pub number: u32,

    /// Build URL
    pub url: String,

    /// Build timestamp
    pub timestamp: u64,

    /// Build result as a [BuildStatus]
    pub result: Option<BuildStatus>,

    /// Build runs as a [Vec] of [ShortBuild]s
    pub runs: Option<Vec<ShortBuild>>,
}

/// Builds that can be represented as [Run]
pub trait AsRun {
    /// Convert `&self` to [Run]
    async fn as_run(&self, build_id: i64, jenkins_client: &Jenkins) -> Run;
}

/// Builds that can be represented as [JobBuild]
pub trait AsBuild {
    /// Convert `&self` to [JobBuild]
    fn as_build(&self, job_id: i64) -> JobBuild;
}

/// Jobs that can be represented as [Job]
pub trait AsJob {
    /// Convert `&self` to [Job]
    fn as_job(&self, last_n: usize) -> crate::db::Job;
}

/// [Build]s with common fields
pub trait HasBuildFields {
    /// Get [BuildStatus]
    fn build_status(&self) -> Option<BuildStatus>;

    /// Get `display_name`
    fn full_display_name_or_default(&self) -> &str;
}

/// Works for most [jenkins_api::build] structs
macro_rules! impl_HasBuildFields {
    (for $($t:ty),+) => {
        $(impl HasBuildFields for $t {
            fn build_status(&self) -> Option<BuildStatus> {
                self.result
            }

            fn full_display_name_or_default(&self) -> &str {
                self.full_display_name
                    .as_ref()
                    .unwrap_or(&self.display_name)
            }
        })*
    }
}

impl_HasBuildFields!(for jenkins_api::build::CommonBuild);

impl Job for SparseJob {
    fn name(&self) -> &str {
        &self.name
    }
    fn url(&self) -> &str {
        &self.url
    }
}

impl AsJob for SparseJob {
    fn as_job(&self, last_n: usize) -> crate::db::Job {
        crate::db::Job {
            name: self.name.as_str().into(),
            last_build: self.builds.iter().take(last_n).last().map(|b| b.number),
            url: self.url.as_str().into(),
        }
    }
}

impl AsBuild for SparseBuild {
    fn as_build(&self, job_id: i64) -> JobBuild {
        JobBuild {
            url: self.url.clone(),
            number: self.number,
            status: self.result,
            timestamp: self.timestamp,
            job_id,
        }
    }
}

impl<T> AsRun for T
where
    T: Build + HasBuildFields,
{
    async fn as_run(&self, build_id: i64, jenkins_client: &Jenkins) -> Run {
        let display_name = self.full_display_name_or_default();
        let status = self.build_status();
        Run {
            url: self.url().into(),
            status,
            display_name: display_name.into(),
            log: match status {
                Some(BuildStatus::Failure | BuildStatus::Unstable | BuildStatus::Aborted) => {
                    // only get log on failure
                    match self.get_console(jenkins_client).await {
                        Ok(l) => Some(l.into()),
                        Err(e) => {
                            log::error!("Failed to retrieve build log for run {display_name}: {e}");
                            None
                        }
                    }
                }
                _ => None,
            },
            tag_schema: None,
            build_id,
        }
    }
}

impl SparseMatrixProject {
    /// Query the Jenkins build server for all jobs and their last build from a `project_name`
    pub async fn pull_jobs(client: &Jenkins, project_name: &str) -> Result<Self> {
        client
            .get_object_as(
                Path::View { name: project_name },
                TreeBuilder::new()
                    .with_field(
                        TreeBuilder::object("jobs")
                            .with_subfield("name")
                            .with_subfield("url")
                            .with_subfield(
                                TreeBuilder::object("builds")
                                    .with_subfield("number")
                                    .with_subfield("url")
                                    .with_subfield("displayName")
                                    .with_subfield("timestamp")
                                    .with_subfield("result")
                                    .with_subfield(
                                        TreeBuilder::object("runs")
                                            .with_subfield("url")
                                            .with_subfield("number"),
                                    ),
                            ),
                    )
                    .build(),
            )
            .await
            .map_err(Error::from_boxed)
    }
}
