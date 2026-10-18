//! Rewriting history, and what stops it: rebases, amending, rewording, conflicts and the merge editor, bisecting, stashes, and patches.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "amend",
        group: Group::Rewriting,
        title: "Amending the last commit",
        on: &["Amend", "Amend the last commit*"],
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
        on: &["Edit the message*", "Save message*"],
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
                "Uncommitted changes?",
                "HEAD's message changes anyway. An earlier commit's needs a rebase, which git refuses while files are changed: commit or stash them first.",
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
        id: "rewrite-menu",
        group: Group::Rewriting,
        title: "Rewriting a branch from a commit",
        on: &["Rewrite *"],
        what: "The commit menu's group for changing the current branch's history from that \
               commit on: rebasing interactively, editing a message, squashing the commit into \
               its parent, or dropping it. Each makes the commits from there on again, with new \
               names; the old ones stay in the reflog.",
        git: "git rebase --interactive (egit writes the todo)",
        cases: &[
            (
                "Greyed out?",
                "It needs a branch checked out (not a detached HEAD), no task running, and no operation in progress.",
            ),
            (
                "The commit isn't on the current branch?",
                "Only commits HEAD reaches can be rewritten from here: switch to the branch holding it first.",
            ),
        ],
    },
    Topic {
        id: "rebase",
        group: Group::Rewriting,
        title: "Rebasing",
        on: &["Rebase *"],
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
                "During a rebase, which side is “current”?",
                "The branch being rebased onto: “incoming” is your commit being replayed. It's the other way round from a merge.",
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
        id: "rebase-keeping-merges",
        group: Group::Rewriting,
        title: "Rebasing, keeping merges",
        on: &["Rebase * onto *, keeping merges"],
        what: "A plain rebase flattens: merge commits on your branch disappear and only the \
               commits they brought in are replayed, in a line. Keeping merges makes each merge \
               again on top of the target, with the same shape of branches meeting, so a branch \
               that merged others still shows where they joined.",
        git: "git rebase --rebase-merges",
        cases: &[
            (
                "A merge conflicted the first time?",
                "Making it again meets the same conflict: git doesn't remember how you resolved it (unless rerere is on), so resolve it again and Continue.",
            ),
            (
                "Want a straight line instead?",
                "Use the plain Rebase: the merges go and their commits are replayed one by one.",
            ),
            (
                "Undo it?",
                "Undo moves the branch back; Abort does too while the rebase is still stopped.",
            ),
        ],
    },
    Topic {
        id: "interactive",
        group: Group::Rewriting,
        title: "Rebasing interactively",
        on: &["Rebase interactively*"],
        what: "Rewrites the current branch from a commit on: reorder commits, drop them, squash \
               one into the one before (messages joined) or fix it up (its message dropped), \
               reword messages. Every commit from the first one changed onward gets a new name.",
        git: "git rebase --interactive --no-autosquash --no-update-refs, its todo written by egit (a sequence editor copies it in)",
        cases: &[
            (
                "Uncommitted changes?",
                "git refuses to start a rebase while files are changed: commit or stash them first.",
            ),
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
            (
                "A fixup! commit isn't moved on its own?",
                "egit runs git without --autosquash: what the editor shows is exactly what happens.",
            ),
        ],
    },
    Topic {
        id: "rebase-editor",
        group: Group::Rewriting,
        title: "The rebase editor",
        on: &["Back to the history"],
        what: "The commits from the one chosen up to HEAD, newest at the top, each with a step: \
               Pick, Reword, Squash, Fix up, or Drop. Nothing happens until Start rebase: until \
               then the plan is only egit's, and going back to the history forgets it. Keys \
               while no field is focused: ↑ ↓ to choose, Alt+↑ Alt+↓ to move, P R S F D for \
               the steps.",
        git: "",
        cases: &[
            (
                "The range couldn't be read?",
                "HEAD doesn't reach that commit (it is on another branch), or there are more than 10,000 commits from it: pick a commit nearer HEAD.",
            ),
            (
                "Rows won't move?",
                "The range holds merges: git decides their order, so only Pick, Reword and Drop of ordinary commits are offered.",
            ),
            (
                "Newest at the top, but git's todo is oldest first?",
                "egit turns the plan round when it hands it to git: a squash melds into the row below it.",
            ),
        ],
    },
    Topic {
        id: "rebase-start",
        group: Group::Rewriting,
        title: "Starting the rebase",
        on: &["Start rebase"],
        what: "Hands the plan to git as its todo, oldest first, and git replays the commits from \
               the first one changed: each kept commit is made again on the one before, reworded \
               ones get their new messages, squashed and fixed-up ones melt into the commit \
               below them, dropped ones are left out. The branch then points at the last new \
               commit.",
        git: "git rebase --interactive <base> (GIT_SEQUENCE_EDITOR copies egit's todo in)",
        cases: &[
            (
                "Disabled?",
                "Its hover says why: nothing changes yet, a reworded commit has an empty message, the lowest commit kept would meld into nothing, or every commit is dropped (reset the branch instead).",
            ),
            (
                "Stopped part-way?",
                "A commit didn't apply cleanly: resolve and Continue, Skip it, or Abort to have the branch back as it was. Rewords still happen after a Continue.",
            ),
            (
                "Regretted afterwards?",
                "Undo moves the branch back to where it was; the reflog has the old commits too.",
            ),
        ],
    },
    Topic {
        id: "rebase-move",
        group: Group::Rewriting,
        title: "Moving a commit in the plan",
        on: &["Move up", "Move down"],
        what: "Changes the order the commits will be replayed in: up is later (newer), down is \
               earlier. Only the plan changes until the rebase starts; then each commit is \
               applied in its new place, on top of whatever comes before it now.",
        git: "",
        cases: &[
            (
                "Can moving conflict?",
                "Yes: a commit that changed lines a later one changes too may no longer apply on its own. Resolve and Continue, or Abort.",
            ),
            (
                "Disabled?",
                "The first row can't move up, the last can't move down, and in a range holding merges nothing moves.",
            ),
            (
                "From the keyboard?",
                "Alt+↑ and Alt+↓ move the selected row.",
            ),
        ],
    },
    Topic {
        id: "rebase-pick",
        group: Group::Rewriting,
        title: "Pick",
        on: &["Pick"],
        what: "Keeps the commit as it is: its changes and message are replayed unchanged. If \
               nothing before it changed, git reuses it as it was; otherwise it is made again \
               with a new name, the same message, author, and date.",
        git: "pick <commit> in git's todo",
        cases: &[(
            "Its name changed though nothing did?",
            "A commit's name covers its parent: once one before it changed, every later one is new.",
        )],
    },
    Topic {
        id: "rebase-reword",
        group: Group::Rewriting,
        title: "Reword",
        on: &["Reword"],
        what: "Keeps the commit's changes and gives it the message typed below the list. Its \
               author and date stay; it, and every commit after it, get new names.",
        git: "pick, then exec git commit --amend --file=- with the new message",
        cases: &[
            ("An empty message?", "Refused: a commit needs a message."),
            (
                "After a conflict?",
                "The new message is in the todo itself, so a rebase continued after a conflict still rewords.",
            ),
        ],
    },
    Topic {
        id: "rebase-squash",
        group: Group::Rewriting,
        title: "Squash",
        on: &["Squash"],
        what: "Melds the commit into the one below it in the list (the older one): one commit \
               holds both changes, and their messages are joined. The older commit's author \
               and date stay.",
        git: "squash <commit> in git's todo",
        cases: &[
            (
                "Nothing below to meld into?",
                "The lowest commit kept can't be squashed: refused.",
            ),
            (
                "Want only one of the messages?",
                "Use Fix up, which keeps the older commit's message alone.",
            ),
            (
                "Merges in the range?",
                "Squashing isn't offered: git's own todo keeps the merges' shape.",
            ),
        ],
    },
    Topic {
        id: "rebase-fixup",
        group: Group::Rewriting,
        title: "Fix up",
        on: &["Fix up"],
        what: "Melds the commit into the one below it (the older one), as Squash does, but \
               throws its message away: the older commit's message stays as it was. Made for \
               “oops, forgot a file” commits.",
        git: "fixup <commit> in git's todo",
        cases: &[(
            "Both messages wanted?",
            "Use Squash; or Reword the commit it melds into to write a new one.",
        )],
    },
    Topic {
        id: "drop",
        group: Group::Rewriting,
        title: "Drop",
        on: &["Drop"],
        what: "In the rebase editor, leaves the commit out: its changes go from the branch and \
               the commits after it are replayed without it. In a dialog, confirms dropping a \
               commit (the same, from its menu) or a stash (deleted from the stash list).",
        git: "drop in git's todo; git stash drop stash@{n}",
        cases: &[
            (
                "Later commits needed its changes?",
                "They may conflict as they're replayed: resolve and Continue, or Abort.",
            ),
            (
                "Dropped the wrong thing?",
                "Undo brings it back: the branch moves back, or the stash is stored again.",
            ),
            (
                "Every commit dropped?",
                "Refused: resetting the branch does that more plainly.",
            ),
        ],
    },
    Topic {
        id: "squash-into-parent",
        group: Group::Rewriting,
        title: "Squashing into its parent",
        on: &["Squash into its parent"],
        what: "Melds the commit into the one before it: one commit holds both changes, their \
               messages joined, and every commit after it is made again on top with new names. \
               It is a one-step interactive rebase from the parent's parent.",
        git: "git rebase --interactive, the commit marked squash",
        cases: &[
            (
                "It's the first commit?",
                "There is nothing before it: refused.",
            ),
            (
                "The parent is a merge, or merges follow?",
                "Refused: melding commits next to merges would change the merges' shape.",
            ),
            (
                "Uncommitted changes?",
                "git refuses to rebase while files are changed: commit or stash them first.",
            ),
            ("Pushed already?", "Pushing needs a force push afterwards."),
        ],
    },
    Topic {
        id: "drop-commit",
        group: Group::Rewriting,
        title: "Dropping a commit",
        on: &["Drop this commit…"],
        what: "Removes the commit from the current branch: its changes go, and every commit \
               after it is replayed without it, getting new names. Unlike Revert, no new commit \
               says so: history reads as if it never happened. A dialog asks first.",
        git: "git rebase --interactive, the commit marked drop",
        cases: &[
            (
                "Others already have it (pushed)?",
                "Revert it instead: dropping rewrites history they share, and pushing would need force.",
            ),
            (
                "Later commits depend on it?",
                "They may conflict when replayed: resolve and Continue, or Abort.",
            ),
            (
                "Want it back?",
                "Undo; or find it in the reflog, where the old commits stay for a while.",
            ),
        ],
    },
    Topic {
        id: "continue",
        group: Group::Conflicts,
        title: "Continuing an operation",
        on: &["Continue"],
        what: "Tells the stopped merge, rebase, cherry-pick, revert, or patch series to carry on \
               once its conflicts are resolved and staged. A merge then makes its merge commit; \
               the others make the commit they stopped at and go on to the next, possibly \
               stopping again.",
        git: "git merge|rebase|cherry-pick|revert|am --continue",
        cases: &[
            (
                "It refused?",
                "A file is still marked Conflict: resolve it and stage it (Stage file, or Save resolution in the merge editor).",
            ),
            (
                "Its message?",
                "The one git prepared (the merge's, the commit's own) is kept as it is.",
            ),
            (
                "Nothing left to commit after resolving?",
                "The commit became empty (your resolution matches what was there): Skip it instead.",
            ),
            (
                "Stopped again?",
                "Another commit conflicted: resolve that one the same way.",
            ),
        ],
    },
    Topic {
        id: "skip",
        group: Group::Conflicts,
        title: "Skipping",
        on: &["Skip"],
        what: "In a stopped rebase, cherry-pick, revert, or patch series: leaves out the commit \
               (or patch) it stopped at, throws away the conflict's changes to it, and carries \
               on with the next. In a bisection: says the commit checked out can't be tested, \
               and git picks another near it.",
        git: "git rebase|cherry-pick|revert|am --skip; git bisect skip",
        cases: &[
            (
                "Lost something?",
                "Only that one commit's changes: they are still in the original commit, which the reflog keeps.",
            ),
            (
                "No Skip for a merge?",
                "A merge is one step: there is nothing to skip to. Resolve it, or Abort.",
            ),
            (
                "Skipped in a bisection, and now several commits are left?",
                "git can't tell which of the untested ones is first: it lists them all.",
            ),
        ],
    },
    Topic {
        id: "abort",
        group: Group::Conflicts,
        title: "Aborting an operation",
        on: &["Abort…", "Abort", "Keep going"],
        what: "Gives up the merge, rebase, cherry-pick, revert, or patch series in progress: \
               the branch goes back to the commit it was at before, and the work tree and index \
               to how they were then. Conflicts resolved so far are lost. A dialog asks first; \
               Keep going closes it and changes nothing.",
        git: "git merge|rebase|cherry-pick|revert|am --abort (git bisect reset for a bisection)",
        cases: &[
            (
                "Uncommitted changes from before it started?",
                "A merge's abort tries to keep them, but can't always untangle them from the merge's: stash before merging to be safe.",
            ),
            (
                "A rebase was half done?",
                "Every commit already replayed is dropped too: the branch is exactly where it was.",
            ),
            (
                "Aborted by mistake?",
                "Start the operation again: the commits it would have made are still where they came from.",
            ),
        ],
    },
    Topic {
        id: "show-conflicts",
        group: Group::Conflicts,
        title: "Showing the conflicts",
        on: &["Show the conflicts"],
        what: "Shows the work tree beside the history: the files in conflict are listed (marked \
               U), each opening in the merge editor. Nothing changes until you resolve one.",
        git: "git status --porcelain=v2 (files in conflict are unmerged)",
        cases: &[(
            "None listed?",
            "They are all resolved and staged: Continue.",
        )],
    },
    Topic {
        id: "conflict-stage-file",
        group: Group::Conflicts,
        title: "Marking a conflict resolved",
        on: &[],
        what: "Stages the file in conflict as the work tree has it now, which marks it resolved: \
               whatever the file holds is taken, conflict markers included if you left any. Use \
               it after resolving the file in another editor.",
        git: "git add",
        cases: &[
            (
                "Markers still in it?",
                "git doesn't check: they would be committed. Look for <<<<<<< before staging.",
            ),
            (
                "Changed your mind?",
                "Once staged it isn't a conflict here any more: git checkout --merge <file> in a terminal brings the conflict back, or Abort starts over.",
            ),
        ],
    },
    Topic {
        id: "conflicts",
        group: Group::Conflicts,
        title: "Conflicts",
        on: &[],
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
            (
                "Line endings or whitespace only?",
                "git still counts it as a conflict when both sides changed the lines: pick the side whose endings the repository keeps.",
            ),
        ],
    },
    Topic {
        id: "merge-editor",
        group: Group::Conflicts,
        title: "The merge editor",
        on: &[],
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
            (
                "Previous and Next?",
                "Step between this file's conflicts (Alt+↑, Alt+↓); the lines between them merged cleanly and need nothing.",
            ),
        ],
    },
    Topic {
        id: "merge-save",
        group: Group::Conflicts,
        title: "Saving a resolution",
        on: &["Save resolution"],
        what: "Builds the file from your choices, writes it to the index as resolved, and \
               writes the work tree's copy through the repository's filters (line endings, \
               LFS). The file then leaves the list of conflicts; once none is left, Continue \
               finishes the operation.",
        git: "git hash-object -w --no-filters; git update-index --index-info; git checkout-index --force",
        cases: &[
            (
                "Disabled?",
                "A conflict is still open: every one needs a choice first (the count says how many are left).",
            ),
            (
                "The file changed meanwhile?",
                "Refused if the conflict's sides changed since it was read: it reads them again.",
            ),
            (
                "Edits in the work tree's copy?",
                "Saving overwrites them with what the editor built.",
            ),
        ],
    },
    Topic {
        id: "take-current",
        group: Group::Conflicts,
        title: "Taking the current side",
        on: &["Current", "Take current"],
        what: "Resolves the selected conflict with the current side's lines, dropping the \
               incoming side's there. Current is HEAD: your branch in a merge, cherry-pick, or \
               revert, but the branch being rebased onto in a rebase.",
        git: "",
        cases: &[
            (
                "In a rebase, current isn't my work?",
                "Right: during a rebase your own commit is the incoming side.",
            ),
            (
                "Other conflicts in the file?",
                "Only the selected one is resolved; the next open one is selected.",
            ),
        ],
    },
    Topic {
        id: "take-incoming",
        group: Group::Conflicts,
        title: "Taking the incoming side",
        on: &["Incoming", "Take incoming"],
        what: "Resolves the selected conflict with the incoming side's lines: the branch being \
               merged in, the commit being cherry-picked or reverted, the patch being applied, \
               or, in a rebase, your own commit being replayed.",
        git: "",
        cases: &[(
            "Both sides added something useful?",
            "Take Both, then Edit… to tidy what they make together.",
        )],
    },
    Topic {
        id: "take-both",
        group: Group::Conflicts,
        title: "Taking both sides",
        on: &["Both", "Both, incoming first"],
        what: "Keeps the lines of both sides, one after the other: the current side's first, or \
               (from More) the incoming side's first. Right when each side added something \
               separate, like two new entries to a list; wrong when they changed the same \
               thing two ways.",
        git: "",
        cases: &[(
            "Duplicated lines?",
            "Both sides may have kept a line the other changed: Edit… to remove what repeats.",
        )],
    },
    Topic {
        id: "take-base",
        group: Group::Conflicts,
        title: "Taking the base",
        on: &["Base"],
        what: "Resolves the conflict with the lines as they were before either side changed \
               them: both changes there are dropped. The base is the commits' common ancestor \
               (in a cherry-pick or revert, the picked commit's parent).",
        git: "",
        cases: &[(
            "No base lines?",
            "Both sides added these lines: the base had none, so taking it leaves nothing there.",
        )],
    },
    Topic {
        id: "take-all",
        group: Group::Conflicts,
        title: "Taking a side everywhere",
        on: &["All current", "All incoming", "Every conflict"],
        what: "Resolves every conflict in this file with one side, as git's merge-file --ours \
               or --theirs would. Lines that merged cleanly keep both sides' changes: only the \
               conflicting places take the side chosen.",
        git: "",
        cases: &[
            (
                "Want the whole file as one side had it, clean merges too?",
                "That isn't this: after saving, check the result, or restore that side's file by hand.",
            ),
            (
                "Some conflicts already chosen?",
                "They are all set to the side picked.",
            ),
        ],
    },
    Topic {
        id: "merge-edit",
        group: Group::Conflicts,
        title: "Editing a conflict's text",
        on: &["Edit…"],
        what: "Opens the selected conflict's text in a field: what it resolves to now, or both \
               sides while it is open. Apply makes what you typed its resolution; Cancel or \
               Escape leaves it as it was. A line ending is added unless the conflict ends the \
               file.",
        git: "",
        cases: &[
            (
                "Disabled?",
                "Its lines aren't UTF-8 text, so they can't be edited here: take a side.",
            ),
            (
                "Want to edit the whole file?",
                "Open it in your editor, resolve it there, and stage it (Stage file).",
            ),
        ],
    },
    Topic {
        id: "merge-reopen",
        group: Group::Conflicts,
        title: "Reopening a conflict",
        on: &["Reopen this conflict"],
        what: "Undoes the choice made for the selected conflict: it shows both sides again, \
               open, until chosen anew. Nothing is written until Save resolution.",
        git: "",
        cases: &[(
            "Saved already?",
            "The file is staged as resolved: edit it in the work tree and stage it again, or git checkout --merge <file> in a terminal brings its conflicts back.",
        )],
    },
    Topic {
        id: "show-base",
        group: Group::Conflicts,
        title: "Showing the base",
        on: &["Show the base"],
        what: "Adds the base's lines to each open conflict, between the two sides: what the \
               file said before either side changed it. Seeing what each side changed from \
               often makes the right answer plain.",
        git: "git merge-file --zdiff3",
        cases: &[(
            "The base is empty?",
            "Both sides added these lines from nothing (or the file is new on both): there was nothing before.",
        )],
    },
    Topic {
        id: "both-sides-above",
        group: Group::Conflicts,
        title: "Both sides above",
        on: &["Both sides above"],
        what: "Shows each side's whole file side by side above the merged one, each scrolled to \
               the selected conflict, so you can read the code around it as each side has it. \
               Lines selected in either select that side's conflict. A preference: it stays as \
               set.",
        git: "",
        cases: &[(
            "Too little room?",
            "Turn it off: the merged file then takes the whole height.",
        )],
    },
    Topic {
        id: "keep-a-side",
        group: Group::Conflicts,
        title: "Conflicts that aren't text",
        on: &[
            "Keep the current side's",
            "Keep the incoming side's",
            "Delete the file",
        ],
        what: "When one side deleted the file, or it is binary, a symbolic link, or a \
               submodule, there are no lines to merge: the file is kept whole as one side has \
               it, or deleted. The choice is staged at once.",
        git: "git update-index --index-info (a side kept); git rm --force (deleted)",
        cases: &[
            (
                "One side deleted it, the other changed it?",
                "Keeping it keeps the changed version; deleting drops those changes. Check whether the change moved elsewhere (a rename).",
            ),
            (
                "A submodule both sides moved?",
                "Keeping a side records the commit that side records; the submodule's own checkout isn't moved.",
            ),
            (
                "A binary file?",
                "git can't merge it: keep one side, and redo the other side's change by hand if both mattered.",
            ),
        ],
    },
    Topic {
        id: "bisect",
        group: Group::Looking,
        title: "Bisecting",
        on: &["Bisect"],
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
            (
                "Uncommitted changes?",
                "Each step checks out a commit: changes in the way stop it. Stash them before starting.",
            ),
            (
                "Detached HEAD while it runs?",
                "Expected: git checks out each commit to test, without a branch. Stop bisecting puts your branch back.",
            ),
        ],
    },
    Topic {
        id: "bisect-bad",
        group: Group::Looking,
        title: "Marking a commit bad",
        on: &["Mark as bad", "Bad"],
        what: "Says this commit has the bug (or the change you're looking for). Starts a \
               bisection when none is under way. With a good commit known too, git checks out \
               one halfway between to test next.",
        git: "git bisect start; git bisect bad <commit>",
        cases: &[
            (
                "Marked it wrongly?",
                "The answer will be wrong too: Stop bisecting and start again.",
            ),
            (
                "Looking for when something was fixed, not broken?",
                "Mark the fixed commits bad and the older ones good; the first “bad” commit is the fix.",
            ),
        ],
    },
    Topic {
        id: "bisect-good",
        group: Group::Looking,
        title: "Marking a commit good",
        on: &["Mark as good", "Good"],
        what: "Says this commit doesn't have the bug. Every commit it reaches is taken as good \
               too. With a bad commit known, git checks out one halfway between to test next.",
        git: "git bisect good <commit>",
        cases: &[
            (
                "Not sure?",
                "Skip it rather than guess: a wrong mark sends the search the wrong way.",
            ),
            (
                "The good commit isn't an ancestor of the bad one?",
                "git then tests the merge base first: it must know whether the bug is older than where they split.",
            ),
        ],
    },
    Topic {
        id: "bisect-mark",
        group: Group::Looking,
        title: "Bisecting in other words",
        on: &["Mark as *"],
        what: "A bisection started with terms of its own (old and new, say, instead of good and \
               bad) is marked in those words: the older state, then the newer. git finds the \
               first commit in the newer state.",
        git: "git bisect <term> <commit> (the terms from git bisect terms)",
        cases: &[(
            "Where did the terms come from?",
            "The bisection was started outside egit with --term-old and --term-new; egit uses them as given.",
        )],
    },
    Topic {
        id: "bisect-skip",
        group: Group::Looking,
        title: "Skipping a commit while bisecting",
        on: &["Skip it (it can't be tested)"],
        what: "Says this commit can't be tested (it doesn't build, say), so it is neither good \
               nor bad. git checks out another one near it. Only for a bisection under way.",
        git: "git bisect skip <commit>",
        cases: &[(
            "The answer is a list?",
            "If the first bad commit could be one of those skipped, git can't tell which: it names them all.",
        )],
    },
    Topic {
        id: "bisect-stop",
        group: Group::Looking,
        title: "Stopping a bisection",
        on: &["Stop bisecting"],
        what: "Ends the bisection and puts HEAD back where it was when it started: your branch \
               checked out again. The marks are forgotten.",
        git: "git bisect reset",
        cases: &[
            (
                "Want to keep looking at the commit found?",
                "Show the commit first, or note its name: it stays in the history either way.",
            ),
            (
                "Changes made while testing?",
                "They come along if they don't clash with the branch's files; otherwise git refuses: stash or discard them first.",
            ),
        ],
    },
    Topic {
        id: "stash",
        group: Group::Stashes,
        title: "Stashing",
        on: &["Stash", "Stashes · *", "Stashes, latest first"],
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
            ("Ignored files?", "Never stashed: they stay where they are."),
        ],
    },
    Topic {
        id: "stash-changes",
        group: Group::Stashes,
        title: "Stashing the changes",
        on: &["Stash the changes…", "Stash the changes"],
        what: "Asks for an optional message, then saves every uncommitted change in a new stash \
               (the latest, stash@{0}) and puts the work tree and index back to HEAD. The \
               branch doesn't move.",
        git: "git stash push (--include-untracked, --message as given)",
        cases: &[
            (
                "Disabled?",
                "There is nothing to stash, or a task is running.",
            ),
            (
                "Without a message?",
                "git names it after the branch and HEAD's commit: “WIP on main: 1a2b3c4 Subject”.",
            ),
            (
                "Submodules?",
                "Their changes inside aren't stashed: only a moved submodule commit is.",
            ),
        ],
    },
    Topic {
        id: "stash-untracked",
        group: Group::Stashes,
        title: "Stashing untracked files too",
        on: &["Files git doesn't track too"],
        what: "Also stashes files git doesn't track yet (new files never added), removing them \
               from the work tree until the stash is applied. Off, they stay where they are and \
               only changes to tracked files are stashed.",
        git: "git stash push --include-untracked",
        cases: &[
            (
                "Ignored files (build output)?",
                "Not included either way: they stay.",
            ),
            (
                "Applying puts a file back that exists again?",
                "git refuses rather than overwrite it: move the new file aside and apply again.",
            ),
        ],
    },
    Topic {
        id: "stash-apply",
        group: Group::Stashes,
        title: "Applying a stash",
        on: &["Apply"],
        what: "Puts the stash's changes back into the work tree, merging them with what changed \
               since, and keeps the stash in the list. Changes that were staged when stashed \
               come back unstaged, but for new files, which come back added.",
        git: "git stash apply stash@{n}",
        cases: &[
            (
                "Conflicts?",
                "Files the stash and later work both changed are marked Conflict: resolve and stage them. The stash stays.",
            ),
            (
                "On another branch?",
                "It applies there just as well: a stash isn't tied to the branch it came from.",
            ),
            (
                "Refused as stale?",
                "The list changed since it was shown (another stash pushed or dropped): it was read again; try once more.",
            ),
        ],
    },
    Topic {
        id: "stash-pop",
        group: Group::Stashes,
        title: "Popping a stash",
        on: &["Pop"],
        what: "Applies the stash, then drops it from the list when it applied cleanly: the \
               usual way to take back what you stashed.",
        git: "git stash pop stash@{n}",
        cases: &[
            (
                "Conflicts?",
                "The stash is kept, not dropped: resolve, stage, then drop it yourself.",
            ),
            (
                "Popped by mistake?",
                "The changes are in the work tree: stash them again if you meant to keep the stash (Undo doesn't take a pop back).",
            ),
        ],
    },
    Topic {
        id: "stash-drop",
        group: Group::Stashes,
        title: "Dropping a stash",
        on: &["Drop…"],
        what: "Deletes the stash from the list, after asking: its changes are gone from \
               there. Stashes after it move up a number.",
        git: "git stash drop stash@{n}",
        cases: &[
            (
                "Dropped the wrong one?",
                "Undo stores it back as the latest stash.",
            ),
            (
                "Long gone?",
                "git keeps a dropped stash's commit until it collects garbage (weeks): git fsck --unreachable can still find it.",
            ),
        ],
    },
    Topic {
        id: "stash-and-switch",
        group: Group::Stashes,
        title: "Stashing to switch",
        on: &["Stash and switch"],
        what: "Your uncommitted changes would be overwritten by the switch, so git refused it. \
               This stashes them (untracked files too), then switches. They wait in the stash \
               list: apply or pop them where you want them back.",
        git: "git stash push --include-untracked; git switch",
        cases: &[
            (
                "Want the changes on the new branch?",
                "Pop the stash after switching: they apply there, merged with that branch's files.",
            ),
            (
                "Undo?",
                "Undo switches back and pops the stash it made, as long as it is still the latest.",
            ),
        ],
    },
    Topic {
        id: "patches",
        group: Group::Stashes,
        title: "Patches",
        on: &["Patches"],
        what: "Commits or changes as text, to send or apply elsewhere. A mailbox patch keeps \
               each commit's author and message and applies as commits; a plain diff applies \
               as changes. The Patches menu applies them, and copies or saves your uncommitted \
               changes as one.",
        git: "git format-patch, git am --3way, git apply (--3way)",
        cases: &[
            (
                "Applying stopped?",
                "Resolve its conflicts and Continue, Skip that patch, or Abort.",
            ),
            (
                "Stopped without conflicts to resolve?",
                "The patch didn't apply even merging three ways (its base isn't here): Show the patch, apply it by hand and stage it, then Continue; or Skip it.",
            ),
            (
                "Binary files?",
                "Patches egit makes carry them (--binary); one made without that can't recreate them.",
            ),
        ],
    },
    Topic {
        id: "apply-patch-files",
        group: Group::Stashes,
        title: "Applying patches from files",
        on: &["Apply patches from files…"],
        what: "Picks .patch, .diff, .mbox, or .eml files and shows what they hold and whether \
               they apply here, before anything is applied. Dropping such files on the window \
               does the same.",
        git: "git mailsplit, git mailinfo, git apply --numstat --summary --check",
        cases: &[
            (
                "Disabled?",
                "A task is running, or an operation is in progress: finish or abort it first.",
            ),
            (
                "Several files?",
                "Applied in the order picked, as one series.",
            ),
        ],
    },
    Topic {
        id: "paste-patch",
        group: Group::Stashes,
        title: "Pasting a patch",
        on: &["Paste a patch to apply…", "Paste another"],
        what: "Opens a field to paste a patch into: a mailbox from git format-patch (From … \
               lines) or a plain diff (diff --git …). Once you stop typing it is read: what it \
               holds and whether it applies here.",
        git: "git apply --check (and --3way)",
        cases: &[
            (
                "Not a patch git can apply?",
                "It needs diff headers (--- a/… +++ b/… and @@ hunks): a diff copied from a web page often loses its tabs or trailing spaces.",
            ),
            (
                "From an email?",
                "Paste the whole message, headers included: its author and subject become the commit's.",
            ),
        ],
    },
    Topic {
        id: "apply-patch",
        group: Group::Stashes,
        title: "Applying a patch",
        on: &["Apply to *"],
        what: "Applies the patch to the branch checked out, as chosen above: as commits, each \
               with its own author and message, or as changes for you to commit. Where it \
               doesn't apply exactly, git merges it in three ways, which may stop at conflicts.",
        git: "git am --3way (as commits); git apply, --3way when staged (as changes)",
        cases: &[
            (
                "Disabled?",
                "Its hover says why: a task running, an operation in progress, or git can't apply it here.",
            ),
            (
                "It stopped part-way through a series?",
                "The banner says at which patch: resolve and Continue, Skip that patch, or Abort to undo the whole series.",
            ),
            (
                "Undo?",
                "Undo takes back commits applied; changes applied to the work tree are yours to discard.",
            ),
        ],
    },
    Topic {
        id: "apply-as-commits",
        group: Group::Stashes,
        title: "Applying as commits",
        on: &["As commits, keeping their authors and messages"],
        what: "Each patch in the mailbox becomes a commit on the current branch, with the \
               author, date, and message the mail carries; you are its committer. The way \
               projects that take patches by email apply them.",
        git: "git am --3way",
        cases: &[
            (
                "A patch conflicts?",
                "git am stops there: resolve and Continue, Skip it, or Abort the series.",
            ),
            (
                "Committer date differs from the author's?",
                "Expected: the author's date is when it was written, the committer's when it was applied.",
            ),
        ],
    },
    Topic {
        id: "apply-as-changes",
        group: Group::Stashes,
        title: "Applying as changes",
        on: &["As changes to commit yourself"],
        what: "Applies the patch's changes to your files without making commits: you review \
               them, then commit them your way. The mail's authors and messages are not used.",
        git: "git apply",
        cases: &[(
            "Several commits in the mailbox?",
            "Their changes land together, as one set of uncommitted changes.",
        )],
    },
    Topic {
        id: "sign-off-each",
        group: Group::Stashes,
        title: "Signing off each commit",
        on: &["Add a Signed-off-by line to each"],
        what: "Adds a “Signed-off-by: Your Name <email>” line to each commit's message as it is \
               applied: what projects using the Developer Certificate of Origin ask of whoever \
               passes a patch on.",
        git: "git am --signoff",
        cases: &[(
            "Already signed off by you?",
            "git doesn't add the same line twice in a row.",
        )],
    },
    Topic {
        id: "stage-applied",
        group: Group::Stashes,
        title: "Applying as staged changes",
        on: &["Stage them (merging in three ways where they don't apply as they are)"],
        what: "Applies the changes to both the index and the work tree, so they are staged \
               ready to commit. Where a hunk doesn't fit as it is, git merges it in three ways \
               using the blobs the patch names, which may leave conflicts to resolve. Off, the \
               changes go to the work tree alone and must fit exactly.",
        git: "git apply --3way (staged); git apply (not)",
        cases: &[(
            "Three-way merging failed?",
            "The patch names blobs this repository doesn't have: fetch the branch it was made on, or apply it by hand.",
        )],
    },
    Topic {
        id: "copy-staged-patch",
        group: Group::Stashes,
        title: "Copying the staged changes as a patch",
        on: &["Copy the staged changes as a patch"],
        what: "Puts what is staged on the clipboard as a diff (binary files included), to paste \
               into a message, an issue, or another repository's Paste a patch. Nothing in the \
               repository changes.",
        git: "git diff --cached --binary",
        cases: &[
            ("Disabled?", "Nothing is staged."),
            (
                "Refused as not text?",
                "A binary change can't go on the clipboard as text: save it as a file instead.",
            ),
        ],
    },
    Topic {
        id: "copy-every-patch",
        group: Group::Stashes,
        title: "Copying every change as a patch",
        on: &["Copy every change as a patch"],
        what: "Puts every change to tracked files since HEAD, staged or not, on the clipboard \
               as one diff. New files are in it only once staged.",
        git: "git diff HEAD --binary",
        cases: &[(
            "Disabled?",
            "No tracked file changed, or there is no commit yet to compare with.",
        )],
    },
    Topic {
        id: "save-staged-patch",
        group: Group::Stashes,
        title: "Saving the staged changes as a patch",
        on: &["Save the staged changes as a patch…"],
        what: "Writes what is staged to a .patch file you pick: a diff, binary files included, \
               that git apply (or egit's Patches menu) applies elsewhere.",
        git: "git diff --cached --binary",
        cases: &[(
            "Want it as a commit with a message?",
            "Commit it, then save the commit as a patch: that keeps author and message.",
        )],
    },
    Topic {
        id: "save-every-patch",
        group: Group::Stashes,
        title: "Saving every change as a patch",
        on: &["Save every change as a patch…"],
        what: "Writes every change to tracked files since HEAD, staged or not, to a .patch file \
               you pick. Untracked files are left out until staged.",
        git: "git diff HEAD --binary",
        cases: &[(
            "A backup before something risky?",
            "It works as one, but a stash or a branch is easier to bring back.",
        )],
    },
    Topic {
        id: "save-commit-patch",
        group: Group::Stashes,
        title: "Saving a commit as a patch",
        on: &["Save as a patch…"],
        what: "Writes the commit as a mailbox patch file (0001-its-subject.patch): its author, \
               date, message, and changes, as git format-patch makes them. git am, or \
               egit's Patches menu, applies it elsewhere as the same commit.",
        git: "git format-patch -1 --stdout --default-prefix <commit>",
        cases: &[(
            "A merge commit?",
            "format-patch shows no changes for a merge: save the commits it brought instead.",
        )],
    },
    Topic {
        id: "copy-commit-patch",
        group: Group::Stashes,
        title: "Copying a commit as a patch",
        on: &["Copy as a patch"],
        what: "Puts the commit, as a mailbox patch (author, message, and changes), on the \
               clipboard, ready to paste into an email or egit's Paste a patch elsewhere.",
        git: "git format-patch -1 --stdout --default-prefix <commit>",
        cases: &[(
            "Refused?",
            "Its changes aren't text (a binary file): save it as a file instead.",
        )],
    },
    Topic {
        id: "save-patch-series",
        group: Group::Stashes,
        title: "Saving a series of patches",
        on: &["Save this and later commits as patches…"],
        what: "Writes this commit and every later one up to HEAD into a folder you pick, one \
               numbered file each (0001-…, 0002-…), oldest first: a series git am applies in \
               order.",
        git: "git format-patch --default-prefix -o <folder> <commit>^..HEAD (--root from the first commit)",
        cases: &[
            (
                "Merges in between?",
                "They're left out: format-patch writes the ordinary commits only.",
            ),
            (
                "Too many?",
                "More than 10,000 commits is refused: pick a later commit.",
            ),
            (
                "Not in HEAD's history?",
                "Refused: the series runs from the commit up to HEAD.",
            ),
        ],
    },
    Topic {
        id: "show-patch",
        group: Group::Stashes,
        title: "Showing the patch it stopped at",
        on: &["Show the patch"],
        what: "Applying a series stopped at a patch: this shows that patch, to read what it \
               wanted to change, so you can apply it by hand where git couldn't.",
        git: "git am --show-current-patch=diff",
        cases: &[(
            "Applied it by hand?",
            "Stage the files, then Continue: git makes the commit with the patch's author and message.",
        )],
    },
    Topic {
        id: "am-in-progress",
        group: Group::Stashes,
        title: "Applying patches, stopped",
        on: &[],
        what: "A series of mailbox patches stopped at one that didn't apply: earlier ones are \
               committed already, later ones wait. The banner says which (2 of 5) and its \
               subject. Resolve it and Continue, Skip that patch, or Abort to put the branch \
               back as it was before the series.",
        git: "git am --continue, --skip, --abort",
        cases: &[
            (
                "Nothing marked Conflict?",
                "git couldn't even merge it: Show the patch, apply it by hand, stage, Continue.",
            ),
            (
                "Started from a terminal?",
                "egit sees the same state git keeps: Continue and Abort work on it here.",
            ),
        ],
    },
];
