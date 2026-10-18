//! Help: what each thing egit offers does at git's level, and what happens in the cases that
//! surprise people (cancelling a rebase, editing a pushed commit's message, a submodule that
//! won't sit still), as topics.
//!
//! Two ways in. Holding Alt while pointing at a control shows its topic beside it: the shared
//! controls (`widgets`, the menus' items) note where they are and what they say as they are
//! drawn, while Alt is held only (nothing is kept otherwise), and custom ones (a commit's row,
//! a badge, the graph) note their topic outright; at the frame's end the topmost one under the
//! pointer is found and its topic drawn. And F1 (or the status bar's Help) opens every topic,
//! searchable.

use std::sync::Arc;

use egui::{Align2, Color32, Context, Frame, Id, LayerId, Margin, Order, Pos2, Rect, RichText, Ui};
use parking_lot::Mutex;

use crate::theme::Weighted as _;
use crate::theme::{Palette, icon};

/// Topics, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Everyday,
    Branches,
    Rewriting,
    Remotes,
    Conflicts,
    Inside,
    Looking,
    Egit,
}

impl Group {
    /// Every group, in the order shown.
    pub(crate) const ALL: [Self; 8] = [
        Self::Everyday,
        Self::Branches,
        Self::Remotes,
        Self::Rewriting,
        Self::Conflicts,
        Self::Inside,
        Self::Looking,
        Self::Egit,
    ];

    /// Its name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Everyday => "Every day",
            Self::Branches => "Branches and tags",
            Self::Rewriting => "Rewriting history",
            Self::Remotes => "Remotes",
            Self::Conflicts => "Conflicts",
            Self::Inside => "Submodules, LFS, worktrees",
            Self::Looking => "Looking into history",
            Self::Egit => "egit itself",
        }
    }
}

/// A topic: what something does, the git that does it, and the cases that surprise.
#[derive(Debug)]
pub(crate) struct Topic {
    pub(crate) id: &'static str,
    pub(crate) group: Group,
    pub(crate) title: &'static str,
    /// What it does, at git's level.
    pub(crate) what: &'static str,
    /// The git commands egit runs for it.
    pub(crate) git: &'static str,
    /// Questions people meet, and their answers.
    pub(crate) cases: &'static [(&'static str, &'static str)],
}

/// Every topic.
pub(crate) const TOPICS: &[Topic] = &[
    Topic {
        id: "start",
        group: Group::Egit,
        title: "How egit works",
        what: "egit runs your installed git for everything that reads or changes a repository, \
               and shows what git prints. Nothing is hidden from git: whatever egit does, git \
               on the command line sees the same way, and the other way round. Changes made \
               outside egit (another program, a terminal) show up on their own.",
        git: "git, with --porcelain and -z formats; gitoxide reads some objects faster",
        cases: &[
            (
                "Where is something?",
                "Press ⌘K (Ctrl+K) and type: every command, branch, and recent repository is there.",
            ),
            ("What does this do?", "Hold Alt and point at it."),
            (
                "Did I lose work?",
                "Probably not: Undo takes back what egit did, and the Reflog shows every place HEAD and your branches have been for the last 90 days.",
            ),
        ],
    },
    Topic {
        id: "commit",
        group: Group::Everyday,
        title: "Committing",
        what: "A commit records what is staged (the index) as a snapshot of the whole tree, \
               with your name, the time, a message, and its parent: the commit HEAD was at. \
               The current branch then moves to it. Changes not staged stay in the work tree, \
               uncommitted. The message's first line is its summary; the description follows \
               a blank line.",
        git: "git commit --file=- --cleanup=whitespace (with --amend, --signoff, -S, --trailer as asked)",
        cases: &[
            (
                "Nothing is staged?",
                "git refuses: stage what you mean to commit first (amending can still change the last message).",
            ),
            (
                "A hook refused it?",
                "No commit was made and nothing is lost: what the hook printed says why.",
            ),
            (
                "On a detached HEAD?",
                "The commit belongs to no branch: create one before you switch away, or it is only in the reflog.",
            ),
            (
                "Committed to the wrong branch?",
                "Undo takes the commit back (its changes staged again); switch, then commit there.",
            ),
        ],
    },
    Topic {
        id: "amend",
        group: Group::Rewriting,
        title: "Amending the last commit",
        what: "Amending makes a new commit in place of HEAD's: its changes plus what is staged, \
               with the message as edited, the same parents. Commits never change: the old one \
               still exists (in the reflog), and the branch now points at the new one.",
        git: "git commit --amend",
        cases: &[
            (
                "Already pushed it?",
                "The remote has the old commit: pushing now needs a force push, and anyone who built on it has to rebase.",
            ),
            (
                "Changed your mind?",
                "Undo puts the branch back on the commit as it was.",
            ),
            (
                "Amending a merge?",
                "It stays a merge: its parents are kept.",
            ),
        ],
    },
    Topic {
        id: "reword",
        group: Group::Rewriting,
        title: "Editing a commit's message",
        what: "HEAD's message is changed by amending only its message: what is staged stays \
               staged, out of it. An earlier commit's is changed by a rebase from its parent: \
               that commit and every one after it are made again, with new names, though only \
               one message changed.",
        git: "git commit --amend --only (HEAD's); git rebase --interactive with that commit reworded (earlier ones)",
        cases: &[
            (
                "Was it pushed?",
                "Then its new self and every later commit differ from the remote's: pushing needs a force push.",
            ),
            (
                "Can it conflict?",
                "Not the message itself; the commits replayed after it apply as they did, so it doesn't, unless the work tree is in the way.",
            ),
            (
                "Is it on another branch too?",
                "That branch keeps the old commits: the two now differ from that commit on.",
            ),
            (
                "Regretted?",
                "Undo moves the branch back to where it was before.",
            ),
        ],
    },
    Topic {
        id: "stage",
        group: Group::Everyday,
        title: "Staging and unstaging",
        what: "The index (the staging area) is the next commit being prepared. Staging a file \
               copies its current content into it; unstaging puts HEAD's content back there, \
               leaving the work tree alone. Staging lines or hunks writes the index's copy with \
               only those changes moved across.",
        git: "git add --all, git reset (whole files); for lines, the index entry rewritten through hash-object and update-index",
        cases: &[
            (
                "Changed the file since its diff showed?",
                "Staging lines refuses (the diff is stale): it reads the diff again.",
            ),
            (
                "A submodule?",
                "Staging it records the commit it is at now as the one this repository records.",
            ),
            (
                "An ignored file?",
                ".gitignore keeps it out of the lists; git add --force would add it anyway.",
            ),
        ],
    },
    Topic {
        id: "work",
        group: Group::Everyday,
        title: "Uncommitted changes",
        what: "The work in progress: files changed in the work tree, files staged, and files git \
               doesn't track yet (untracked). The row above the newest commit counts them; \
               clicking it shows them beside the history, with the form that commits.",
        git: "git status --porcelain=v2 -z --untracked-files=all",
        cases: &[
            (
                "Want them out of the way for a while?",
                "Stash them, and apply the stash later.",
            ),
            (
                "Want to throw them away?",
                "A hard reset to HEAD discards tracked changes (untracked files stay).",
            ),
        ],
    },
    Topic {
        id: "branch",
        group: Group::Branches,
        title: "Branches",
        what: "A branch is a name pointing at a commit; committing on it moves it to the new \
               commit. Making one costs nothing and copies nothing. The branch checked out is \
               the current one: HEAD names it.",
        git: "git branch, git branch --move, git branch --delete",
        cases: &[
            (
                "Deleting one not merged?",
                "Its commits stay reachable only from the reflog (90 days): egit asks first, and Undo brings it back.",
            ),
            (
                "Renaming the current branch?",
                "Fine: its upstream setting follows; the remote's branch keeps its old name until you push.",
            ),
            (
                "Two branches at one commit?",
                "Both point at it; they part as soon as one moves.",
            ),
        ],
    },
    Topic {
        id: "switch",
        group: Group::Branches,
        title: "Switching",
        what: "Switching checks another branch out: the work tree and the index take its \
               commit's files, and HEAD names it. Uncommitted changes come along when they \
               don't touch files that differ between the two; otherwise git refuses, and egit \
               offers to stash them first.",
        git: "git switch --no-guess",
        cases: &[
            (
                "A remote's branch?",
                "egit makes a local branch tracking it and switches to that.",
            ),
            (
                "A commit, not a branch?",
                "HEAD is then detached: commits made there belong to no branch until you create one.",
            ),
            (
                "Submodules didn't move?",
                "Switching records other commits for them without moving them, unless submodule.recurse is on: update them.",
            ),
        ],
    },
    Topic {
        id: "detached",
        group: Group::Branches,
        title: "A detached HEAD",
        what: "HEAD normally names a branch; detached, it names a commit. You can look around, \
               build, and even commit, but those commits belong to no branch: switching away \
               leaves them reachable only from the reflog.",
        git: "git switch --detach",
        cases: &[
            (
                "Made commits while detached?",
                "Create a branch here before switching away: they are kept.",
            ),
            (
                "Already switched away?",
                "The Reflog lists where HEAD was: create a branch at that move.",
            ),
            (
                "Inside a submodule?",
                "Normal: submodules are checked out at a commit, not on a branch.",
            ),
        ],
    },
    Topic {
        id: "merge",
        group: Group::Branches,
        title: "Merging",
        what: "Merging brings another branch's commits into the current one. When the current \
               branch has nothing the other lacks, it simply moves forward (a fast-forward); \
               otherwise git makes a merge commit with two parents, holding both sides' changes.",
        git: "git merge --no-edit",
        cases: &[
            (
                "It stopped at conflicts?",
                "Resolve each file (the merge editor), stage it, then Continue; or Abort to go back exactly to before the merge.",
            ),
            (
                "Aborting with changes made before the merge?",
                "git may not restore those cleanly: stash before merging when the work tree isn't clean.",
            ),
            (
                "Already up to date?",
                "The other branch's commits are all in yours: nothing to do.",
            ),
            (
                "Regretted?",
                "Undo moves the branch back to before the merge.",
            ),
        ],
    },
    Topic {
        id: "fast-forward",
        group: Group::Branches,
        title: "Fast-forwarding",
        what: "A branch is fast-forwarded when it moves to a commit that already holds it: no \
               merge commit, nothing replayed, the history stays a line. It is refused when the \
               branch has commits the target lacks.",
        git: "git merge --ff-only (the current branch); git fetch . <ref>:<branch> (another)",
        cases: &[
            (
                "Refused?",
                "The two have diverged: merge or rebase instead.",
            ),
            (
                "A branch checked out in another worktree?",
                "git refuses to move it from here.",
            ),
        ],
    },
    Topic {
        id: "rebase",
        group: Group::Rewriting,
        title: "Rebasing",
        what: "Rebasing replays the current branch's own commits (those the target lacks) on \
               top of the target, one at a time, making new commits with new names. The old \
               ones stay in the reflog. The history becomes a line, as if the work started \
               from the target.",
        git: "git rebase (--rebase-merges to keep merges)",
        cases: &[
            (
                "It stopped at a conflict?",
                "It stopped while replaying one commit: resolve, stage, Continue; Skip leaves that commit out; Abort puts the branch back exactly where it was.",
            ),
            (
                "Cancelled the task part-way?",
                "git stops where it is: the rebase stays in progress (the banner says so) until you Continue or Abort.",
            ),
            (
                "During a rebase, which side is \"current\"?",
                "The branch being rebased onto: \"incoming\" is your commit being replayed. It's the other way round from a merge.",
            ),
            (
                "Its commits were pushed?",
                "The remote has the old ones: pushing needs a force push, and collaborators must rebase too.",
            ),
            (
                "A commit vanished?",
                "One whose change is already in the target is dropped as empty: nothing is lost.",
            ),
        ],
    },
    Topic {
        id: "interactive",
        group: Group::Rewriting,
        title: "Rebasing interactively",
        what: "Rewrites the current branch from a commit on: reorder commits, drop them, squash \
               one into the one before (messages joined) or fix it up (its message dropped), \
               reword messages. Every commit from the first one changed onward gets a new name.",
        git: "git rebase --interactive, its todo written by egit (a sequence editor copies it in)",
        cases: &[
            (
                "Squash the first commit?",
                "There is nothing before it to squash into: refused.",
            ),
            (
                "Dropped a commit others build on?",
                "Later commits may conflict as they're replayed without it.",
            ),
            (
                "Merges in the range?",
                "They are kept, in git's order: they can't be moved or melded, only reworded.",
            ),
            (
                "Stopped at a conflict?",
                "As any rebase: resolve and Continue, or Abort to have it all back as before.",
            ),
        ],
    },
    Topic {
        id: "cherry-pick",
        group: Group::Rewriting,
        title: "Cherry-picking",
        what: "Copies one commit's change onto the current branch as a new commit (new name, \
               same message and author). The original stays where it was.",
        git: "git cherry-pick",
        cases: &[
            (
                "Its change is already here?",
                "The result would be empty: egit skips it and says so.",
            ),
            ("Conflicts?", "Resolve, stage, Continue; or Abort."),
            ("Picked the wrong one?", "Undo takes it back."),
        ],
    },
    Topic {
        id: "revert",
        group: Group::Rewriting,
        title: "Reverting",
        what: "Makes a new commit that undoes a commit's change. History keeps both: nothing is \
               rewritten, so it's safe on pushed branches.",
        git: "git revert --no-edit (a merge against its first parent)",
        cases: &[
            (
                "Reverted a merge, then want the branch again?",
                "Merging it again brings nothing back: git thinks it's merged. Revert the revert instead.",
            ),
            (
                "Conflicts?",
                "Later commits changed the same lines: resolve, stage, Continue.",
            ),
        ],
    },
    Topic {
        id: "reset",
        group: Group::Rewriting,
        title: "Resetting",
        what: "Moves the current branch to another commit. Soft keeps everything the move \
               leaves behind staged; mixed keeps it, not staged; keep keeps your uncommitted \
               changes and refuses where they'd be overwritten; hard throws away every \
               uncommitted change to tracked files.",
        git: "git reset --soft, --mixed, --keep, --hard",
        cases: &[
            (
                "The commits it moved past?",
                "Still in the reflog (and Undo takes the reset back).",
            ),
            (
                "Hard reset lost changes?",
                "Uncommitted changes git never stored are gone: that's why egit asks first. Untracked files are left alone.",
            ),
        ],
    },
    Topic {
        id: "stash",
        group: Group::Everyday,
        title: "Stashing",
        what: "Saves your uncommitted changes (staged and not, and untracked files when asked) \
               in a stash and cleans the work tree. Applying puts them back and keeps the \
               stash; popping applies, then drops it.",
        git: "git stash push, apply, pop, drop",
        cases: &[
            (
                "Applying conflicts?",
                "The files changed since: resolve them; a pop that conflicts keeps the stash.",
            ),
            ("Dropped one by mistake?", "Undo stores it back."),
            (
                "Where are stashes kept?",
                "In refs/stash and its reflog: they're local, never pushed.",
            ),
        ],
    },
    Topic {
        id: "fetch",
        group: Group::Remotes,
        title: "Fetching",
        what: "Downloads the remotes' new commits and moves your remote-tracking branches \
               (origin/main) to where the remotes' branches are. It never changes your own \
               branches or files: you see what's new before taking it. Pruning removes \
               remote-tracking branches whose branches were deleted on the remote.",
        git: "git fetch --all --prune",
        cases: &[
            (
                "In the background?",
                "egit fetches every few minutes (Preferences), never asking for a password: a fetch that would ask fails quietly.",
            ),
            (
                "Something new came?",
                "A line under the top bar says how many commits your branch's upstream gained.",
            ),
        ],
    },
    Topic {
        id: "pull",
        group: Group::Remotes,
        title: "Pulling",
        what: "Fetches, then brings the upstream into the current branch: by a merge (git's \
               default), a rebase (pull.rebase), or only a fast-forward, as configured.",
        git: "git pull (--rebase, --no-rebase, --ff-only)",
        cases: &[
            (
                "The branches diverged?",
                "egit asks: merge or rebase, and can remember the choice (pull.rebase).",
            ),
            (
                "Uncommitted changes in the way?",
                "git refuses: commit or stash them first.",
            ),
            (
                "Conflicts?",
                "Resolve as for a merge or a rebase, whichever the pull did.",
            ),
            (
                "Submodules show as moved after?",
                "The pull recorded new commits for them: update them (the banner offers to).",
            ),
        ],
    },
    Topic {
        id: "push",
        group: Group::Remotes,
        title: "Pushing",
        what: "Uploads the current branch's commits to its upstream and moves the remote's \
               branch to them. The remote refuses when it has commits you don't: fetch and \
               integrate them first. A branch's first push sets its upstream.",
        git: "git push --porcelain (--set-upstream the first time)",
        cases: &[
            (
                "Rejected as behind?",
                "Someone pushed first: pull, then push.",
            ),
            (
                "Rewrote pushed commits?",
                "Only a force push replaces them.",
            ),
            (
                "Tags?",
                "Pushing a branch pushes no tags: push each tag from its menu.",
            ),
            (
                "LFS files?",
                "Pushing commits pushes the LFS files they need, when LFS is set up.",
            ),
        ],
    },
    Topic {
        id: "force-push",
        group: Group::Remotes,
        title: "Force pushing",
        what: "Replaces the remote's branch with yours: commits only the remote had are dropped \
               from it. egit forces with a lease: the push is refused if the remote's branch \
               moved since you last fetched (someone else pushed), so you don't destroy work you \
               haven't seen.",
        git: "git push --force-with-lease --force-if-includes",
        cases: &[
            (
                "Refused even with force?",
                "The lease failed: fetch, look at what came, then decide.",
            ),
            (
                "Others had pulled the old commits?",
                "They must rebase onto the new ones (or reset to them).",
            ),
        ],
    },
    Topic {
        id: "upstream",
        group: Group::Remotes,
        title: "Upstreams",
        what: "A branch's upstream is the remote branch it follows: Pull pulls from it, Push \
               pushes to it, and ↑ ↓ count the commits each side has that the other lacks.",
        git: "git branch --set-upstream-to, --unset-upstream",
        cases: &[
            (
                "It says \"gone\"?",
                "The remote's branch was deleted (and pruned here): push to make it again, or forget the upstream.",
            ),
            ("Not pushed yet?", "The first push sets it."),
        ],
    },
    Topic {
        id: "remote",
        group: Group::Remotes,
        title: "Remotes",
        what: "Other copies of the repository you fetch from and push to, each named (origin by \
               convention) with an address. Their branches show here as remote-tracking \
               branches (origin/main), moved only by fetching.",
        git: "git remote add, rename, remove, set-url",
        cases: &[
            (
                "Renaming one?",
                "Its remote-tracking branches and the branches following it move along.",
            ),
            (
                "Removing one?",
                "Its remote-tracking branches go; the remote itself is untouched.",
            ),
        ],
    },
    Topic {
        id: "tag",
        group: Group::Branches,
        title: "Tags",
        what: "A tag names a commit and doesn't move. Lightweight tags are just names; annotated \
               ones also hold a message, who made them, and when.",
        git: "git tag, git tag --annotate, git push <remote> <tag>",
        cases: &[
            (
                "Pushed my branch, but not its tag?",
                "Tags are pushed on their own: push it from its menu.",
            ),
            (
                "Deleted a tag here?",
                "The remotes keep theirs until it's deleted there too.",
            ),
        ],
    },
    Topic {
        id: "worktree",
        group: Group::Inside,
        title: "Worktrees",
        what: "More work trees for the same repository, each checking out its own branch: work \
               on two branches at once without stashing. They share commits, branches, and \
               stashes.",
        git: "git worktree add, remove, lock, unlock, prune",
        cases: &[
            (
                "A branch can't be checked out?",
                "It's checked out in another worktree: git allows one at a time.",
            ),
            (
                "Removing one with changes?",
                "egit asks again: its uncommitted changes would be lost.",
            ),
            (
                "Its folder was deleted?",
                "Prune forgets it; locked ones (on removable drives) are kept.",
            ),
        ],
    },
    Topic {
        id: "submodules",
        group: Group::Inside,
        title: "Submodules",
        what: "A submodule is another repository checked out inside this one, at a commit this \
               one records (not on a branch: its HEAD is detached, normally). This repository \
               stores only that commit's name; .gitmodules says where to clone it from.",
        git: "git submodule status, update --init --recursive, sync, deinit; git rm",
        cases: &[
            (
                "Its folder is empty?",
                "It isn't set up here: set it up (or Mend everything), which clones it and checks out the commit recorded.",
            ),
            (
                "It's \"at another commit\" after a pull?",
                "The pull recorded a newer commit without moving it: update it to the commit recorded.",
            ),
            (
                "It's at another commit and I committed in it?",
                "Record its commit here (stage its path) and commit: that's how a submodule moves on.",
            ),
            (
                "Fetching it fails after its address changed?",
                "Sync addresses: the address set up here is stale.",
            ),
            (
                "Forget it here, or remove it?",
                "Forgetting (deinit) only empties it here; removing deletes it from the repository (to commit).",
            ),
            (
                "Tired of updating them by hand?",
                "\"Update them on every switch and pull\" sets submodule.recurse: git moves them along.",
            ),
        ],
    },
    Topic {
        id: "submodule-update",
        group: Group::Inside,
        title: "Updating submodules",
        what: "To the commits recorded: each submodule checks out the commit this repository \
               records (cloning it first if needed): what a fresh checkout should look like. To \
               their branches' latest: each fetches its branch and checks out its newest \
               commit; this repository then shows them moved, to record and commit.",
        git: "git submodule update --init --recursive (--remote for the latest)",
        cases: &[
            (
                "Commits I made in it?",
                "Updating to the commit recorded leaves them only in its reflog: record them here first if you want them.",
            ),
            (
                "Changes in it in the way?",
                "git refuses for that submodule: commit or stash them inside it.",
            ),
        ],
    },
    Topic {
        id: "submodule-mend",
        group: Group::Inside,
        title: "Mending submodules",
        what: "Does what usually gets a checkout's submodules working: brings every address set \
               up here in line with .gitmodules, then sets up every submodule and checks out \
               the commit recorded, nested ones too.",
        git: "git submodule sync --recursive, then git submodule update --init --recursive",
        cases: &[
            (
                "A submodule you moved on yourself?",
                "It's put back at the commit recorded: record it first if you meant to keep it.",
            ),
            (
                "Still failing?",
                "Its address may be private or gone: the problem says what git couldn't fetch.",
            ),
        ],
    },
    Topic {
        id: "lfs",
        group: Group::Inside,
        title: "Git LFS",
        what: "Large files kept apart: git commits a small pointer for each file a pattern \
               tracks, and the file itself lives in the remote's LFS store. LFS's filters turn \
               pointers into files when checking out, and its hooks upload files when pushing.",
        git: "git lfs install --local, track, untrack, fetch, checkout, pull, push, prune, locks",
        cases: &[
            (
                "Files are tiny text pointers?",
                "LFS isn't set up here, or the files weren't downloaded: set it up, then Pull LFS files.",
            ),
            (
                "Tracked a pattern but old files are still in git?",
                "Tracking only applies to files added from now on: add them again (or migrate history).",
            ),
            (
                "A lockable file is read-only?",
                "Lock it first: whoever holds the lock may change and push it.",
            ),
            (
                "Prune deleted something I need?",
                "Only files no recent commit or branch needs: fetching brings them back.",
            ),
        ],
    },
    Topic {
        id: "conflicts",
        group: Group::Conflicts,
        title: "Conflicts",
        what: "A conflict is where both sides changed the same lines differently: git can't \
               choose. The operation stops; each file in conflict is marked, and stays so until \
               its resolution is staged. Then Continue finishes the operation, or Abort puts \
               everything back as it was before it started.",
        git: "git merge-file --zdiff3; git update-index for each resolution; --continue, --abort, --skip",
        cases: &[
            (
                "Which side is mine?",
                "In a merge, current is your branch. In a rebase, current is the branch you're rebasing onto and incoming is your own commit being replayed.",
            ),
            (
                "What's the base?",
                "What both sides started from: seeing it often shows what each side meant.",
            ),
            (
                "Resolved it in another editor?",
                "Stage the file: egit takes the work tree's version as the resolution.",
            ),
        ],
    },
    Topic {
        id: "merge-editor",
        group: Group::Conflicts,
        title: "The merge editor",
        what: "Each conflict, one at a time: take the current side, the incoming one, both (in \
               either order), the base, or edit the text. Above, each side whole (the file as \
               that side has it), scrolled to the conflict. Saving the resolution writes the \
               file and stages it.",
        git: "git merge-file; git hash-object, update-index, checkout-index",
        cases: &[
            (
                "Took the wrong side?",
                "Reopen it (More), or pick again before saving.",
            ),
            (
                "A side deleted the file?",
                "There are no lines to merge: keep a side's file, or delete it.",
            ),
        ],
    },
    Topic {
        id: "bisect",
        group: Group::Looking,
        title: "Bisecting",
        what: "Finds the commit that brought a bug, by halves: mark a commit bad and one good, \
               git checks out the commit halfway between, you test and mark it, and so on until \
               one is left: the first bad commit.",
        git: "git bisect start, bad, good, skip, reset",
        cases: &[
            (
                "A commit can't be tested?",
                "Skip it: git picks another nearby (the result may then be a few candidates).",
            ),
            ("Done?", "Stop bisecting: HEAD goes back to where it was."),
        ],
    },
    Topic {
        id: "reflog",
        group: Group::Looking,
        title: "The reflog",
        what: "Every move of HEAD and of each branch, kept locally for about 90 days: commits \
               \"lost\" to a reset, a rebase, an amend, or a deleted branch are still listed \
               there, and a branch made at one of them keeps them for good.",
        git: "git log --walk-reflogs",
        cases: &[(
            "Is it shared?",
            "No: it's yours alone, never pushed or fetched.",
        )],
    },
    Topic {
        id: "undo",
        group: Group::Egit,
        title: "Undo and Redo",
        what: "Undo takes back what egit did (commits, merges, rebases, switches, branches, tags, \
               stashes, resets), checking first that nothing moved since; Redo does it again. \
               They work from what git keeps: commits are never destroyed by them.",
        git: "git reset, switch, branch, tag, stash store as each needs",
        cases: &[
            (
                "Pushes and fetches?",
                "Not undone: they changed another repository too.",
            ),
            (
                "Undo refused?",
                "The repository moved on since (another commit, a switch outside egit): it won't guess.",
            ),
        ],
    },
    Topic {
        id: "blame",
        group: Group::Looking,
        title: "Blame",
        what: "Each line, with the last commit that changed it. Renames are followed; \"Follow \
               moved lines\" also finds lines moved within the file (slower).",
        git: "git blame --incremental (-M)",
        cases: &[(
            "A whole-file reformat hides the real change?",
            "Blame before that commit: its parent's view of the line.",
        )],
    },
    Topic {
        id: "history",
        group: Group::Looking,
        title: "The commit graph",
        what: "Every commit, newest first, each joined to its parents by lines: a branch's \
               commits run down one lane, merges join two (a hollow node), the current commit \
               is ringed. Badges are the branches and tags pointing at a commit. The row above \
               the newest shows uncommitted changes.",
        git: "git rev-list --topo-order --parents; refs from for-each-ref",
        cases: &[
            (
                "Too many branches?",
                "Hide some with their eye in the sidebar, or show only one: only what's shown changes.",
            ),
            (
                "Slow on a huge repository?",
                "Write a commit-graph file (egit offers one): git walks history far faster with it.",
            ),
        ],
    },
    Topic {
        id: "signing",
        group: Group::Everyday,
        title: "Signed commits",
        what: "A signature (OpenPGP, SSH, or X.509) proves who made a commit. egit signs as your \
               git configuration says (commit.gpgSign, gpg.format, user.signingKey) and checks \
               the signatures on screen.",
        git: "git commit -S; git log --show-signature",
        cases: &[
            (
                "Asked for a passphrase?",
                "egit relays your signing program's question.",
            ),
            (
                "\"Unchecked\" or doubtful?",
                "The key isn't one you trust yet (gpg's trust, or gpg.ssh.allowedSignersFile).",
            ),
        ],
    },
    Topic {
        id: "co-authors",
        group: Group::Everyday,
        title: "Co-authors",
        what: "People who wrote a commit with you, as Co-authored-by lines at the message's end \
               (git trailers): GitHub and GitLab credit them too.",
        git: "git commit --trailer \"Co-authored-by: Name <email>\"",
        cases: &[(
            "Someone shows twice?",
            "Two emails for one person: a .mailmap file joins them.",
        )],
    },
    Topic {
        id: "patches",
        group: Group::Everyday,
        title: "Patches",
        what: "Commits or changes as text, to send or apply elsewhere. A mailbox patch keeps \
               each commit's author and message and applies as commits; a plain diff applies \
               as changes.",
        git: "git format-patch, git am --3way, git apply (--3way)",
        cases: &[(
            "Applying stopped?",
            "Resolve its conflicts and Continue, Skip that patch, or Abort.",
        )],
    },
    Topic {
        id: "search",
        group: Group::Looking,
        title: "Searching the history",
        what: "Finds commits by message, author, changed path, or name: the matches are marked \
               in the graph, and Enter steps through them.",
        git: "git rev-list --grep, --author, -- <path>; rev-parse",
        cases: &[(
            "Case?",
            "Smart: all lower case matches any case; a capital matches exactly.",
        )],
    },
    Topic {
        id: "drag",
        group: Group::Branches,
        title: "Dragging a branch",
        what: "Drop a branch's badge (or sidebar row) on another's: onto the current branch to \
               merge it in or rebase onto it; drag the current branch onto another to rebase \
               onto that.",
        git: "git merge, git rebase, git merge --ff-only",
        cases: &[(
            "Merging into a branch not checked out?",
            "git merges into the current branch only: switch to it first.",
        )],
    },
    Topic {
        id: "pull-requests",
        group: Group::Remotes,
        title: "Pull and merge requests",
        what: "Read from GitHub's or GitLab's API with your git credentials: open requests, their \
               checks and reviews. Checking one out fetches its head and switches to it.",
        git: "the forge's API; git fetch <remote> refs/pull/N/head (or refs/merge-requests/N/head)",
        cases: &[(
            "Asked to sign in?",
            "A token with read access (repo, read_api) lets egit read them.",
        )],
    },
    Topic {
        id: "credentials",
        group: Group::Egit,
        title: "Passwords and keys",
        what: "When git or ssh asks for a password, a passphrase, or a host key, egit shows the \
               question and hands your answer back (askpass), wiping it after. Credential \
               helpers remember them, as configured.",
        git: "GIT_ASKPASS, SSH_ASKPASS; git credential fill, approve, reject",
        cases: &[(
            "It never asks?",
            "Background fetches never ask: a fetch that would ask fails quietly instead.",
        )],
    },
    Topic {
        id: "discard",
        group: Group::Everyday,
        title: "Deleting",
        what: "Deleting a branch, a tag, a stash, a remote's branch: egit says what would be lost \
               first. Local deletions can be undone; a remote's branch deleted is gone for \
               everyone until pushed again.",
        git: "git branch --delete, git tag --delete, git stash drop, git push --delete",
        cases: &[(
            "Not merged?",
            "Its commits would be reachable only from the reflog: egit asks again.",
        )],
    },
];

/// The topic named `id`.
pub(crate) fn topic(id: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|topic| topic.id == id)
}

/// Controls whose words are exactly these, and their topics.
const EXACT: &[(&str, &str)] = &[
    ("Push", "push"),
    ("Pull", "pull"),
    ("Fetch", "fetch"),
    ("Undo", "undo"),
    ("Redo", "undo"),
    ("Amend", "amend"),
    ("Sign off", "commit"),
    ("Sign", "signing"),
    ("Search", "search"),
    ("Reflog", "reflog"),
    ("Patches", "patches"),
    ("LFS", "lfs"),
    ("Git LFS", "lfs"),
    ("Submodules", "submodules"),
    ("Submodules…", "submodules"),
    ("Mend everything", "submodule-mend"),
    ("Continue", "conflicts"),
    ("Abort…", "conflicts"),
    ("Abort", "conflicts"),
    ("Skip", "conflicts"),
    ("Save resolution", "merge-editor"),
    ("Current", "merge-editor"),
    ("Incoming", "merge-editor"),
    ("Both", "merge-editor"),
    ("Base", "merge-editor"),
    ("Edit…", "merge-editor"),
    ("Show the base", "conflicts"),
    ("Both sides above", "merge-editor"),
    ("Stage file", "conflicts"),
    ("Add a co-author", "co-authors"),
    ("Stage all", "stage"),
    ("Unstage all", "stage"),
    ("Stash the changes", "stash"),
    ("Force push…", "force-push"),
    ("Blame", "blame"),
    ("File history", "blame"),
    ("Repository", "start"),
    ("Commands", "start"),
    ("Help", "start"),
];

/// Controls whose words start so, and their topics (the first that fits).
const STARTS: &[(&str, &str)] = &[
    ("Commit ", "commit"),
    ("Amend the last commit", "amend"),
    ("Edit the message", "reword"),
    ("Save message", "reword"),
    ("Rebase interactively", "interactive"),
    ("Squash", "interactive"),
    ("Drop this commit", "interactive"),
    ("Rebase ", "rebase"),
    ("Merge ", "merge"),
    ("Fast-forward ", "fast-forward"),
    ("Cherry-pick", "cherry-pick"),
    ("Revert", "revert"),
    ("Reset ", "reset"),
    ("Soft", "reset"),
    ("Mixed", "reset"),
    ("Keep", "reset"),
    ("Hard", "reset"),
    ("Stage ", "stage"),
    ("Unstage ", "stage"),
    ("Stash", "stash"),
    ("Apply", "stash"),
    ("Pop", "stash"),
    ("Drop", "stash"),
    ("Switch ", "switch"),
    ("Create a branch", "branch"),
    ("Rename", "branch"),
    ("Delete ", "discard"),
    ("Push ", "push"),
    ("Pull ", "pull"),
    ("Fetch ", "fetch"),
    ("Set upstream", "upstream"),
    ("Forget ", "upstream"),
    ("Tag", "tag"),
    ("Create a tag", "tag"),
    ("Add a remote", "remote"),
    ("Change address", "submodules"),
    ("Update ", "submodule-update"),
    ("Set it up", "submodules"),
    ("Record its commit", "submodules"),
    ("Sync", "submodules"),
    ("Forget it here", "submodules"),
    ("Remove…", "submodules"),
    ("Add a submodule", "submodules"),
    ("Add a worktree", "worktree"),
    ("Create a worktree", "worktree"),
    ("Lock", "worktree"),
    ("Unlock", "worktree"),
    ("Open in a tab", "worktree"),
    ("Set up Git LFS", "lfs"),
    ("Track", "lfs"),
    ("Stop tracking", "lfs"),
    ("Fetch every file", "lfs"),
    ("Check out", "lfs"),
    ("Prune", "lfs"),
    ("Read the locks", "lfs"),
    ("Bisect", "bisect"),
    ("Mark ", "bisect"),
    ("Good", "bisect"),
    ("Bad", "bisect"),
    ("Stop bisecting", "bisect"),
    ("Save as a patch", "patches"),
    ("Copy as a patch", "patches"),
    ("Apply patches", "patches"),
    ("Show the files", "history"),
    ("Hide ", "history"),
    ("Show only", "history"),
    ("Pin ", "history"),
    ("Write a commit-graph", "history"),
];

/// The topic a control's words point to: exact words first, then their start.
pub(crate) fn for_label(label: &str) -> Option<&'static Topic> {
    let label = label.trim();
    // A tool's words follow its icon: "  Push".
    let label = label.split_once("  ").map_or(label, |(icon, words)| {
        if icon.chars().count() <= 2 {
            words
        } else {
            label
        }
    });
    if let Some((_, id)) = EXACT.iter().find(|(words, _)| *words == label) {
        return topic(id);
    }
    STARTS
        .iter()
        .find(|(words, _)| label.starts_with(words))
        .and_then(|(_, id)| topic(id))
}

/// The help window: every topic, by group, searchable.
#[derive(Debug)]
pub(crate) struct HelpWindow {
    open: bool,
    query: String,
    selected: &'static str,
}

impl Default for HelpWindow {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: "start",
        }
    }
}

impl HelpWindow {
    /// Opens it, or closes it.
    pub(crate) const fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Whether it is open.
    #[cfg(test)]
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }
}

/// Draws the help window, while open: the topics on the left (those whose words hold what is
/// typed), the one selected on the right.
pub(crate) fn window(ctx: &Context, help: &mut HelpWindow) {
    if !help.open {
        return;
    }
    let palette = Palette::of(ctx);
    let screen = ctx.content_rect();
    let size = egui::vec2(
        (screen.width() - 48.0).clamp(320.0, 900.0),
        (screen.height() - 96.0).clamp(240.0, 620.0),
    );
    let modal = egui::Modal::new(Id::new("help-window")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}  Help", icon::QUESTION))
                    .size(crate::theme::TITLE)
                    .semibold(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::widgets::icon_tool(ui, true, icon::X, "Close").clicked() {
                    help.open = false;
                }
                ui.label(crate::widgets::muted(
                    ui,
                    "Hold Alt over anything to see its topic",
                ));
            });
        });
        ui.add_space(8.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(240.0, size.y),
                egui::Layout::top_down_justified(egui::Align::Min),
                |ui| topics_list(ui, help, palette),
            );
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("help-topic")
                .max_height(size.y)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        if let Some(topic) = topic(help.selected) {
                            body(ui, topic, palette);
                        }
                    });
                });
        });
    });
    if modal.should_close() {
        help.open = false;
    }
}

/// The topics, by group, those holding what is typed.
fn topics_list(ui: &mut Ui, help: &mut HelpWindow, palette: Palette) {
    let label = ui.label(crate::widgets::muted(ui, "Find a topic"));
    ui.add(crate::widgets::line_field(&mut help.query).hint_text("rebase, submodule, push…"))
        .labelled_by(label.id);
    let query = help.query.trim().to_lowercase();
    let matches = |topic: &Topic| {
        query.is_empty()
            || [topic.title, topic.what, topic.git]
                .iter()
                .any(|text| text.to_lowercase().contains(&query))
            || topic.cases.iter().any(|(question, answer)| {
                question.to_lowercase().contains(&query) || answer.to_lowercase().contains(&query)
            })
    };
    egui::ScrollArea::vertical()
        .id_salt("help-topics")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in Group::ALL {
                let shown: Vec<&Topic> = TOPICS
                    .iter()
                    .filter(|topic| topic.group == group && matches(topic))
                    .collect();
                if shown.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(group.name())
                        .small()
                        .color(palette.muted)
                        .semibold(),
                );
                for topic in shown {
                    let selected = help.selected == topic.id;
                    if ui.selectable_label(selected, topic.title).clicked() {
                        help.selected = topic.id;
                    }
                }
            }
        });
}

/// What was noted this frame: each control's place, its layer, and what it says or its topic.
#[derive(Clone, Default)]
struct Noted(Arc<Mutex<Vec<(Rect, LayerId, Said)>>>);

/// A noted control's words, or its topic.
#[derive(Clone)]
enum Said {
    Words(String),
    Topic(&'static str),
}

fn noted(ctx: &Context) -> Noted {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Noted>(Id::new("help-noted"))
            .clone()
    })
}

/// Whether Alt is held: only then is anything noted.
fn asking(ui: &Ui) -> bool {
    ui.input(|input| input.modifiers.alt)
}

/// Notes a control drawn at `rect` saying `words`, while Alt is held.
pub(crate) fn note(ui: &Ui, rect: Rect, words: &str) {
    if asking(ui) {
        noted(ui.ctx())
            .0
            .lock()
            .push((rect, ui.layer_id(), Said::Words(words.to_owned())));
    }
}

/// Notes something drawn at `rect` whose topic is `id`, while Alt is held.
pub(crate) fn note_topic(ui: &Ui, rect: Rect, id: &'static str) {
    if asking(ui) {
        noted(ui.ctx())
            .0
            .lock()
            .push((rect, ui.layer_id(), Said::Topic(id)));
    }
}

/// At a frame's start: what was noted goes, and tooltips wait while Alt is held (the topic
/// takes their place).
pub(crate) fn begin_frame(ctx: &Context) {
    noted(ctx).0.lock().clear();
    let alt = ctx.input(|input| input.modifiers.alt);
    let delay = if alt {
        f32::MAX
    } else {
        crate::theme::TOOLTIP_DELAY
    };
    let current = ctx.global_style().interaction.tooltip_delay;
    if (current - delay).abs() > f32::EPSILON {
        ctx.all_styles_mut(|style| style.interaction.tooltip_delay = delay);
    }
}

/// At a frame's end, with Alt held: the topic of the topmost control under the pointer (the
/// smallest among those noted on the topmost layer there), beside it.
pub(crate) fn show(ctx: &Context) {
    if !ctx.input(|input| input.modifiers.alt) {
        return;
    }
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return;
    };
    let layer = ctx.layer_id_at(pointer);
    let found = {
        let noted = noted(ctx);
        let noted = noted.0.lock();
        noted
            .iter()
            .filter(|(rect, at, _)| rect.contains(pointer) && Some(*at) == layer)
            .filter_map(|(rect, _, said)| {
                let topic = match said {
                    Said::Words(words) => for_label(words),
                    Said::Topic(id) => topic(id),
                }?;
                Some((rect.area(), *rect, topic))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, rect, topic)| (rect, topic))
    };
    let Some((rect, topic)) = found else {
        return;
    };
    let palette = Palette::of(ctx);
    // Beside the control, below it where there is room.
    let at = Pos2::new(rect.left(), rect.bottom() + 6.0);
    egui::Area::new(Id::new("help-osd"))
        .order(Order::Tooltip)
        .fixed_pos(at)
        .constrain(true)
        .interactable(false)
        .pivot(Align2::LEFT_TOP)
        .show(ctx, |ui| osd(ui, topic, palette));
}

/// A topic, as the Alt overlay shows it.
fn osd(ui: &mut Ui, topic: &Topic, palette: Palette) {
    Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.5, palette.accent))
        .corner_radius(crate::theme::RADIUS_BOX)
        .inner_margin(Margin::same(14))
        .shadow(ui.style().visuals.popup_shadow)
        .show(ui, |ui| {
            ui.set_max_width(420.0);
            body(ui, topic, palette);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{}  Alt shows this anywhere · F1 shows every topic",
                    icon::INFO
                ))
                .small()
                .color(palette.muted),
            );
        });
}

/// A topic's title, what it does, its git, and its cases.
pub(crate) fn body(ui: &mut Ui, topic: &Topic, palette: Palette) {
    ui.label(
        RichText::new(topic.title)
            .size(crate::theme::HEADING)
            .semibold(),
    );
    ui.add_space(4.0);
    ui.add(egui::Label::new(topic.what).wrap());
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            RichText::new(format!("egit runs: {}", topic.git))
                .monospace()
                .small()
                .color(palette.muted),
        )
        .wrap(),
    );
    if topic.cases.is_empty() {
        return;
    }
    ui.add_space(8.0);
    for (question, answer) in topic.cases {
        ui.add(egui::Label::new(RichText::new(*question).medium().color(palette.text)).wrap());
        ui.add(egui::Label::new(RichText::new(*answer).color(text_soft(palette))).wrap());
        ui.add_space(4.0);
    }
}

/// An answer's colour: the muted text's, which reads on every surface.
const fn text_soft(palette: Palette) -> Color32 {
    palette.muted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_is_named_once_and_said_whole() {
        for (at, topic) in TOPICS.iter().enumerate() {
            assert!(
                TOPICS.iter().skip(at + 1).all(|other| other.id != topic.id),
                "{} twice",
                topic.id
            );
            assert!(
                !topic.title.is_empty() && topic.what.len() > 40,
                "{}",
                topic.id
            );
            assert!(!topic.what.contains("  "), "{}: doubled spaces", topic.id);
        }
        for group in Group::ALL {
            assert!(
                TOPICS.iter().any(|topic| topic.group == group),
                "{}",
                group.name()
            );
        }
    }

    #[test]
    fn controls_find_their_topics_by_their_words() {
        let id = |label: &str| for_label(label).map(|topic| topic.id);
        assert_eq!(id("Push"), Some("push"));
        assert_eq!(
            id("\u{e13a}  Push"),
            Some("push"),
            "a tool's icon is skipped"
        );
        assert_eq!(id("Merge topic into main"), Some("merge"));
        assert_eq!(id("Rebase main onto topic, keeping merges"), Some("rebase"));
        assert_eq!(id("Rebase interactively from here…"), Some("interactive"));
        assert_eq!(id("Mend everything"), Some("submodule-mend"));
        assert_eq!(id("Stage src/main.rs"), Some("stage"));
        assert_eq!(id("Nothing at all"), None);
        // Every topic a word points to exists.
        for label in [
            "Push",
            "Commit 2 files",
            "Update all to the commits recorded",
            "Set up Git LFS here",
        ] {
            assert!(for_label(label).is_some(), "{label}");
        }
    }
}
