use std::{fmt, ops::Deref};

use chumsky::{pratt::*, prelude::*};
use regex::Regex;
use rusqlite::{Error, ToSql};
use serde_json::to_value;

use crate::{config::Severity, db::TagInfo};

/// Represents an expression
#[derive(Clone)]
pub enum TagExpr {
    Not(Box<Self>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    TagSet(Regex),
    SeveritySet(Severity),
    Tag(Regex),
    Severity(Severity),
}

impl TagExpr {
    pub fn parse(expr: &'_ str) -> Result<Self, Vec<Rich<'_, char>>> {
        let tag_pattern = none_of::<_, _, extra::Err<Rich<char>>>('"')
            .repeated()
            .to_slice()
            .try_map(|p, span| Regex::new(p).map_err(|e| Rich::custom(span, e)))
            .delimited_by(just('"'), just('"'));
        let severity_const = none_of('"')
            .repeated()
            .to_slice()
            .try_map(|s, span| {
                Severity::iter()
                    .find(|e| e.to_string() == s)
                    .ok_or(Rich::custom(
                        span,
                        "Failed to parse Severity as enum variant!",
                    ))
            })
            .delimited_by(just('"'), just('"'));

        let tag_set = just("T").ignore_then(tag_pattern).map(TagExpr::TagSet);
        let severity_set = just("S")
            .ignore_then(severity_const)
            .map(TagExpr::SeveritySet);

        let tag = just("t").ignore_then(tag_pattern).map(TagExpr::Tag);
        let severity = just("s").ignore_then(severity_const).map(TagExpr::Severity);

        recursive(|atom| {
            choice((
                atom.delimited_by(just('('), just(')')),
                tag_set,
                severity_set,
                tag,
                severity,
            ))
            .padded()
            .pratt((
                prefix(2, just('!'), |_, e, _| TagExpr::Not(Box::new(e))),
                infix(left(1), just("&&"), |l, _, r, _| {
                    TagExpr::And(Box::new(l), Box::new(r))
                }),
                infix(left(0), just("||"), |l, _, r, _| {
                    TagExpr::Or(Box::new(l), Box::new(r))
                }),
            ))
        })
        .parse(expr)
        .into_result()
    }

    pub fn eval_rows<T: Deref<Target = TagInfo>>(self, tags: &[T]) -> Vec<TagExpr> {
        let tag_to_set = |p: Regex, invert| {
            tags.iter()
                .filter(|t| p.is_match(&t.name) ^ invert)
                .map(|t| TagExpr::Tag(Regex::new(&t.name).unwrap()))
                .collect()
        };
        let severity_to_set = |s, invert| {
            tags.iter()
                .filter(|t| (t.severity == s) ^ invert)
                .map(|t| TagExpr::Tag(Regex::new(&t.name).unwrap()))
                .collect()
        };

        match self {
            TagExpr::Not(e) => match *e {
                TagExpr::Not(inner) => inner.eval_rows(tags),
                TagExpr::And(l, r) => {
                    TagExpr::Or(TagExpr::Not(l).into(), TagExpr::Not(r).into()).eval_rows(tags)
                }
                TagExpr::Or(l, r) => {
                    TagExpr::And(TagExpr::Not(l).into(), TagExpr::Not(r).into()).eval_rows(tags)
                }
                TagExpr::TagSet(p) => tag_to_set(p, true),
                TagExpr::SeveritySet(s) => severity_to_set(s, true),
                TagExpr::Tag(_) | TagExpr::Severity(_) => vec![TagExpr::Not(e)],
            },
            TagExpr::And(l, r) => {
                let l_rows = l.eval_rows(tags);
                let r_rows = r.eval_rows(tags);

                r_rows
                    .into_iter()
                    .flat_map(|y| {
                        l_rows
                            .clone()
                            .into_iter()
                            .map(move |x| TagExpr::And(x.clone().into(), y.clone().into()))
                    })
                    .collect()
            }
            TagExpr::Or(l, r) => {
                let l_rows = l.eval_rows(tags);
                let r_rows = r.eval_rows(tags);

                r_rows
                    .into_iter()
                    .flat_map(|y| {
                        l_rows
                            .clone()
                            .into_iter()
                            .map(move |x| TagExpr::Or(x.clone().into(), y.clone().into()))
                    })
                    .collect()
            }
            TagExpr::TagSet(p) => tag_to_set(p, false),
            TagExpr::SeveritySet(s) => severity_to_set(s, false),
            TagExpr::Tag(_) | TagExpr::Severity(_) => vec![self],
        }
    }

    pub fn to_sql_select(&self) -> Result<(String, Vec<Box<dyn ToSql + Send>>), Error> {
        // FIXME: avoid all these allocations when making the select!
        fn to_where_expr(expr: &TagExpr) -> Result<(String, Vec<Box<dyn ToSql + Send>>), Error> {
            match expr {
                TagExpr::Not(e) => {
                    let (expr, params) = to_where_expr(e)?;

                    Ok((format!("NOT ({expr})"), params))
                }
                TagExpr::And(l, r) => {
                    let (l_expr, mut l_params) = to_where_expr(l)?;
                    let (r_expr, mut r_params) = to_where_expr(r)?;
                    l_params.append(&mut r_params);

                    Ok((format!("({l_expr}) AND ({r_expr})"), l_params))
                }
                TagExpr::Or(l, r) => {
                    let (l_expr, mut l_params) = to_where_expr(l)?;
                    let (r_expr, mut r_params) = to_where_expr(r)?;
                    l_params.append(&mut r_params);

                    Ok((format!("({l_expr}) OR ({r_expr})"), l_params))
                }
                TagExpr::Tag(p) => Ok((
                    "
                    EXISTS (
                        SELECT 1 FROM issues
                        JOIN tags ON tags.id = issues.tag_id
                        WHERE issues.run_id = runs.id AND tags.name REGEXP ?
                    )
                    "
                    .into(),
                    vec![Box::new(p.as_str().to_owned())],
                )),
                TagExpr::Severity(s) => Ok((
                    "
                    EXISTS (
                        SELECT 1 FROM issues
                        JOIN tags ON tags.id = issues.tag_id
                        WHERE issues.run_id = runs.id AND tags.severity = ?
                    )
                    "
                    .into(),
                    vec![Box::new(to_value(s).map_err(|_| Error::InvalidQuery)?)],
                )),
                _ => Err(Error::InvalidQuery),
            }
        }

        let (where_expr, params) = to_where_expr(self)?;
        Ok((
            format!(
                "
                SELECT DISTINCT id FROM runs
                WHERE {where_expr}
                    AND build_id IN (
                        SELECT id FROM builds
                        GROUP BY job_id
                        HAVING MAX(number)
                    )
                "
            ),
            params,
        ))
    }
}

impl fmt::Display for TagExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagExpr::Not(e) => write!(f, "!{e}"),
            TagExpr::And(l, r) => write!(f, "{l} && {r}"),
            TagExpr::Or(l, r) => write!(f, "{l} || {r}"),
            TagExpr::TagSet(p) => write!(f, "{{{p}}}"),
            TagExpr::SeveritySet(s) => write!(f, "{{{s}}}"),
            TagExpr::Tag(p) => write!(f, "{p}"),
            TagExpr::Severity(s) => write!(f, "{s}"),
        }
    }
}
