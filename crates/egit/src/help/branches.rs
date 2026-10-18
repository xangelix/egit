//! Branches and tags, and what the commit and branch menus do with them.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "branch",
        group: Group::Branches,
        title: "Branches",
        on: &["Create a branch*"],
        what: "A branch is a name pointing at a commit; committing on it moves it to the new \
               commit. Making one costs nothing and copies nothing: “Create a branch here…” \
               writes one name at the commit you clicked. The branch checked out is the \
               current one: HEAD names it.",
        git: "git branch <name> <commit> (git switch --create with “Switch to it”)",
        cases: &[
            (
                "Deleting one not merged?",
                "Its commits stay reachable only from the reflog (90 days): egit asks first, and Undo brings it back.",
            ),
            (
                "Two branches at one commit?",
                "Both point at it; they part as soon as one moves.",
            ),
            (
                "A double click on a branch's badge?",
                "Switches to it. Its menu (right click) holds everything else; dragging it onto another branch merges or rebases.",
            ),
            (
                "Topic and topic on a Mac or Windows?",
                "Branch names differ by case for git, but those file systems don't tell them apart: avoid names that differ only in case.",
            ),
        ],
    },
    Topic {
        id: "switch-to-it",
        group: Group::Branches,
        title: "Switch to it",
        on: &["Switch to it"],
        what: "Ticked, the new branch is made and checked out at once, so your next commit goes \
               on it. Unticked, the branch is only made: you stay where you are, and it waits \
               at its commit until you switch to it.",
        git: "git switch --no-guess --create <name> <commit> (ticked); git branch <name> <commit>",
        cases: &[
            (
                "Uncommitted changes?",
                "They come along to the new branch, as long as they don't touch files that differ between the two commits; otherwise git refuses and nothing is made.",
            ),
            (
                "Made at the commit you're on?",
                "Nothing in the work tree changes: the new branch starts where you are.",
            ),
        ],
    },
    Topic {
        id: "branch-names",
        group: Group::Branches,
        title: "What a branch or tag may be called",
        on: &[],
        what: "git's rules for names (git check-ref-format): no spaces, no ~ ^ : ? * [ or \
               backslash, no “..”, no “@{”, not starting with “-” or “.”, not ending with \
               “/”, “.” or “.lock”. A “/” makes folders: feature/login sits in feature/ in the \
               sidebar. egit says what is wrong as you type.",
        git: "git check-ref-format's rules, checked by egit",
        cases: &[
            (
                "feature exists, and now feature/login won't do?",
                "A name can't be both a branch and a folder of branches: rename or delete feature first.",
            ),
            (
                "Names that differ only by case?",
                "git keeps them apart, but macOS and Windows file systems don't: Fix and fix collide there. Pick one spelling.",
            ),
            (
                "A branch and a tag with one name?",
                "git allows it, but commands then have to guess which you mean: keep them apart.",
            ),
        ],
    },
    Topic {
        id: "rename-branch",
        group: Group::Branches,
        title: "Renaming a branch",
        on: &["Rename *", "Rename"],
        what: "Gives a branch a new name: the commits stay as they are, and its settings (its \
               upstream) and its reflog go with it. Only the name here changes: a remote's \
               copy keeps the old name. In the remote dialog, Rename renames the remote and its \
               remote-tracking branches alike.",
        git: "git branch --move <old> <new> (git remote rename for a remote)",
        cases: &[
            (
                "It was pushed already?",
                "The remote still has the old name, and the upstream still points there: push under the new name (Push… or Set upstream…), then delete the old one on the remote.",
            ),
            (
                "Renaming the current branch?",
                "Fine: HEAD follows the new name, and nothing in the work tree changes.",
            ),
            (
                "Colleagues have the old branch?",
                "Their copies keep the old name, and their upstream points at a branch that will be gone: tell them, or keep the old name.",
            ),
            ("Regretted?", "Undo renames it back."),
        ],
    },
    Topic {
        id: "switch",
        group: Group::Branches,
        title: "Switching",
        on: &["Switch *"],
        what: "Switching checks another branch out: the work tree and the index take its \
               commit's files, and HEAD names it. Uncommitted changes come along when they \
               don't touch files that differ between the two; otherwise git refuses, and egit \
               offers to stash them first.",
        git: "git switch --no-guess",
        cases: &[
            (
                "A remote's branch?",
                "egit makes a local branch tracking it and switches to that; if a local branch of that name exists, it switches to that one instead.",
            ),
            (
                "A commit, not a branch?",
                "HEAD is then detached: commits made there belong to no branch until you create one.",
            ),
            (
                "Submodules didn't move?",
                "Switching records other commits for them without moving them, unless submodule.recurse is on: update them.",
            ),
            (
                "Ignored files (build output) left behind?",
                "git leaves files it ignores alone: they stay in the work tree whichever branch is checked out.",
            ),
            ("Switched by mistake?", "Undo switches back."),
        ],
    },
    Topic {
        id: "switch-tracking",
        group: Group::Branches,
        title: "Switching to a remote's branch",
        on: &["Switch to *, tracking *"],
        what: "A remote's branch (origin/topic) can't be checked out to work on: it moves only \
               when you fetch. So egit makes a local branch of the same name at the same commit, \
               sets origin/topic as its upstream, and switches to it. Pulls and pushes of the \
               new branch then go to origin/topic.",
        git: "git switch --no-guess --create <topic> --track <origin/topic>",
        cases: &[
            (
                "There is a local branch of that name already?",
                "egit switches to that one rather than make another (the menu then says “Switch to topic”), even if it is behind: pull or fast-forward it after.",
            ),
            (
                "The remote's branch looks old?",
                "It is as of your last fetch: fetch first to start from the remote's latest.",
            ),
        ],
    },
    Topic {
        id: "detached",
        group: Group::Branches,
        title: "A detached HEAD",
        on: &["Switch here, without a branch"],
        what: "HEAD normally names a branch; detached, it names a commit. You can look around, \
               build, and even commit, but those commits belong to no branch: switching away \
               leaves them reachable only from the reflog.",
        git: "git switch --no-guess --detach <commit>",
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
            (
                "Why are rewriting and pulling disabled?",
                "They act on a branch, and there is none checked out: switch to one first.",
            ),
        ],
    },
    Topic {
        id: "head",
        group: Group::Branches,
        title: "HEAD",
        on: &[],
        what: "HEAD is where you are: normally the name of the current branch (and so its \
               commit), or a commit itself when detached. Committing moves the branch HEAD \
               names; switching changes what HEAD names. In the graph the node ringed twice is \
               HEAD's commit.",
        git: "git symbolic-ref HEAD, git rev-parse HEAD",
        cases: &[
            (
                "“It is where HEAD is”?",
                "That action makes no sense on the commit you are at (merging it into itself, say).",
            ),
            (
                "Each worktree has its own?",
                "Yes: every worktree has its own HEAD, so each can have its own branch checked out.",
            ),
        ],
    },
    Topic {
        id: "current-branch",
        group: Group::Branches,
        title: "The current branch",
        on: &[],
        what: "The branch checked out, marked with a check in the sidebar and on its badge. \
               Commits, merges, rebases, cherry-picks, reverts, resets, and pulls all act on \
               it; everything else is moved by name. It can't be deleted or hidden from the \
               graph while it is checked out.",
        git: "git symbolic-ref HEAD",
        cases: &[
            (
                "“It is the current branch”?",
                "That action needs another branch (deleting it, merging it into itself): switch away first.",
            ),
            (
                "Checked out in another worktree?",
                "git won't check one branch out twice, nor move or delete it from here while it is checked out there.",
            ),
        ],
    },
    Topic {
        id: "into-current",
        group: Group::Branches,
        title: "Into the current branch",
        on: &["Into *"],
        what: "The items under this header change the branch checked out (named in the \
               header), never the commit or branch you clicked: git merges into, rebases, \
               cherry-picks onto, and reverts in HEAD's branch only. To change another branch, \
               switch to it first.",
        git: "git merge, git rebase, git cherry-pick, git revert",
        cases: &[
            (
                "Disabled?",
                "Another task is running, an operation (a merge, a rebase) is in progress, or the commit is where HEAD already is: its words say which.",
            ),
            (
                "Uncommitted changes?",
                "git refuses when they would be overwritten: commit or stash them first.",
            ),
        ],
    },
    Topic {
        id: "merge",
        group: Group::Branches,
        title: "Merging",
        on: &["Merge *"],
        what: "Merging brings another branch's (or a commit's) commits into the current one. \
               When the current branch has nothing the other lacks, it simply moves forward (a \
               fast-forward); otherwise git makes a merge commit with two parents, holding both \
               sides' changes.",
        git: "git merge --no-edit",
        cases: &[
            (
                "What do “✓ clean” and “⚠ 2 conflicts” beside it mean?",
                "egit works the merge out as the menu opens (git merge-tree: in memory, nothing touched): clean, how many files would conflict (named on hover), up to date, or a fast-forward. Not in a partial clone, where git would fetch to work it out.",
            ),
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
                "Merging into a branch not checked out?",
                "git merges into the current branch only: switch to it first (a drop on another branch says so).",
            ),
            (
                "Regretted?",
                "Undo moves the branch back to before the merge. Once pushed, revert the merge instead.",
            ),
            (
                "Wanted a merge commit, got a fast-forward?",
                "Use the “(no fast-forward)” item beside it: it always makes one.",
            ),
        ],
    },
    Topic {
        id: "merge-no-ff",
        group: Group::Branches,
        title: "Merging without fast-forward",
        on: &["Merge * (no fast-forward)"],
        what: "Always makes a merge commit, even when the current branch could simply move \
               forward to the other. The branch's commits stay grouped behind that merge, so \
               the history shows where it came in, and reverting the merge takes the whole \
               branch back out in one commit.",
        git: "git merge --no-edit --no-ff",
        cases: &[
            (
                "What the plain Merge does instead?",
                "What merge.ff says: unset, it fast-forwards whenever it can, leaving no trace that a branch was merged.",
            ),
            (
                "merge.ff = only in your configuration?",
                "--no-ff on the command line wins: the merge commit is still made.",
            ),
            (
                "The branches have diverged?",
                "Then both items do the same: a merge commit is the only way.",
            ),
            (
                "Already up to date?",
                "Nothing to merge: no commit is made, fast-forward or not.",
            ),
        ],
    },
    Topic {
        id: "merge-commits",
        group: Group::Branches,
        title: "Merge commits",
        on: &[],
        what: "A merge commit has two (or more) parents: the first is the branch you were on, \
               the next the one merged in. Its own changes are only what resolving conflicts \
               added; diffs in egit show it against its first parent, so you see all that \
               came in with the merge.",
        git: "git merge --no-edit; git diff-tree <merge> against its first parent",
        cases: &[
            (
                "Cherry-picking or reverting a merge?",
                "egit takes its changes against the first parent (--mainline 1): everything the merged branch brought.",
            ),
            (
                "Rebasing drops merges?",
                "A plain rebase flattens them into a line; “keeping merges” replays them.",
            ),
        ],
    },
    Topic {
        id: "rebase-or-merge",
        group: Group::Branches,
        title: "Merge or rebase?",
        on: &[],
        what: "Both bring another branch's work into yours. Merging keeps history as it \
               happened and adds a merge commit; nothing is rewritten, so it's always safe. \
               Rebasing replays your commits on top of the other branch: a straight line, but \
               your commits get new names.",
        git: "git merge; git rebase",
        cases: &[
            (
                "Others have your commits already (pushed)?",
                "Prefer merging: a rebase means a force push, and everyone who built on the old commits has to rebase too.",
            ),
            (
                "Many small conflicts?",
                "A rebase stops at each commit that conflicts; a merge asks once for the whole.",
            ),
        ],
    },
    Topic {
        id: "fast-forward",
        group: Group::Branches,
        title: "Fast-forwarding",
        on: &["Fast-forward *"],
        what: "A branch is fast-forwarded when it moves to a commit that already holds it: no \
               merge commit, nothing replayed, the history stays a line. It is refused when the \
               branch has commits the target lacks. A branch not checked out moves without \
               switching to it.",
        git: "git merge --ff-only (the current branch); git fetch . <ref>:<branch> (another)",
        cases: &[
            (
                "Refused?",
                "The two have diverged: merge or rebase instead.",
            ),
            (
                "A branch checked out in another worktree?",
                "git refuses to move it from here: fast-forward it there.",
            ),
            ("Regretted?", "Undo moves the branch back."),
        ],
    },
    Topic {
        id: "tag",
        group: Group::Branches,
        title: "Tags",
        on: &["Tag*", "Create a tag*"],
        what: "A tag names a commit and doesn't move: releases are tagged (v1.2.0). \
               Lightweight tags are just names; annotated ones also hold a message, who made \
               them, and when, and can be signed.",
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
            (
                "Moved a tag that was pushed?",
                "Others keep the old one: git never moves a tag they fetched. Make a new tag instead.",
            ),
        ],
    },
    Topic {
        id: "create-tag",
        group: Group::Branches,
        title: "Creating a tag",
        on: &["Create tag"],
        what: "Tags the commit with the name typed. With a message, the tag is annotated: a \
               tag object holding the message, you as its tagger, and the date (what releases \
               should use: git describe and many tools read only annotated tags). Without one, \
               it is lightweight: just the name.",
        git: "git tag --annotate --cleanup=whitespace --file=- <name> <commit>; git tag --no-sign <name> <commit>",
        cases: &[
            (
                "A tag of that name exists?",
                "git refuses: tags don't move. Delete the old one first, if it was never shared.",
            ),
            (
                "Signed?",
                "With “Sign every annotated tag” in your profile (tag.gpgSign), an annotated tag is signed as it is made; a lightweight one never is.",
            ),
            (
                "It isn't on the remote?",
                "Tags stay here until pushed: its menu's Push sends it.",
            ),
            ("Regretted?", "Undo deletes it again (here only)."),
        ],
    },
    Topic {
        id: "delete-tag",
        group: Group::Branches,
        title: "Deleting a tag",
        on: &["Delete the tag *"],
        what: "Removes the tag here. Its commit stays (branches still hold it), and a remote \
               that has the tag keeps its own.",
        git: "git tag --delete <tag>",
        cases: &[
            (
                "It came back?",
                "A fetch brings tags back from a remote that still has them: delete it there too (git push <remote> :refs/tags/<tag>).",
            ),
            ("Regretted?", "Undo makes it again, at the same commit."),
        ],
    },
    Topic {
        id: "drag",
        group: Group::Branches,
        title: "Dragging a branch",
        on: &["* onto *"],
        what: "Drop a branch's badge (or sidebar row) on another's, and a menu says what that \
               can do. Onto the current branch: merge the dragged one in, rebase onto it, or \
               fast-forward to it. The current branch onto another: rebase onto that, merge it \
               in, or fast-forward. Between two others: fast-forward the one dropped on, or \
               switch to it.",
        git: "git merge, git rebase, git merge --ff-only, git fetch . <ref>:<branch>",
        cases: &[
            (
                "Merging into a branch not checked out?",
                "git merges into the current branch only: switch to it first.",
            ),
            (
                "Dropped by mistake?",
                "Nothing happens until you pick: Escape or a click elsewhere closes the menu.",
            ),
        ],
    },
    Topic {
        id: "discard",
        group: Group::Everyday,
        title: "Deleting",
        on: &["Delete *"],
        what: "Deleting a branch, a tag, a stash, a remote's branch: egit says what would be lost \
               first. Local deletions can be undone; a remote's branch deleted is gone for \
               everyone until pushed again.",
        git: "git branch --delete, git tag --delete, git stash drop, git push --delete",
        cases: &[(
            "Not merged?",
            "Its commits would be reachable only from the reflog: egit asks again.",
        )],
    },
    Topic {
        id: "delete-branch",
        group: Group::Branches,
        title: "Deleting a branch",
        on: &["Delete *…"],
        what: "Removes the branch's name here. Its commits stay wherever another branch or tag \
               holds them. git refuses when the branch has commits not merged (into its \
               upstream, or the current branch): egit then says so, and asks again.",
        git: "git branch --delete <branch> (--force once you confirm)",
        cases: &[
            (
                "The current branch?",
                "It can't be deleted while checked out: switch away first.",
            ),
            (
                "Its remote's copy?",
                "Stays: “Delete … and origin/…” deletes both.",
            ),
            (
                "Deleted by mistake?",
                "Undo makes it again at the same commit; later, the Reflog of HEAD still has where it was.",
            ),
        ],
    },
    Topic {
        id: "delete-unmerged",
        group: Group::Branches,
        title: "Delete anyway",
        on: &["Delete anyway"],
        what: "git refused to delete the branch because it has commits no other branch has \
               (they aren't merged into its upstream or the current branch). Deleting anyway \
               drops the name; those commits are then reachable only from the reflog, until \
               git cleans them up (after 30 days, by default).",
        git: "git branch --delete --force <branch>",
        cases: &[
            (
                "Merged on the remote (squashed or rebased in a pull request)?",
                "git can't see that: the commits on main are new ones. Deleting anyway is then safe.",
            ),
            (
                "Regretted?",
                "Undo makes it again. Later, find its last commit in the Reflog and create a branch there.",
            ),
        ],
    },
    Topic {
        id: "delete-everywhere",
        group: Group::Branches,
        title: "Deleting a branch here and on its remote",
        on: &["Delete * and *…"],
        what: "Deletes the local branch, then its branch on the remote, in that order. If git \
               refuses here because the branch isn't merged, nothing is deleted anywhere, and \
               egit asks again.",
        git: "git branch --delete <branch>, then git push <remote> :refs/heads/<branch>",
        cases: &[
            (
                "The remote refused?",
                "The local branch is gone already; the remote's stays (protected, or not yours to delete).",
            ),
            (
                "Undo?",
                "Takes back the local deletion only: pushes aren't undone.",
            ),
        ],
    },
    Topic {
        id: "recover-branch",
        group: Group::Branches,
        title: "Getting a deleted branch back",
        on: &[],
        what: "Deleting a branch removes a name, not commits. Right after, Undo makes it again. \
               Later, the commits are still in the repository until git's clean-up removes \
               them (about 30 days after nothing reaches them): find the branch's last commit \
               in HEAD's Reflog (if you had it checked out) and create a branch there.",
        git: "git reflog; git branch <name> <commit>",
        cases: &[
            (
                "Never checked it out here?",
                "HEAD's reflog doesn't have it: look for its commits on a remote, or in another clone.",
            ),
            (
                "Deleted on the remote too?",
                "Recreate it here, then push it again.",
            ),
        ],
    },
    Topic {
        id: "graph-scope",
        group: Group::Looking,
        title: "In the graph",
        on: &["In the graph"],
        what: "These change what the graph shows, never the repository: pin a branch's line to \
               the left (or unpin it), hide a branch, show only it, or show it too while only \
               some are shown. Each reads the history again from the branches asked for. A \
               line under the top bar says what applies, with a way back.",
        git: "git rev-list --topo-order, started from the branches shown (--exclude for those hidden)",
        cases: &[
            (
                "A commit I expected is missing?",
                "Only commits reachable from the branches shown are listed: show every branch again.",
            ),
            (
                "How long does it last?",
                "While the tab is open; nothing is written to the repository.",
            ),
        ],
    },
    Topic {
        id: "badges",
        group: Group::Branches,
        title: "Badges in the graph",
        on: &[],
        what: "Beside a commit's message, a badge for each name at it: local branches (the \
               current one with a check), remote-tracking branches (origin/main), tags, a \
               bisection's marks, and pull or merge requests (#12, !12). “+2” counts badges \
               that don't fit the column. Click one to select its commit, double click a \
               branch to switch to it, right click for its menu, drag it onto another to merge \
               or rebase.",
        git: "git for-each-ref (the names), read again whenever refs change",
        cases: &[
            (
                "A branch's badge is missing?",
                "It may be hidden from the graph (its eye in the sidebar), or the history shows only some branches.",
            ),
            (
                "main and origin/main on different commits?",
                "Your branch and the remote's differ: you have commits to push, or the remote has some to pull.",
            ),
        ],
    },
    Topic {
        id: "branch-folders",
        group: Group::Branches,
        title: "Branch folders",
        on: &[],
        what: "Branch names with “/” fold into folders in the sidebar: feature/login and \
               feature/search sit in feature/. A folder is only how the names read: git has no \
               folders of branches, and moving one between folders is renaming it.",
        git: "",
        cases: &[(
            "feature/ won't take a branch named feature?",
            "git stores branches as files: one name can't be both a branch and a folder. Rename one of them.",
        )],
    },
    Topic {
        id: "default-branch",
        group: Group::Branches,
        title: "The default branch",
        on: &[],
        what: "The branch a clone checks out first and pull requests aim at by default: main, \
               master, or whatever the remote's HEAD names. Locally it is an ordinary branch; \
               only the remote and its forge treat it specially (often protected from force \
               pushes and deletion).",
        git: "git clone (checks out the remote's HEAD); origin/HEAD",
        cases: &[(
            "The remote renamed master to main?",
            "Fetch, switch to main (it tracks origin/main), then delete your old master once nothing of yours is only there.",
        )],
    },
    Topic {
        id: "branch-reflog",
        group: Group::Looking,
        title: "A branch's reflog",
        on: &["Show *'s reflog"],
        what: "Every place this branch has pointed, newest first: each commit, merge, rebase, \
               reset, pull, or rename that moved it, with when. It is kept here only (never \
               pushed), and moves older than 90 days are forgotten (30 for commits nothing \
               else reaches).",
        git: "git log --walk-reflogs refs/heads/<branch>",
        cases: &[
            (
                "Lost commits after a rebase or reset?",
                "The move before it points at them: create a branch there, or reset to it.",
            ),
            (
                "A remote branch's reflog?",
                "Records each fetch that moved it here, not what happened on the remote.",
            ),
        ],
    },
    Topic {
        id: "copy-branch-name",
        group: Group::Branches,
        title: "Copying a branch's name",
        on: &["Copy branch name"],
        what: "Puts the branch's short name on the clipboard (main, or origin/main for a \
               remote's), as git commands take it.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "copy-commit-name",
        group: Group::Looking,
        title: "Copying a commit's name",
        on: &["Copy commit name"],
        what: "Puts the commit's full name (its 40-digit SHA-1, or 64 for SHA-256 \
               repositories) on the clipboard: the exact name git takes anywhere, unlike a \
               short one, which may need more digits as a repository grows.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "copy-subject",
        group: Group::Looking,
        title: "Copying a commit's subject",
        on: &["Copy subject"],
        what: "Puts the commit message's first line (its subject) on the clipboard.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "worktree-create",
        group: Group::Inside,
        title: "Creating a worktree",
        on: &[
            "Create worktree",
            "Create a worktree",
            "Create a worktree from *…",
            "Create a worktree here…",
        ],
        what: "Makes a second work tree in the folder given, sharing this repository's history \
               and branches: there you can have another branch checked out at the same time, \
               without stashing or cloning again. A remote's branch gets a local branch \
               tracking it; a commit gets a new branch, or none (detached).",
        git: "git worktree add --quiet [-b <name> --track|--no-track | --detach] -- <folder> <start>",
        cases: &[
            (
                "“Already checked out”?",
                "git checks a branch out in one worktree at a time: pick another, or make a new branch.",
            ),
            ("The folder exists?", "It must be new or empty."),
            (
                "Submodules and LFS files there?",
                "Each worktree checks out its own: set its submodules up there, and LFS files download as it checks out.",
            ),
            (
                "Deleted its folder by hand?",
                "git still lists it until pruned (the sidebar's worktrees).",
            ),
        ],
    },
    Topic {
        id: "worktree-open",
        group: Group::Inside,
        title: "Opening a new worktree",
        on: &["Open it once made"],
        what: "Ticked, the new worktree opens in a tab of its own as soon as git has made it. \
               Unticked, it is made and listed in the sidebar's worktrees, from where you can \
               open it later.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "reset",
        group: Group::Rewriting,
        title: "Resetting",
        on: &["Reset *"],
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
                "Already pushed?",
                "The remote still has the commits: pushing then needs a force push. Revert instead on shared branches.",
            ),
        ],
    },
    Topic {
        id: "reset-soft",
        group: Group::Rewriting,
        title: "Soft reset",
        on: &["Soft: *"],
        what: "Moves the branch only. The index and the work tree stay as they are, so the \
               changes of the commits it moved past are now staged, ready to commit again: \
               the way to squash the last few commits by hand.",
        git: "git reset --quiet --soft <commit>",
        cases: &[
            (
                "Reset to a later commit (from the reflog)?",
                "The difference is staged the other way: committing would undo it. Mixed or keep are usually what you want there.",
            ),
            ("Regretted?", "Undo moves the branch back."),
        ],
    },
    Topic {
        id: "reset-mixed",
        group: Group::Rewriting,
        title: "Mixed reset",
        on: &["Mixed: *"],
        what: "Moves the branch and makes the index the commit's, leaving the work tree alone: \
               the changes of the commits moved past, and anything you had staged, become \
               changes not staged. Nothing in your files is lost. git's default reset.",
        git: "git reset --quiet --mixed <commit>",
        cases: &[
            (
                "Files those commits added?",
                "They stay in the work tree, now untracked.",
            ),
            ("Regretted?", "Undo moves the branch back."),
        ],
    },
    Topic {
        id: "reset-keep",
        group: Group::Rewriting,
        title: "Keep reset",
        on: &["Keep: *"],
        what: "Moves the branch and makes the index and the work tree the commit's, but keeps \
               your uncommitted changes. The changes of the commits moved past are gone from \
               the files (still in the reflog). If a file you changed also differs between \
               the two commits, git refuses and nothing moves.",
        git: "git reset --quiet --keep <commit>",
        cases: &[
            (
                "Refused?",
                "Your uncommitted changes would be overwritten: commit or stash them, then reset.",
            ),
            ("Regretted?", "Undo moves the branch back."),
        ],
    },
    Topic {
        id: "reset-hard",
        group: Group::Rewriting,
        title: "Hard reset",
        on: &["Hard: *", "Reset"],
        what: "Moves the branch and makes the index and the work tree exactly the commit's: \
               every uncommitted change to tracked files is lost, staged or not. Untracked \
               files are left alone. egit asks first.",
        git: "git reset --quiet --hard <commit>",
        cases: &[
            (
                "Can Undo bring my changes back?",
                "It moves the branch back, but uncommitted changes were never stored by git: they are gone. Stash first if unsure.",
            ),
            (
                "Something staged before the reset?",
                "git stored its content when you staged it; it may still be found with git fsck --lost-found, by hand.",
            ),
            (
                "Submodules?",
                "They keep their commits unless submodule.recurse is on: update them after.",
            ),
        ],
    },
    Topic {
        id: "undo-a-commit",
        group: Group::Rewriting,
        title: "Taking a commit back",
        on: &[],
        what: "Four ways, by where the commit is. Just made, not pushed: Undo (or a soft reset \
               to its parent) keeps its changes staged. Further back, not pushed: drop it \
               (rebasing what follows). Pushed: revert it, a new commit undoing it, so nobody's \
               history is rewritten.",
        git: "git reset --soft HEAD~1; git rebase (drop); git revert",
        cases: &[(
            "Only its message is wrong?",
            "Edit the message instead: double click it in the details.",
        )],
    },
    Topic {
        id: "cherry-pick",
        group: Group::Rewriting,
        title: "Cherry-picking",
        on: &["Cherry-pick*"],
        what: "Copies one commit's change onto the current branch as a new commit (new name, \
               same message and author). The original stays where it was.",
        git: "git cherry-pick (a merge against its first parent: --mainline 1)",
        cases: &[
            (
                "What does “⚠ 1 conflict” beside it mean?",
                "egit works the pick out as the menu opens (git merge-tree from the commit's parent: nothing touched): clean, the files that would conflict (on hover), or no changes when its change is in already.",
            ),
            (
                "Its change is already here?",
                "The result would be empty: egit skips it and says so.",
            ),
            ("Conflicts?", "Resolve, stage, Continue; or Abort."),
            (
                "Later merging the branch it came from?",
                "git usually sees the same change and merges cleanly, but the commit is in history twice, under two names.",
            ),
            ("Picked the wrong one?", "Undo takes it back."),
        ],
    },
    Topic {
        id: "revert",
        group: Group::Rewriting,
        title: "Reverting",
        on: &["Revert*"],
        what: "Makes a new commit that undoes a commit's change. History keeps both: nothing is \
               rewritten, so it's safe on pushed branches.",
        git: "git revert --no-edit (a merge against its first parent: --mainline 1)",
        cases: &[
            (
                "What does “⚠ 1 conflict” beside it mean?",
                "egit works the revert out as the menu opens (git merge-tree: nothing touched). A later commit changed the same lines, so undoing this one would conflict there. The latest commit always reverts cleanly.",
            ),
            (
                "Reverted a merge, then want the branch again?",
                "Merging it again brings nothing back: git thinks it's merged. Revert the revert instead.",
            ),
            (
                "Conflicts?",
                "Later commits changed the same lines: resolve, stage, Continue.",
            ),
            (
                "Regretted before pushing?",
                "Undo takes the revert commit back.",
            ),
        ],
    },
];
