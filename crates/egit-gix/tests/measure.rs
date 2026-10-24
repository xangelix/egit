//! Measures gitoxide against the git binary on a large repository, read for read: the
//! topological walk (from the same tips, compared commit by commit), reading commits in batches,
//! and listing references. Not run by default:
//!
//! ```text
//! EGIT_SCALE_REPO=<a large repository> cargo test -p egit-gix --release --test measure \
//!     -- --ignored --nocapture
//! ```

// A measuring tool: it prints what it measures, git's beside gix's (named alike).
#![allow(
    clippy::similar_names,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bstr::ByteSlice as _;
use egit_git::{
    CancelToken, Commit, Git, GitOptions, ObjectId, ObjectReader, Repository, Wait as _,
    list_references, walk_history,
};
use gix::prelude::Find as _;

fn repository_path() -> Option<PathBuf> {
    std::env::var_os("EGIT_SCALE_REPO").map(PathBuf::from)
}

/// The tips egit's walk starts from: every ref but stashes, less those another reaches.
fn tips(git: &Git, repository: &Repository) -> Vec<gix::ObjectId> {
    let listed = git
        .read("rev-list")
        .args(["--no-walk=unsorted", "--exclude=refs/stash", "--all"])
        .current_dir(repository.work_tree())
        .output()
        .wait()
        .unwrap()
        .stdout;
    let all: Vec<String> = listed
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_str().unwrap().to_owned())
        .collect();
    // As egit's walk does: past so many, walking them all costs no more than reducing them
    // (and their names would pass what a command line holds).
    if all.len() > 4_096 {
        return all
            .iter()
            .map(|line| gix::ObjectId::from_hex(line.as_bytes()).unwrap())
            .collect();
    }
    let reduced = git
        .read("merge-base")
        .arg("--independent")
        .args(&all)
        .current_dir(repository.work_tree())
        .output()
        .wait()
        .unwrap()
        .stdout;
    reduced
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| gix::ObjectId::from_hex(line).unwrap())
        .collect()
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

#[test]
#[ignore = "a release-build measurement against EGIT_SCALE_REPO"]
fn measure() {
    let Some(path) = repository_path() else {
        println!("EGIT_SCALE_REPO isn't set: nothing to measure");
        return;
    };
    let git = Git::new(GitOptions::default()).wait().unwrap();
    let repository = Repository::discover(&git, &path).wait().unwrap();
    println!("{}:", path.display());
    let walked = git_walk(&git, &repository);
    gix_walk(&git, &repository, &walked);
    gix_walk_from_every_tip(&repository);
    commit_reads(&git, &repository, &walked);
    tips_listed(&git, &repository);
    references(&git, &repository);
}

/// The walk, as egit has git do it.
fn git_walk(git: &Git, repository: &Repository) -> Vec<(ObjectId, Vec<ObjectId>)> {
    let started = Instant::now();
    let mut first = None;
    let mut walked: Vec<(ObjectId, Vec<ObjectId>)> = Vec::new();
    walk_history(git, repository, &CancelToken::new(), |id, parents| {
        first.get_or_insert_with(|| started.elapsed());
        walked.push((id, parents.to_vec()));
        ControlFlow::Continue(())
    })
    .wait()
    .unwrap();
    println!(
        "  git walk: first commit {}, all {} commits in {}",
        ms(first.unwrap()),
        walked.len(),
        ms(started.elapsed())
    );
    walked
}

/// The same walk through gix, from the same tips, compared commit by commit.
fn gix_walk(git: &Git, repository: &Repository, walked: &[(ObjectId, Vec<ObjectId>)]) {
    let started = Instant::now();
    let tips = tips(git, repository);
    let tips_time = started.elapsed();
    let started = Instant::now();
    let repo = gix::open(repository.work_tree()).unwrap();
    let graph = repo.commit_graph_if_enabled().unwrap();
    let opened = started.elapsed();
    let walk = gix::traverse::commit::topo::Builder::from_iters(
        &repo.objects,
        tips.iter().copied(),
        None::<Vec<gix::ObjectId>>,
    )
    .sorting(gix::traverse::commit::topo::Sorting::TopoOrder)
    .with_commit_graph(graph)
    .build()
    .unwrap();
    let mut first = None;
    let mut same = true;
    let mut count = 0_usize;
    for (index, info) in walk.enumerate() {
        let info = info.unwrap();
        first.get_or_insert_with(|| started.elapsed());
        if let Some((id, parents)) = walked.get(index) {
            let matches = id.as_bytes() == info.id.as_bytes()
                && parents.len() == info.parent_ids.len()
                && parents
                    .iter()
                    .zip(info.parent_ids.iter())
                    .all(|(ours, theirs)| ours.as_bytes() == theirs.as_bytes());
            if !matches && same {
                println!(
                    "  gix walk differs first at commit {index}: {} vs {}",
                    id, info.id
                );
                same = false;
            }
        }
        count += 1;
    }
    println!(
        "  gix walk: tips (git) {}, open {}, first commit {}, all {count} in {}; {}",
        ms(tips_time),
        ms(opened),
        ms(first.unwrap()),
        ms(started.elapsed()),
        if same && count == walked.len() {
            "the same order as git's"
        } else {
            "NOT git's order"
        }
    );
}

/// gix from every tip, none reduced: does its walk need them reduced?
fn gix_walk_from_every_tip(repository: &Repository) {
    let started = Instant::now();
    let repo = gix::open(repository.work_tree()).unwrap();
    let mut every: Vec<gix::ObjectId> = Vec::new();
    for reference in repo.references().unwrap().all().unwrap() {
        let mut reference = reference.unwrap();
        if reference.name().as_bstr().starts_with(b"refs/stash") {
            continue;
        }
        if let Ok(id) = reference.peel_to_commit().map(|commit| commit.id) {
            every.push(id);
        }
    }
    if let Ok(head) = repo.head_id() {
        every.push(head.detach());
    }
    every.sort_unstable();
    every.dedup();
    let listed = started.elapsed();
    let graph = repo.commit_graph_if_enabled().unwrap();
    let walk = gix::traverse::commit::topo::Builder::from_iters(
        &repo.objects,
        every.iter().copied(),
        None::<Vec<gix::ObjectId>>,
    )
    .sorting(gix::traverse::commit::topo::Sorting::TopoOrder)
    .with_commit_graph(graph)
    .build()
    .unwrap();
    let mut first = None;
    let mut count = 0_usize;
    for info in walk {
        info.unwrap();
        first.get_or_insert_with(|| started.elapsed());
        count += 1;
    }
    println!(
        "  gix walk from all {} tips: listed {}, first commit {}, all {count} in {}",
        every.len(),
        ms(listed),
        ms(first.unwrap()),
        ms(started.elapsed())
    );
}

/// Commits read 512 at a time, spread over the history: first from a fresh reader (a new
/// `cat-file`, a newly opened repository), then again with both warm.
fn commit_reads(git: &Git, repository: &Repository, walked: &[(ObjectId, Vec<ObjectId>)]) {
    let step = walked.len().div_ceil(4096).max(1);
    let ids: Vec<ObjectId> = walked.iter().step_by(step).map(|(id, _)| *id).collect();
    let batches = u32::try_from(ids.len().div_ceil(512)).unwrap();
    let git_pass = |reader: &mut ObjectReader| {
        let started = Instant::now();
        let mut subjects = Vec::with_capacity(ids.len());
        for batch in ids.chunks(512) {
            reader
                .read(batch, |_, object| {
                    let commit = Commit::parse(object.unwrap().data).unwrap();
                    subjects.push(commit.subject());
                })
                .wait()
                .unwrap();
        }
        (started.elapsed(), subjects)
    };
    let gix_pass = |repo: &gix::Repository| {
        let started = Instant::now();
        let mut buffer = Vec::new();
        let mut subjects = Vec::with_capacity(ids.len());
        for id in &ids {
            let oid = gix::ObjectId::from_bytes_or_panic(id.as_bytes());
            let data = repo.objects.try_find(&oid, &mut buffer).unwrap().unwrap();
            subjects.push(Commit::parse(data.data).unwrap().subject());
        }
        (started.elapsed(), subjects)
    };
    // As egit reads them: a batch a call.
    let egit_pass = |gix: &egit_gix::GixRepository| {
        let started = Instant::now();
        let mut subjects = Vec::with_capacity(ids.len());
        for batch in ids.chunks(512) {
            gix.read(batch, |_, object| {
                subjects.push(Commit::parse(object.unwrap().data).unwrap().subject());
            })
            .unwrap();
        }
        (started.elapsed(), subjects)
    };
    let mut reader = ObjectReader::start(git, repository.work_tree())
        .wait()
        .unwrap();
    let fresh = gix::open(repository.work_tree()).unwrap();
    let ours = egit_gix::GixRepository::open(repository.work_tree()).unwrap();
    let (git_cold, git_subjects) = git_pass(&mut reader);
    let (gix_cold, gix_subjects) = gix_pass(&fresh);
    let (egit_cold, egit_subjects) = egit_pass(&ours);
    let (git_warm, _) = git_pass(&mut reader);
    let (gix_warm, _) = gix_pass(&fresh);
    let (egit_warm, _) = egit_pass(&ours);
    println!(
        "  {} commits read, per 512: fresh git {} gix {} egit-gix {}; warm git {} gix {} \
         egit-gix {}; {}",
        ids.len(),
        ms(git_cold / batches),
        ms(gix_cold / batches),
        ms(egit_cold / batches),
        ms(git_warm / batches),
        ms(gix_warm / batches),
        ms(egit_warm / batches),
        if git_subjects == gix_subjects && gix_subjects == egit_subjects {
            "the same commits"
        } else {
            "DIFFERENT commits"
        }
    );
}

/// The walk's tips listed: git's `rev-list --no-walk --all` against gix's references, peeled
/// to commits; best of five each.
fn tips_listed(git: &Git, repository: &Repository) {
    let mut git_best = Duration::MAX;
    let mut gix_best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let listed = git
            .read("rev-list")
            .args(["--no-walk=unsorted", "--exclude=refs/stash", "--all"])
            .current_dir(repository.work_tree())
            .output()
            .wait()
            .unwrap();
        assert_ne!(listed.stdout.len(), 0);
        git_best = git_best.min(started.elapsed());
        let started = Instant::now();
        let tips = egit_gix::GixRepository::open(repository.work_tree())
            .unwrap()
            .tips()
            .unwrap();
        assert_ne!(tips.len(), 0);
        gix_best = gix_best.min(started.elapsed());
    }
    println!(
        "  tips listed (best of 5): git {}, gix (opening the repository too) {}",
        ms(git_best),
        ms(gix_best)
    );
}

/// References, peeled, as egit lists them.
fn references(git: &Git, repository: &Repository) {
    let started = Instant::now();
    let refs = list_references(git, repository).wait().unwrap();
    let git_refs = started.elapsed();
    let gix = egit_gix::GixRepository::open(repository.work_tree()).unwrap();
    let started = Instant::now();
    let gix_refs = gix.references().unwrap();
    println!(
        "  references: git {} ({}), gix {} ({})",
        ms(git_refs),
        refs.len(),
        ms(started.elapsed()),
        gix_refs.len()
    );
}
