//! Artifacts downloaded to the user's disk, where the save dialog said (a GitLab job's archive or
//! report, a GitHub run's artifact). Each runs on a thread of its own, the forge's answer
//! streamed into a file beside the one asked for (`<file>.part`, never held whole: an archive
//! may be gigabytes) and renamed into place once whole (and, for GitHub's, its digest checked),
//! so a part never passes for the file; cancelled or failed, the part is removed (as are those
//! still under way when their tab closes). A tab's downloads are said under its commit table:
//! how far each has come, then where it went.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egit_forge::{Account, Artifact, ForgeError, ForgeRepository, download_artifact};
use egit_git::CancelToken;
use parking_lot::Mutex;

use crate::problem::Problem;

/// How often a download's progress wakes the window, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// What is written out at once.
const WRITTEN_AT_ONCE: usize = 1024 * 1024;

/// An artifact asked for: which of the forge's repositories it is in, the artifact, and what
/// it is of (a GitLab job's name, a GitHub run's title), to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactWanted {
    pub(crate) repository: usize,
    pub(crate) artifact: Artifact,
    pub(crate) of: String,
}

/// Where a download stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Under way.
    Running,
    /// Saved where asked.
    Saved,
    /// Not saved: why.
    Failed(Problem),
}

/// One download.
#[derive(Debug)]
pub(crate) struct Download {
    pub(crate) id: u64,
    pub(crate) wanted: ArtifactWanted,
    /// Where it goes.
    pub(crate) to: PathBuf,
    /// How many bytes have come.
    done: AtomicU64,
    stage: Mutex<Stage>,
    cancel: CancelToken,
}

impl Download {
    /// How many bytes have come.
    pub(crate) fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Where it stands.
    pub(crate) fn stage(&self) -> Stage {
        self.stage.lock().clone()
    }
}

/// A tab's downloads, the oldest first.
#[derive(Debug, Default)]
pub(crate) struct Downloads {
    list: Vec<Arc<Download>>,
    next: u64,
}

impl Downloads {
    /// Its downloads, the oldest first.
    pub(crate) fn list(&self) -> &[Arc<Download>] {
        &self.list
    }

    /// Starts downloading `wanted` from the repository `reach` names, through its account, into
    /// `to`, waking `waker` as it goes (none: its project is gone from the remotes, said).
    pub(crate) fn start(
        &mut self,
        reach: Option<(Arc<Account>, ForgeRepository)>,
        wanted: ArtifactWanted,
        to: PathBuf,
        waker: &egui::Context,
    ) {
        self.next = self.next.wrapping_add(1);
        let download = Arc::new(Download {
            id: self.next,
            wanted,
            to,
            done: AtomicU64::new(0),
            stage: Mutex::new(Stage::Running),
            cancel: CancelToken::new(),
        });
        self.list.push(Arc::clone(&download));
        let Some((account, repository)) = reach else {
            *download.stage.lock() = Stage::Failed(Problem::new(
                "The artifact couldn't be downloaded",
                "Its project is no longer among the remotes.",
            ));
            return;
        };
        let (going, waker) = (Arc::clone(&download), waker.clone());
        let started = std::thread::Builder::new()
            .name("artifact download".to_owned())
            .spawn(move || {
                let saved = egit_git::block_on(save(&going, (&account, &repository), &waker));
                let stage = match saved {
                    Ok(()) => Stage::Saved,
                    Err(problem) => Stage::Failed(problem),
                };
                *going.stage.lock() = stage;
                waker.request_repaint();
            });
        if let Err(error) = started {
            *download.stage.lock() = Stage::Failed(Problem::new(
                "The artifact couldn't be downloaded",
                error.to_string(),
            ));
        }
    }

    /// Stops download `id` (its part removed) and forgets it.
    pub(crate) fn cancel(&mut self, id: u64) {
        if let Some(download) = self.list.iter().find(|download| download.id == id) {
            download.cancel.cancel();
        }
        self.list.retain(|download| download.id != id);
    }

    /// Forgets download `id` (one that ended).
    pub(crate) fn dismiss(&mut self, id: u64) {
        self.list.retain(|download| download.id != id);
    }
}

impl Drop for Downloads {
    /// Stops those under way (their parts removed): with their tab closed, nothing would say
    /// how they went.
    fn drop(&mut self) {
        for download in &self.list {
            download.cancel.cancel();
        }
    }
}

/// Where a download is written until it is whole: beside the file, `<file>.part`.
fn part_of(to: &Path) -> PathBuf {
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

/// Downloads `download` into its part, then renames it into place; the part removed otherwise.
async fn save(
    download: &Download,
    (account, repository): (&Account, &ForgeRepository),
    waker: &egui::Context,
) -> Result<(), Problem> {
    let part = part_of(&download.to);
    let written = write(download, (account, repository), (&part, waker))
        .await
        .and_then(|()| {
            std::fs::rename(&part, &download.to).map_err(|error| {
                Problem::new(
                    "The artifact couldn't be saved",
                    format!("{}: {error}", download.to.display()),
                )
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// Streams `download` into the file `part`.
async fn write(
    download: &Download,
    (account, repository): (&Account, &ForgeRepository),
    (part, waker): (&Path, &egui::Context),
) -> Result<(), Problem> {
    let not_saved = |error: &std::io::Error| {
        Problem::new(
            "The artifact couldn't be saved",
            format!("{}: {error}", part.display()),
        )
    };
    let file = std::fs::File::create(part).map_err(|error| not_saved(&error))?;
    let mut writer = std::io::BufWriter::with_capacity(WRITTEN_AT_ONCE, file);
    let mut failed: Option<std::io::Error> = None;
    let mut woken = Instant::now();
    let read = download_artifact(
        account,
        repository,
        &download.wanted.artifact,
        &download.cancel,
        &mut |bytes| {
            if let Err(error) = writer.write_all(bytes) {
                failed = Some(error);
                return false;
            }
            let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            download.done.fetch_add(length, Ordering::Relaxed);
            if woken.elapsed() >= PROGRESS_EVERY {
                woken = Instant::now();
                waker.request_repaint();
            }
            true
        },
    )
    .await;
    if let Some(error) = failed {
        return Err(not_saved(&error));
    }
    match read {
        Ok(_) => {}
        Err(ForgeError::Cancelled) => {
            return Err(Problem::new("The download was cancelled", String::new()));
        }
        Err(error) => {
            return Err(Problem::new(
                "The artifact couldn't be downloaded",
                error.to_string(),
            ));
        }
    }
    let file = writer
        .into_inner()
        .map_err(|error| not_saved(error.error()))?;
    file.sync_all().map_err(|error| not_saved(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_under_way_stop_with_their_tab() {
        let download = Arc::new(Download {
            id: 1,
            wanted: ArtifactWanted {
                repository: 0,
                artifact: Artifact {
                    name: "artifacts.zip".to_owned(),
                    file_name: "artifacts.zip".to_owned(),
                    kind: egit_forge::ArtifactKind::Archive,
                    size: None,
                    expired: false,
                    digest: None,
                    source: egit_forge::ArtifactSource::GitLab {
                        job: 7,
                        file_type: "archive".to_owned(),
                    },
                },
                of: "build".to_owned(),
            },
            to: PathBuf::from("/tmp/out/artifacts.zip"),
            done: AtomicU64::new(0),
            stage: Mutex::new(Stage::Running),
            cancel: CancelToken::new(),
        });
        let downloads = Downloads {
            list: vec![Arc::clone(&download)],
            next: 1,
        };
        drop(downloads);
        assert!(download.cancel.is_cancelled());
    }

    #[test]
    fn a_download_is_written_beside_its_file_until_whole() {
        assert_eq!(
            part_of(Path::new("/tmp/out/artifacts.zip")),
            PathBuf::from("/tmp/out/artifacts.zip.part")
        );
    }
}
