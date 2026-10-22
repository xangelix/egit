//! A GitHub Actions workflow file, read for what egit draws: each job's id, its name (as the
//! run's jobs are named after it), what it `needs`, whether it may fail
//! (`continue-on-error: true`), and whether it calls a reusable workflow (`uses`), whose jobs
//! the run names `<caller> / <job>`.
//!
//! Not a YAML parser: a reader of the `jobs:` mapping as workflows write it, by indentation
//! (block scalars such as a step's `run: |` skipped whole, comments left out, keys and values
//! plain or quoted, `needs` a word, a flow list (`[a, b]`, over lines too), or a block list).
//! Anchors and aliases aren't followed: what it can't read is left out, and the graph shows
//! fewer lines, never wrong ones.

use std::collections::HashMap;

use super::{Job, PipelineDetails, grouped};

/// A workflow's jobs, as its file names them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workflow {
    jobs: Vec<WorkflowJob>,
}

/// A job of a workflow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WorkflowJob {
    /// Its id, the key under `jobs:` (what others' `needs` name).
    id: String,
    /// Its `name:`, if it has one (the run's jobs are named after it, else after the id).
    name: Option<String>,
    /// The ids of the jobs it needs.
    needs: Vec<String>,
    /// Whether it may fail without failing the run (`continue-on-error: true`).
    continue_on_error: bool,
    /// Whether it calls a reusable workflow (its jobs named `<it> / <theirs>`).
    uses: bool,
}

/// A line of the file: its indentation and its text without a comment (never blank).
struct Line<'a> {
    indent: usize,
    text: &'a str,
}

impl Workflow {
    /// Reads `text`, a workflow file; nothing where it has no `jobs:`.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let lines = significant(text);
        let Some(start) = lines.iter().position(|line| {
            line.indent == 0 && key_of(line.text).is_some_and(|(key, _)| key == "jobs")
        }) else {
            return Self::default();
        };
        let body: Vec<&Line<'_>> = lines
            .get(start.saturating_add(1)..)
            .unwrap_or_default()
            .iter()
            .take_while(|line| line.indent > 0)
            .collect();
        let Some(job_indent) = body.first().map(|line| line.indent) else {
            return Self::default();
        };
        let mut jobs = Vec::new();
        let mut at = 0;
        while let Some(line) = body.get(at) {
            at = at.saturating_add(1);
            if line.indent != job_indent {
                continue;
            }
            let Some((id, _)) = key_of(line.text) else {
                continue;
            };
            let end = body
                .get(at..)
                .unwrap_or_default()
                .iter()
                .position(|inner| inner.indent <= job_indent)
                .map_or(body.len(), |found| at.saturating_add(found));
            jobs.push(job(&id, body.get(at..end).unwrap_or_default()));
            at = end;
        }
        Self { jobs }
    }

    /// Which of its jobs a run's job named `name` is one of: named after it whole, after it
    /// with a matrix's values (`test (ubuntu-latest, 1.80)`), a reusable workflow's job after
    /// it (`build / compile`), or matching its name's words where it has expressions
    /// (`Test on ${{ matrix.os }}`); the best fit (whole first, then the longest name).
    fn job_of(&self, name: &str) -> Option<usize> {
        let mut best: Option<(usize, (bool, usize))> = None;
        for (at, job) in self.jobs.iter().enumerate() {
            let shown = job.name.as_deref().unwrap_or(&job.id);
            let fit = if shown.contains("${{") {
                fits_expressions(shown, name).then_some((false, shown.len()))
            } else if name == shown {
                Some((true, shown.len()))
            } else {
                let after = name.strip_prefix(shown);
                let matrix = after.is_some_and(|after| after.starts_with(" ("));
                let called = job.uses && after.is_some_and(|after| after.starts_with(" / "));
                (matrix || called).then_some((false, shown.len()))
            };
            if let Some(fit) = fit
                && best.is_none_or(|(_, held)| fit > held)
            {
                best = Some((at, fit));
            }
        }
        best.map(|(at, _)| at)
    }
}

/// The job `id`, from the lines under it.
fn job(id: &str, lines: &[&Line<'_>]) -> WorkflowJob {
    let mut job = WorkflowJob {
        id: id.to_owned(),
        ..WorkflowJob::default()
    };
    let Some(indent) = lines.first().map(|line| line.indent) else {
        return job;
    };
    let mut at = 0;
    while let Some(line) = lines.get(at) {
        at = at.saturating_add(1);
        if line.indent != indent {
            continue;
        }
        let Some((key, value)) = key_of(line.text) else {
            continue;
        };
        match key.as_str() {
            "name" => job.name = Some(scalar(value)).filter(|name| !name.is_empty()),
            "continue-on-error" => job.continue_on_error = scalar(value) == "true",
            "uses" => job.uses = true,
            "needs" => {
                let (needs, taken) = list(value, lines.get(at..).unwrap_or_default(), indent);
                job.needs = needs;
                at = at.saturating_add(taken);
            }
            _ => {}
        }
    }
    job
}

/// A `needs:` value: a word, or a flow list (going on over the lines after where it isn't
/// closed), or, where it is empty, the block list in the lines after (`- a`, as deep as the
/// key or deeper); and how many of those lines it took.
fn list(value: &str, after: &[&Line<'_>], indent: usize) -> (Vec<String>, usize) {
    let value = value.trim();
    if let Some(open) = value.strip_prefix('[') {
        let mut text = open.to_owned();
        let mut taken = 0_usize;
        while !text.contains(']') {
            let Some(line) = after.get(taken) else {
                break;
            };
            text.push(' ');
            text.push_str(line.text);
            taken = taken.saturating_add(1);
        }
        let inside = text.split(']').next().unwrap_or_default();
        let items = inside
            .split(',')
            .map(scalar)
            .filter(|item| !item.is_empty())
            .collect();
        return (items, taken);
    }
    if !value.is_empty() {
        return (vec![scalar(value)], 0);
    }
    let mut items = Vec::new();
    let mut taken = 0_usize;
    for line in after {
        let item = line
            .text
            .strip_prefix('-')
            .filter(|_| line.indent >= indent);
        let Some(item) = item.filter(|item| item.is_empty() || item.starts_with(' ')) else {
            break;
        };
        items.push(scalar(item));
        taken = taken.saturating_add(1);
    }
    items.retain(|item| !item.is_empty());
    (items, taken)
}

/// The lines that say something: each one's indentation and its text without a comment;
/// blank lines, comments, and block scalars' lines (`run: |`, those deeper after it) left out.
fn significant(text: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    // A block scalar's key's indentation, while its lines go on.
    let mut scalar_under: Option<usize> = None;
    for raw in text.lines() {
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let content = raw.trim_start_matches(' ');
        let indent = raw.len().saturating_sub(content.len());
        let content = uncommented(content).trim_end();
        if content.is_empty() {
            continue;
        }
        if let Some(under) = scalar_under {
            if indent > under {
                continue;
            }
            scalar_under = None;
        }
        // `key: |`, `key: >-`, `- run: |2`: the deeper lines after it are its text.
        let value = key_of(content.trim_start_matches("- ")).map(|(_, value)| value.trim());
        if value.is_some_and(|value| value.starts_with('|') || value.starts_with('>')) {
            scalar_under = Some(indent);
        }
        lines.push(Line {
            indent,
            text: content,
        });
    }
    lines
}

/// `text` without a comment: from a `#` that starts it or follows a space, outside quotes.
fn uncommented(text: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut before = ' ';
    for (at, character) in text.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character == '#' && before.is_whitespace() {
            return text.get(..at).unwrap_or(text);
        }
        before = character;
    }
    text
}

/// A mapping's line: its key (unquoted) and what follows its `: ` (none where it isn't one).
fn key_of(text: &str) -> Option<(String, &str)> {
    let (key, value) =
        if let Some(quoted) = text.strip_prefix('"').or_else(|| text.strip_prefix('\'')) {
            let close = text.chars().next()?;
            let end = quoted.find(close)?;
            let key = quoted.get(..end)?;
            let rest = quoted.get(end.saturating_add(1)..)?.strip_prefix(':')?;
            (key, rest)
        } else {
            let at = text.match_indices(':').map(|(at, _)| at).find(|at| {
                text.get(at.saturating_add(1)..)
                    .is_none_or(|rest| rest.is_empty() || rest.starts_with(' '))
            })?;
            (text.get(..at)?, text.get(at.saturating_add(1)..)?)
        };
    let key = key.trim();
    (!key.is_empty() && !key.starts_with('-') && !key.contains(['{', '[']))
        .then(|| (key.to_owned(), value))
}

/// A scalar's text: unquoted (`'it''s'`, `"a \"b\""`), else trimmed.
fn scalar(text: &str) -> String {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    if let Some(inner) = text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    text.to_owned()
}

/// Whether `name` fits `pattern`, a name holding expressions (`Test on ${{ matrix.os }}`):
/// its other words in order, each expression any text.
fn fits_expressions(pattern: &str, name: &str) -> bool {
    let mut parts = Vec::new();
    let mut rest = pattern;
    while let Some(open) = rest.find("${{") {
        parts.push(rest.get(..open).unwrap_or_default());
        rest = rest
            .get(open..)
            .and_then(|from| {
                from.find("}}")
                    .and_then(|close| from.get(close.saturating_add(2)..))
            })
            .unwrap_or_default();
    }
    parts.push(rest);
    let Some((first, others)) = parts.split_first() else {
        return false;
    };
    let Some(mut left) = name.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = others.split_last() else {
        return left.is_empty();
    };
    for part in middle {
        let Some(at) = left.find(part) else {
            return false;
        };
        left = left
            .get(at.saturating_add(part.len())..)
            .unwrap_or_default();
    }
    left.ends_with(last)
}

/// `workflow` laid over `details`' jobs: see [`PipelineDetails::with_workflow`].
pub(super) fn lay_over(details: &mut PipelineDetails, workflow: &Workflow) {
    let mut jobs: Vec<Job> = std::mem::take(&mut details.stages)
        .into_iter()
        .flat_map(|stage| stage.jobs)
        .collect();
    let owners: Vec<Option<usize>> = jobs.iter().map(|job| workflow.job_of(&job.name)).collect();
    // The run's jobs' names, by the workflow's job they are of.
    let mut names_of: HashMap<&str, Vec<String>> = HashMap::new();
    for (job, owner) in jobs.iter().zip(&owners) {
        if let Some(of) = owner.and_then(|owner| workflow.jobs.get(owner)) {
            names_of
                .entry(of.id.as_str())
                .or_default()
                .push(job.name.clone());
        }
    }
    for (job, owner) in jobs.iter_mut().zip(&owners) {
        let Some(of) = owner.and_then(|owner| workflow.jobs.get(owner)) else {
            continue;
        };
        job.needs = Some(
            of.needs
                .iter()
                .flat_map(|need| names_of.get(need.as_str()).into_iter().flatten().cloned())
                .collect(),
        );
        job.allow_failure = job.allow_failure || of.continue_on_error;
    }
    // Each job's stage: the workflow's job it is one of, else its name's group.
    details.stages = grouped(jobs, |job| {
        workflow
            .job_of(&job.name)
            .and_then(|owner| workflow.jobs.get(owner))
            .map_or_else(
                || {
                    let group = super::github::group_of(&job.name).to_owned();
                    (format!("?{group}"), group)
                },
                |of| {
                    let shown = of
                        .name
                        .as_deref()
                        .filter(|name| !name.contains("${{"))
                        .unwrap_or(&of.id);
                    (format!("={}", of.id), shown.to_owned())
                },
            )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job as read: its id, name, needs, `continue-on-error`, and whether it `uses`.
    type Read<'a> = (&'a str, Option<&'a str>, Vec<&'a str>, bool, bool);

    const CI: &str = "\
name: CI
on: [push]
# A comment: jobs: not here
env:
  RUST: stable
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: |
          echo 'needs: [nothing]'
          name: not a key
      - name: Build   # trailing comment
        run: cargo build
  test:
    name: Test
    needs: build
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    continue-on-error: true
  'lint':
    needs: [build, test]
  docs:
    name: \"Docs on ${{ matrix.os }}\"
    needs:
    - build
    - 'lint'
  release:
    needs: [
      test,
      docs ]
    uses: ./.github/workflows/release.yml
";

    #[test]
    fn a_workflow_s_jobs_are_read_with_their_names_and_needs() {
        let workflow = Workflow::parse(CI);
        let read: Vec<Read<'_>> = workflow
            .jobs
            .iter()
            .map(|job| {
                (
                    job.id.as_str(),
                    job.name.as_deref(),
                    job.needs.iter().map(String::as_str).collect(),
                    job.continue_on_error,
                    job.uses,
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                ("build", None, vec![], false, false),
                ("test", Some("Test"), vec!["build"], true, false),
                ("lint", None, vec!["build", "test"], false, false),
                (
                    "docs",
                    Some("Docs on ${{ matrix.os }}"),
                    vec!["build", "lint"],
                    false,
                    false
                ),
                ("release", None, vec!["test", "docs"], false, true),
            ]
        );
    }

    #[test]
    fn a_run_s_jobs_are_found_by_their_names() {
        let workflow = Workflow::parse(CI);
        let id = |name: &str| {
            workflow
                .job_of(name)
                .map(|at| workflow.jobs[at].id.as_str())
        };
        assert_eq!(id("build"), Some("build"));
        assert_eq!(id("Test (ubuntu-latest)"), Some("test"));
        assert_eq!(id("Test"), Some("test"));
        assert_eq!(id("Docs on macos-latest"), Some("docs"));
        assert_eq!(id("release / publish"), Some("release"));
        assert_eq!(id("lint / not called"), None);
        assert_eq!(id("unknown"), None);
    }

    #[test]
    fn what_isn_t_a_workflow_reads_as_none() {
        assert_eq!(Workflow::parse(""), Workflow::default());
        assert_eq!(
            Workflow::parse("on: push\nsteps:\n  - run: x\n"),
            Workflow::default()
        );
        assert_eq!(Workflow::parse("jobs:\n"), Workflow::default());
    }
}
