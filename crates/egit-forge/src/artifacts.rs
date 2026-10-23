//! CI/CD artifacts, to download: what a GitLab job kept (its `artifacts:paths` as one zip
//! archive, `artifacts.zip`, and its reports apart, each a file of its own: `junit.xml.gz`,
//! `gl-sast-report.json`; its log and the archive's metadata left out) and what a GitHub Actions
//! run uploaded (each named artifact, a zip).
//!
//! A download streams to the caller's sink, never held whole (an archive may be gigabytes): the
//! forge answers with a redirect to its storage (GitLab's CDN, GitHub's blobs), fetched without
//! the token, or (a self-managed GitLab serving it itself) with the file; GitHub's SHA-256
//! digest of the zip is checked as it comes.

use std::fmt::Write as _;

use egit_git::CancelToken;
use sha2::{Digest as _, Sha256};

use crate::account::Account;
use crate::answer::reason;
use crate::error::ForgeError;
use crate::host::ForgeKind;
use crate::repository::ForgeRepository;

/// An artifact of a pipeline's job (GitLab's) or of a workflow's run (GitHub's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// What the forge calls it: GitLab's file (`artifacts.zip`, `junit.xml.gz`), a GitHub
    /// artifact's name (`dist-linux`).
    pub name: String,
    /// The file it is saved as unless the user says otherwise: GitLab's own, a GitHub
    /// artifact's name and `.zip`.
    pub file_name: String,
    /// What it is.
    pub kind: ArtifactKind,
    /// Its size in bytes, as the forge says (a GitHub artifact's zip, GitLab's file).
    pub size: Option<u64>,
    /// Whether it expired (the forge no longer keeps it).
    pub expired: bool,
    /// GitHub's digest of the zip (`sha256:<hex>`), checked as it is downloaded.
    pub digest: Option<String>,
    /// Where it is downloaded from.
    pub source: ArtifactSource,
}

/// What an artifact is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    /// A GitLab job's `artifacts:paths`, as one zip archive.
    Archive,
    /// A GitLab job's report (`artifacts:reports`: `JUnit`, coverage, a scan), a file of its own.
    Report,
    /// A GitHub artifact a workflow uploaded (a zip of what it named).
    Uploaded,
}

/// Where an artifact is downloaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactSource {
    /// A GitLab job's, by its file type (`archive`, `junit`, …).
    GitLab {
        /// The job's number.
        job: u64,
        /// GitLab's file type, in its REST API's words.
        file_type: String,
    },
    /// A GitHub artifact, by its number.
    GitHub {
        /// The artifact's number.
        id: u64,
    },
}

/// A GitLab artifact of job `job`, as its GraphQL API lists it (`fileType`, `name`, `size`,
/// `expireAt`); none for its log or the archive's metadata, which aren't downloads.
pub(crate) fn gitlab_artifact(
    job: u64,
    (file_type, name, size, expires): (&str, &str, Option<u64>, Option<i64>),
) -> Option<Artifact> {
    let file_type = file_type.to_ascii_lowercase();
    if matches!(file_type.as_str(), "trace" | "metadata") {
        return None;
    }
    let kind = if file_type == "archive" {
        ArtifactKind::Archive
    } else {
        ArtifactKind::Report
    };
    let file_name = if name.is_empty() {
        format!("{file_type}.zip")
    } else {
        name.to_owned()
    };
    Some(Artifact {
        name: file_name.clone(),
        file_name,
        kind,
        size,
        expired: expires.is_some_and(|expires| expires <= crate::answer::now()),
        digest: None,
        source: ArtifactSource::GitLab { job, file_type },
    })
}

/// Downloads `artifact` of `repository`, handing its bytes to `sink` as they come (it says
/// `false` to stop: its file couldn't be written); returns how many there were.
///
/// # Errors
///
/// Any of [`ForgeError`]'s: [`ForgeError::Cancelled`] when `cancel` (or the sink) stopped it,
/// [`ForgeError::Said`] for an artifact that expired, [`ForgeError::Mismatched`] for one that
/// doesn't match its digest.
pub fn download_artifact(
    account: &Account,
    repository: &ForgeRepository,
    artifact: &Artifact,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<u64, ForgeError> {
    let host = account.host().name().to_owned();
    let what = format!("{}'s artifact {}", repository.full_name(), artifact.name);
    let url = match (&artifact.source, repository.host().kind()) {
        (ArtifactSource::GitLab { job, file_type }, ForgeKind::GitLab) => {
            account.host().rest_url(&format!(
                "/projects/{}/jobs/{job}/artifacts?file_type={file_type}",
                crate::gitlab::encoded(repository.path())
            ))
        }
        (ArtifactSource::GitHub { id }, ForgeKind::GitHub) => account.host().rest_url(&format!(
            "/repos/{}/actions/artifacts/{id}/zip",
            repository.path()
        )),
        _ => return Err(crate::answer::not_found(account, what)),
    };
    let expected = artifact
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase);
    let mut hasher = expected.as_ref().map(|_| Sha256::new());
    let mut given = 0_u64;
    let mut take = |part: &[u8]| -> bool {
        if cancel.is_cancelled() {
            return false;
        }
        if let Some(hasher) = &mut hasher {
            hasher.update(part);
        }
        given = given.saturating_add(u64::try_from(part.len()).unwrap_or(u64::MAX));
        sink(part)
    };
    let first = account.download(&url, &mut take, cancel)?;
    let done = match (first.status, first.location.as_deref()) {
        (301..=308, Some(location)) => account.download_public(location, &mut take, cancel)?,
        _ => first,
    };
    if done.stopped || cancel.is_cancelled() {
        return Err(ForgeError::Cancelled);
    }
    match done.status {
        200..=299 => {}
        404 => return Err(crate::answer::not_found(account, what)),
        410 => {
            return Err(ForgeError::Said {
                host,
                message: format!(
                    "{} has expired: the forge no longer keeps it",
                    artifact.name
                ),
            });
        }
        403 => {
            return Err(ForgeError::Forbidden {
                host,
                message: reason(&done.said),
            });
        }
        status => {
            return Err(ForgeError::Status {
                host,
                status,
                message: reason(&done.said),
            });
        }
    }
    if let (Some(expected), Some(hasher)) = (expected, hasher) {
        let found = hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            });
        if found != expected {
            return Err(ForgeError::Mismatched {
                host,
                what: artifact.file_name.clone(),
            });
        }
    }
    Ok(given)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gitlab_job_s_archive_and_reports_are_artifacts_its_log_isn_t() {
        let archive = gitlab_artifact(7, ("ARCHIVE", "artifacts.zip", Some(309), None)).unwrap();
        assert_eq!(archive.kind, ArtifactKind::Archive);
        assert_eq!(archive.file_name, "artifacts.zip");
        assert_eq!(
            archive.source,
            ArtifactSource::GitLab {
                job: 7,
                file_type: "archive".to_owned()
            }
        );
        let junit = gitlab_artifact(7, ("JUNIT", "junit.xml.gz", None, Some(1))).unwrap();
        assert_eq!(junit.kind, ArtifactKind::Report);
        assert!(junit.expired);
        assert!(gitlab_artifact(7, ("TRACE", "job.log", None, None)).is_none());
        assert!(gitlab_artifact(7, ("METADATA", "metadata.gz", None, None)).is_none());
    }
}
