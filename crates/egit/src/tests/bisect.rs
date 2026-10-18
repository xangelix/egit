//! Bisecting and rebasing merges, driven as a user drives them: a bisection started from two
//! commits' menus, its commits marked from the banner until the first bad one is shown, then
//! stopped; and a branch holding a merge rebased onto another keeping it, then rewritten in the
//! rebase editor (its order git's: a commit dropped, nothing moved or melded, the merge kept).

use std::path::{Path, PathBuf};

use egit_git::ObjectId;
use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::patches::wait_for_text;
use super::screens::{
    harness, history_of, history_shown, menu_item, open_path, profile_loaded, render, settle,
    wait_for_label,
};
use crate::app::EgitApp;
use crate::theme::icon;

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

fn opened(sandbox: &Sandbox, repo: &Path) -> Harness<'static, EgitApp> {
    let mut harness = harness(sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, repo);
    settle(&mut harness, "the history", history_shown);
    harness
}

/// Picks `item` in the Bisect submenu of the commit whose subject is `subject`.
fn bisect_item(harness: &mut Harness<'_, EgitApp>, subject: &str, item: &str) {
    harness
        .get_by_role_and_label(Role::Button, subject)
        .click_secondary();
    // egui names a submenu's button with its arrow.
    wait_for_label(harness, "Bisect ⏵");
    harness.get_by_label("Bisect ⏵").click();
    wait_for_label(harness, item);
    harness.get_by_label(item).click();
}

/// Whether no task runs and the bisection read is `done`.
fn bisected(done: impl Fn(&egit_git::BisectLeft) -> bool) -> impl Fn(&mut EgitApp) -> bool {
    move |app| {
        let repo = &mut app.state.repos[0];
        !repo.tasks.is_busy()
            && repo
                .status
                .ok_ref()
                .and_then(|status| status.bisect.as_ref())
                .is_some_and(|bisection| done(&bisection.left))
    }
}

#[test]
fn a_bisection_finds_the_first_bad_commit_from_the_banner() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for number in 1..=10 {
        sandbox
            .commit_file(&repo, "f", &format!("{number}\n"), &format!("c{number}"))
            .unwrap();
    }
    let seventh = rev(&sandbox, &repo, "main~3");
    let mut harness = opened(&sandbox, &repo);
    settle(&mut harness, "every commit", history_of(10));

    // Started from the newest's menu (bad), then the oldest's (good).
    bisect_item(&mut harness, "c10", "Mark as bad");
    settle(
        &mut harness,
        "the bisection started",
        bisected(|left| *left == egit_git::BisectLeft::NeedsGood),
    );
    wait_for_text(&mut harness, "Mark a commit good");
    wait_for_label(&mut harness, &format!("{} bad", icon::X_CIRCLE));
    bisect_item(&mut harness, "c1", "Mark as good");
    settle(
        &mut harness,
        "a commit to test",
        bisected(|left| matches!(left, egit_git::BisectLeft::Testing { .. })),
    );
    render(&mut harness, "bisect");

    // Each commit checked out marked from the banner, until the first bad one.
    mark_until_found(&mut harness, &sandbox, &repo);
    wait_for_text(
        &mut harness,
        &format!("{} is the first bad commit.", seventh.abbreviated(8)),
    );
    harness.get_by_label("Show the commit").click();
    settle(&mut harness, "the first bad commit selected", move |app| {
        app.state.repos[0]
            .history_view
            .selected_row()
            .is_some_and(|row| app.state.repos[0].history.shown.id(row) == Some(seventh))
    });
    // Its details read and their lines counted: they land in the background.
    settle(&mut harness, "its details", |app| {
        app.state.repos[0].history_view.details_shown()
            && app.state.repos[0].history_view.counts_shown()
    });
    render(&mut harness, "bisect-found");

    // Stopped: back on the branch.
    harness.get_by_label("Stop bisecting").click();
    settle(&mut harness, "the bisection stopped", |app| {
        let repo = &mut app.state.repos[0];
        !repo.tasks.is_busy()
            && repo
                .status
                .ok_ref()
                .is_some_and(|status| status.operations.is_empty())
    });
    assert_eq!(rev(&sandbox, &repo, "HEAD"), rev(&sandbox, &repo, "main"));
}

/// `One`; `side` (`Side one`, `Side two`) and `main` (`Two`) from it, `side` merged into `main`
/// (`Merge side`), then `Three`; and `upstream`, moved on from `One` (`Upstream`).
fn merged(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox.run(&repo, "branch", &["upstream"]).unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "side"])
        .unwrap();
    sandbox
        .commit_file(&repo, "s1", "s1\n", "Side one")
        .unwrap();
    sandbox
        .commit_file(&repo, "s2", "s2\n", "Side two")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    sandbox
        .run(
            &repo,
            "merge",
            &["--quiet", "--no-ff", "--message", "Merge side", "side"],
        )
        .unwrap();
    sandbox.commit_file(&repo, "f", "three\n", "Three").unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "upstream"])
        .unwrap();
    sandbox.commit_file(&repo, "u", "u\n", "Upstream").unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox
        .run(&repo, "branch", &["--delete", "--force", "side"])
        .unwrap();
    repo
}

/// How many merges `main` has that `upstream` hasn't.
fn merges(sandbox: &Sandbox, repo: &Path) -> String {
    let out = sandbox
        .run(repo, "rev-list", &["--merges", "--count", "upstream..main"])
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn a_branch_is_rebased_keeping_its_merges() {
    let sandbox = Sandbox::new().unwrap();
    let repo = merged(&sandbox);
    let mut harness = opened(&sandbox, &repo);
    settle(&mut harness, "every commit", history_of(7));

    // Onto `upstream`, the merge kept.
    menu_item(
        &mut harness,
        &format!("{} upstream", icon::GIT_BRANCH),
        "Rebase main onto upstream, keeping merges",
    );
    wait_for_label(&mut harness, "Rebased onto upstream.");
    assert_eq!(merges(&sandbox, &repo), "1");
    assert!(repo.join("u").exists());
    let rebased = rev(&sandbox, &repo, "main");
    settle(&mut harness, "the history read again", move |app| {
        let repo = &mut app.state.repos[0];
        !repo.history.is_loading() && repo.history.shown.row_of(&rebased, 0).is_some()
    });

    // Rewritten in the editor: rows don't move, nothing melds, the merge can't be dropped.
    harness
        .get_by_role_and_label(Role::Button, "Two")
        .click_secondary();
    wait_for_label(&mut harness, "Rebase interactively from here…");
    harness
        .get_by_label("Rebase interactively from here…")
        .click();
    wait_for_text(&mut harness, "1 merge is kept, in git's order");
    let select = |harness: &mut Harness<'_, EgitApp>, subject: &str| {
        harness.get_by_role_and_label(Role::Button, subject).click();
        harness.run_steps(2);
    };
    let press = |harness: &mut Harness<'_, EgitApp>, modifiers, key| {
        harness.key_press_modifiers(modifiers, key);
        harness.run_steps(2);
    };
    select(&mut harness, "Merge side");
    press(&mut harness, Modifiers::NONE, Key::D);
    press(&mut harness, Modifiers::ALT, Key::ArrowDown);
    select(&mut harness, "Side two");
    press(&mut harness, Modifiers::NONE, Key::S);
    let starting = harness.get_by_label("Start rebase");
    assert!(
        starting.accesskit_node().is_disabled(),
        "nothing changes: the merge isn't dropped, nothing moves or melds"
    );
    select(&mut harness, "Side one");
    press(&mut harness, Modifiers::NONE, Key::D);
    render(&mut harness, "rebase-merges");
    harness.get_by_label("Start rebase").click();
    wait_for_label(&mut harness, "Rebased: the branch is rewritten.");
    assert_eq!(merges(&sandbox, &repo), "1", "still a merge");
    assert!(!repo.join("s1").exists(), "its commit dropped");
    assert!(repo.join("s2").exists());
}

/// Whether the bisection found its first bad commit.
fn found(app: &mut EgitApp) -> bool {
    app.state.repos[0].status.ok_ref().is_some_and(|status| {
        status
            .bisect
            .as_ref()
            .is_some_and(|bisection| matches!(bisection.left, egit_git::BisectLeft::Found(_)))
    })
}

/// Marks each commit checked out from the banner (bad from c7 on, good before) until the first
/// bad commit is found.
fn mark_until_found(harness: &mut Harness<'_, EgitApp>, sandbox: &Sandbox, repo: &Path) {
    for _ in 0..10 {
        // From the bisection itself: its words may be a frame behind.
        if found(harness.state_mut()) {
            break;
        }
        let number: u32 = std::fs::read_to_string(repo.join("f"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mark = if number >= 7 { "Bad" } else { "Good" };
        // Its button, clicked in the frame it is found (the banner is drawn again as the
        // status is read); unless the status read since says the bisection is over.
        let mut clicked = false;
        for _ in 0..2_000 {
            harness.step();
            if found(harness.state_mut()) {
                break;
            }
            if harness
                .query_by_label(mark)
                .is_some_and(|button| !button.accesskit_node().is_disabled())
            {
                harness.get_by_label(mark).click();
                clicked = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !clicked {
            assert!(found(harness.state_mut()), "{mark} never came");
            break;
        }
        settle(
            harness,
            "the mark taken in",
            bisected(|left| !matches!(left, egit_git::BisectLeft::NeedsGood)),
        );
        let checked = rev(sandbox, repo, "HEAD");
        settle(harness, "the status read again", move |app| {
            let repo = &mut app.state.repos[0];
            !repo.tasks.is_busy()
                && repo.status.ok_ref().is_some_and(|status| {
                    status.bisect.as_ref().is_some_and(|bisection| {
                        matches!(bisection.left, egit_git::BisectLeft::Found(_))
                    }) || matches!(&status.head, egit_git::Head::Detached(commit) if commit.id == checked)
                })
        });
    }
}
