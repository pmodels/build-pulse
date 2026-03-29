//! HTML report generation using [maud] templating.
use std::{collections::HashMap, str::from_utf8_unchecked, time::SystemTime};

use anyhow::{Error, Result};
use jenkins_api::build::BuildStatus;
use maud::{DOCTYPE, Markup, html};
use time::{OffsetDateTime, UtcOffset, macros::format_description};

use crate::{
    config::{Severity, TagView},
    db::{
        Artifact, BlobFormat, Database, InDatabase, Issue, Job, JobBuild, Queryable, Run,
        Similarity, Statistics, TagInfo,
    },
    tag_expr::TagExpr,
};

/// Format `time` as a [String]
#[inline]
fn format_timestamp<T>(time: T) -> Result<String>
where
    T: Into<OffsetDateTime>,
{
    time.into()
        .format(
            format_description!("[month repr:short] [day], [year] [hour repr:12]:[minute]:[second] [period] UTC[offset_hour padding:none sign:mandatory]")
        )
        .map_err(Error::from)
}

/// Format [`Option<BuildStatus>`] to string
#[inline]
fn status_as_str(status: Option<BuildStatus>) -> &'static str {
    match status {
        Some(BuildStatus::Success) => "good",
        Some(BuildStatus::Failure) => "bad",
        Some(BuildStatus::Unstable) => "unstable",
        Some(BuildStatus::NotBuilt) => "not built",
        Some(BuildStatus::Aborted) => "aborted",
        None => "no build",
    }
}

/// Format [`Option<BuildStatus>`] as class name
#[inline]
fn status_as_class(status: Option<BuildStatus>) -> Option<&'static str> {
    match status {
        Some(BuildStatus::Failure) => Some("error"),
        Some(BuildStatus::Unstable) => Some("warning"),
        Some(BuildStatus::Aborted) => Some("info"),
        _ => None,
    }
}

/// Format [Severity] as class name
#[inline]
fn severity_as_class(severity: Severity) -> Option<&'static str> {
    match severity {
        Severity::Error => Some("error"),
        Severity::Warning => Some("warning"),
        Severity::Info => Some("info"),
        _ => None,
    }
}

/// Render a [crate::api::SparseJob]
fn render_job(job: &InDatabase<Job>, db: &Database, tz: UtcOffset) -> Result<Markup> {
    Ok(html! {
        h2 {
            a href=(job.url) {
                (job.name)
            }
        }
        @if let Some((last_build, rest)) = JobBuild::select_all_by_job(db, job.id, ())?.split_first() {
            (render_build(&last_build, db, tz, true)?)
            @for build in rest {
                (render_build(&build, db, tz, false)?)
            }
        } @else {
            p {
                "No builds available."
            }
        }
    })
}

/// Render a [JobBuild]
fn render_build(
    build: &InDatabase<JobBuild>,
    db: &Database,
    tz: UtcOffset,
    latest: bool,
) -> Result<Markup> {
    let mut runs = Run::select_all_by_build(db, &build, ())?;
    runs.sort_by_cached_key(|r| match r.status {
        Some(BuildStatus::Failure) => 0,
        Some(BuildStatus::Unstable) => 1,
        Some(BuildStatus::Aborted) => 2,
        Some(BuildStatus::Success) => 3,
        Some(BuildStatus::NotBuilt) => 4,
        None => 4,
    });
    Ok(html! {
        details open[latest && matches!(build.status, Some(BuildStatus::Failure | BuildStatus::Unstable | BuildStatus::Aborted))] {
            summary {
                @if latest {
                    b {
                        "Latest: "
                    }
                }
                a href=(build.url) {
                    "Build #"
                    (build.number)
                }
                " on "
                i {
                    (format_timestamp(
                        OffsetDateTime::from_unix_timestamp(
                            (build.timestamp/1000).cast_signed()
                        )?
                        .to_offset(tz)
                    )?)
                }
                " was "
                b {
                    (status_as_str(build.status))
                }
            }
            @for run in runs {
                (render_run(&run, db)?)
                br;
            }
        }
    })
}

/// Render a [Run]
fn render_run(run: &InDatabase<Run>, db: &Database) -> Result<Markup> {
    let issues = Issue::select_all_not_metadata(db, (db, run))?;
    Ok(html! {
        table {
            tr #(run.id) class=[status_as_class(run.status)] {
                td rowspan="2" { // status
                    b {
                        (status_as_str(run.status))
                    }
                }
                td rowspan="2" { // name
                    a href=(run.url) {
                        (run.display_name)
                    }
                }
                td { // name
                    b {
                        "Identified Tags"
                    }
                }
            }
            tr #(run.id) class=[status_as_class(run.status)] {
                td { // tags
                    @let tags = TagInfo::select_all_by_run(db, run, ())?;
                    @if !tags.is_empty() {
                        @for t in tags {
                            code title=(t.desc) {
                                (t.name)
                                ", "
                            }
                        }
                    } @else {
                        i {
                            "untagged!"
                        }
                    }
                }
            }
            @if !issues.is_empty() {
                @for i in issues {
                    tr class=[status_as_class(run.status)] {
                        td colspan="3" { // issues
                            pre {
                                (i.snippet)
                            }
                            @if i.duplicates > 0 {
                                b {
                                    (i.duplicates)
                                    " duplicate emits"
                                }
                            }
                        }
                    }
                }
            } @else if matches!(
                run.status,
                Some(
                    BuildStatus::Failure
                    | BuildStatus::Unstable
                    | BuildStatus::Aborted
                ),
            ) {
                tr class=[status_as_class(run.status)] {
                    td colspan="3" { // issues
                        b {
                            "Unknown issue(s)!"
                        }
                    }
                }
            }
            tr class=[status_as_class(run.status)] {
                td colspan="3" { // issues
                    a href={(run.url) "/consoleFull"} {
                        "Full Build Log"
                    }
                }
            }
            @let artifacts = Artifact::select_all_by_run(db, run.id, ())?;
            @for a in artifacts {
                tr class=[status_as_class(run.status)] {
                    td colspan="3" { // artifacts
                        details {
                            summary {
                                b {
                                    (a.path)
                                }
                            }
                            @match a.blob_format() {
                                BlobFormat::Png | BlobFormat::Svg => img src={"artifacts/" (a.id)};,
                                BlobFormat::Utf8 => pre { (unsafe {
                                    // SAFETY: `blob_format` checks if contents is valid UTF-8
                                    from_utf8_unchecked(&a.contents)
                                }) },
                                BlobFormat::Unknown => i { "can't display" },
                                BlobFormat::Null => i { "no data" },
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Render [crate::db::Statistics]
fn render_stats(db: &Database) -> Result<Markup> {
    let stats = Statistics::query(db)?;
    Ok(html! {
        h3 {
            "Job Statistics"
        }
        p {
            "Overall Job Health:"
            progress value=(stats.successful_jobs) max=(stats.total_jobs) {}
            br;
            (stats.successful_jobs)
            " out of "
            (stats.total_jobs)
            " jobs successful."
        }

        h4 {
            "Latest Run Statuses"
        }
        table class="view" {
            tr {
                td {
                    "Failures"
                }
                td {
                    (render_run_ids(stats.failures.iter(), db)?)
                }
            }
            tr {
                td {
                    "Unstable"
                }
                td {
                    (render_run_ids(stats.unstable.iter(), db)?)
                }
            }
            tr {
                td {
                    "Healthy"
                }
                td {
                    (render_run_ids(stats.successful.iter(), db)?)
                }
            }
            tr {
                td {
                    "Aborted"
                }
                td {
                    (render_run_ids(stats.aborted.iter(), db)?)
                }
            }
            tr {
                td {
                    "Not Built"
                }
                td {
                    (render_run_ids(stats.not_built.iter(), db)?)
                }
            }
            tr {
                td {
                    b {
                        "Total"
                    }
                }
                td {
                    b {
                        (stats.successful.len()
                         + stats.failures.len()
                         + stats.unstable.len()
                         + stats.aborted.len()
                         + stats.not_built.len())
                        " runs"
                    }
                }
            }
        }
        br;
        table class="view" {
            tr {
                td {
                    b {
                        "Issues Found"
                    }
                }
                td {
                    b {
                        (stats.issues_found)
                        " issues"
                    }
                }
            }
            tr {
                td {
                    b {
                        "Unknown Issues"
                    }
                }
                td {
                    b {
                        (render_run_ids(stats.unknown_runs.iter(), db)?)
                    }
                }
            }
        }
    })
}

/// Render [crate::db::Similarity]
fn render_similarities(db: &Database) -> Result<Markup> {
    let similarities: HashMap<_, Vec<_>> =
        Similarity::query_all(db, ())?
            .into_iter()
            .fold(HashMap::new(), |mut acc, s| {
                acc.entry(s.tag.severity).or_default().push(s);

                acc
            });

    Ok(html! {
        h4 {
            "Related Issues by Severity"
        }
        @for severity in crate::config::Severity::iter().rev() {
            @if let Some(similarities) = similarities.get(&severity)
                && !similarities.is_empty() {
                details open[matches!(severity, crate::config::Severity::Error)] {
                    summary {
                        (severity)
                        " - "
                        i {
                            (similarities.len())
                            " group(s)"
                        }
                    }
                    @for s in similarities {
                        table {
                            tr class=[severity_as_class(s.tag.severity)] {
                                td {
                                    code title=(s.tag.desc) {
                                        (s.tag.name)
                                    }
                                }
                                td {
                                    (render_run_ids(s.related.iter(), db)?)
                                }
                            }
                            tr class=[severity_as_class(s.tag.severity)] {
                                td colspan="2" {
                                    b {
                                        "Example Snippet"
                                    }
                                    hr;
                                    pre {
                                        (s.example)
                                    }
                                }
                            }
                        }
                        br;
                    }
                }
            }
        }
    })
}

/// Render a [TagView]
fn render_view(view: &TagView, db: &Database) -> Result<Markup> {
    let expr = match TagExpr::parse(&view.expr) {
        Ok(expr) => Ok(expr),
        Err(e) => Err(Error::msg(
            e.iter().fold(String::new(), |acc, e| format!("{acc}\n{e}")),
        )),
    }?;
    let rows = expr.eval_rows(&TagInfo::select_all(db, ())?);

    Ok(html! {
        h4 {
            (view.name)
        }
        table class="view" {
            @for expr in rows {
                @let matches = Run::select_all_id_by_expr(db, &expr)?;
                @if !matches.is_empty() {
                    tr {
                        td {
                            code {
                                (expr)
                            }
                        }
                        td {
                            (render_run_ids(matches.iter(), db)?)
                        }
                    }
                }
            }
        }
    })
}

/// Render a list of [Run] ids as their display name
fn render_run_ids<'a, T>(ids: T, db: &Database) -> Result<Markup>
where
    T: ExactSizeIterator + Iterator<Item = &'a i64>,
{
    Ok(html! {
        @let len = ids.len();
        @if len > 0 {
            details {
                summary {
                    (len)
                    " runs"
                }
                ul {
                    @for id in ids {
                        li {
                            a href={"#" (id)} {
                                (Run::select_one_display_name(db, *id)?)
                            }
                        }
                    }
                }
            }
        } @else {
            "0 runs"
        }
    })
}

/// Render an HTML report for [Database] info
pub fn render(db: &Database, views: &[TagView], tz: UtcOffset) -> Result<Markup> {
    Ok(html! {
        (DOCTYPE)
        html lang="en" {
            head {
                title {
                    "build-pulse report"
                }
                meta charset="utf-8";
                link rel="stylesheet" type="text/css" href="static/style.css";
            }
            body {
                h1 {
                    "build-pulse"
                }
                (render_stats(db)?)
                (render_similarities(db)?)
                @for view in views {
                    (render_view(view, db)?)
                }
                @for job in Job::select_all(db, ())? {
                    (render_job(&job, db, tz)?)
                }
                p {
                    "Report generated on "
                    code {
                        (format_timestamp(
                            OffsetDateTime::from(SystemTime::now())
                            .to_offset(tz)
                        )?)
                    }
                }
            }
        }
    })
}
