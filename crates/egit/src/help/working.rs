//! What is uncommitted: the work tree, staging, committing, and a file's diff.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "commit",
        group: Group::Everyday,
        title: "Committing",
        on: &["Commit", "Commit *"],
        what: "A commit records what is staged (the index) as a snapshot of the whole tree, \
               with your name, the time, a message, and its parent: the commit HEAD was at. \
               The current branch then moves to it. Changes not staged stay in the work tree, \
               uncommitted. The button says how many staged files go in; it waits for a \
               summary and at least one staged file (or Amend).",
        git: "git commit --file=- --cleanup=whitespace (with --amend, --signoff, -S, --trailer as asked)",
        cases: &[
            (
                "Nothing is staged?",
                "The button stays disabled: stage what you mean to commit first (amending can still change the last message).",
            ),
            (
                "A hook refused it?",
                "No commit was made and nothing is lost: the form keeps your message, and git's whole complaint shows under it.",
            ),
            (
                "On a detached HEAD?",
                "The commit belongs to no branch: create one before you switch away, or it is only in the reflog.",
            ),
            (
                "Committed to the wrong branch?",
                "Undo takes the commit back (its changes staged again); switch, then commit there.",
            ),
            (
                "Commit and push?",
                "Commits, then pushes the branch as Push would (⇧⌘Enter): to its upstream, or asking where the first time. A commit a hook refused isn't pushed.",
            ),
        ],
    },
    Topic {
        id: "commit-summary",
        group: Group::Everyday,
        title: "A commit's summary",
        on: &[],
        what: "The message's first line: what the commit does, in a line. It is what the \
               history, git log --oneline, pull requests, and patch files show for the \
               commit. egit warns past 72 characters, as many tools cut longer ones short. \
               It can't be empty: git refuses a commit without a message.",
        git: "the first line of git commit --file=-",
        cases: &[
            (
                "A typo, after committing?",
                "Double-click the commit's subject in the details to edit it in place, or tick Amend.",
            ),
            (
                "Starts with fixup! or squash!?",
                "git's autosquash reads such words; egit's own rebase editor doesn't, so they stay plain words there.",
            ),
        ],
    },
    Topic {
        id: "commit-description",
        group: Group::Everyday,
        title: "A commit's description",
        on: &[],
        what: "Everything after the summary: why the change was made, what it leaves out, \
               links. git keeps it after a blank line under the summary. Only whitespace is \
               cleaned up: trailing spaces and blank lines at the ends go, and a line starting \
               with # is kept (git's own editor would drop it). ⌘Enter (Ctrl+Enter) commits, \
               from it, the summary, or anywhere beside them.",
        git: "git commit --file=- --cleanup=whitespace",
        cases: &[
            (
                "Long, and the field stops growing?",
                "It scrolls inside its frame; the whole text is committed.",
            ),
            (
                "Lines like Signed-off-by: or Co-authored-by:?",
                "They are trailers, read by tools: egit adds them for Sign off and co-authors, at the message's end.",
            ),
        ],
    },
    Topic {
        id: "sign-off",
        group: Group::Everyday,
        title: "Sign off",
        on: &["Sign off"],
        what: "Adds a Signed-off-by: line with your name and email at the message's end. It \
               is a statement, not a signature: projects that ask for it (the Linux kernel, \
               many under a Developer Certificate of Origin) take it to mean you may submit \
               the change. Nothing is checked cryptographically; for that, Sign.",
        git: "git commit --signoff",
        cases: &[
            (
                "Signed off twice when amending?",
                "git doesn't add a line identical to the last trailer, so amending with Sign off keeps one.",
            ),
            (
                "The wrong name in it?",
                "It is your committer identity (user.name, user.email): fix them in your profile, then amend.",
            ),
        ],
    },
    Topic {
        id: "signing",
        group: Group::Everyday,
        title: "Signed commits",
        on: &["Sign"],
        what: "A signature (OpenPGP, SSH, or X.509) proves a commit came from your key. The \
               box starts as commit.gpgSign says; ticked, egit asks git to sign with the \
               format and key your configuration names (gpg.format, user.signingKey); \
               unticked, it asks git not to, whatever the configuration says. egit checks \
               the signatures of the commits on screen.",
        git: "git commit -S (or --no-gpg-sign); git log --show-signature to check",
        cases: &[
            (
                "Asked for a passphrase?",
                "egit relays your signing program's question, labelled Signing the commit.",
            ),
            (
                "Signing failed?",
                "No commit was made: git's message says why (no key configured, an agent not running, a key expired).",
            ),
            (
                "Shown as unchecked or doubtful?",
                "The key isn't one you trust yet: gpg's trust, or gpg.ssh.allowedSignersFile for SSH keys.",
            ),
        ],
    },
    Topic {
        id: "co-authors",
        group: Group::Everyday,
        title: "Co-authors",
        on: &["Add a co-author", "Co-author", "Co-authors"],
        what: "People who wrote a commit with you, as Co-authored-by: lines at the message's \
               end (git trailers); GitHub and GitLab credit them too. The picker offers who \
               wrote the last 5,000 commits on any branch, most commits first, through \
               .mailmap, leaving you out; or type Name <email> for anyone else.",
        git: "git commit --trailer \u{201c}Co-authored-by: Name <email>\u{201d}; git log --all --no-merges --format=%aN %aE",
        cases: &[
            (
                "Someone shows twice?",
                "Two emails for one person: a .mailmap file joins them.",
            ),
            (
                "Not credited on GitHub?",
                "The email must belong to their account there (a noreply address works).",
            ),
            (
                "Amending?",
                "The last commit's co-author lines become chips again, and git writes them back.",
            ),
        ],
    },
    Topic {
        id: "co-author-remove",
        group: Group::Everyday,
        title: "Removing a co-author",
        on: &["Remove * as a co-author"],
        what: "Takes that person out of the commit being written: no Co-authored-by: line \
               for them. Nothing is written until you commit, and the list of recent authors \
               still offers them.",
        git: "",
        cases: &[(
            "Already committed with them?",
            "Tick Amend: their line shows as a chip again; remove it, then amend.",
        )],
    },
    Topic {
        id: "commit-hooks",
        group: Group::Everyday,
        title: "Hooks",
        on: &[],
        what: "Scripts git runs as it commits: pre-commit (may refuse, after checking the \
               staged files), prepare-commit-msg and commit-msg (may change or refuse the \
               message), post-commit. They live in .git/hooks or core.hooksPath, and run \
               the same from egit as from a terminal. What they print shows under the form.",
        git: "run by git commit",
        cases: &[
            (
                "A hook waits for input?",
                "It has no terminal under egit: the editor is a no-op and prompts fail, so it ends as refused.",
            ),
            (
                "A formatter hook changed files?",
                "Its changes are in the work tree, not in the commit: stage them and amend, or commit again.",
            ),
            (
                "Want to skip them once?",
                "egit always runs them; git commit --no-verify in a terminal skips pre-commit and commit-msg.",
            ),
        ],
    },
    Topic {
        id: "stage",
        group: Group::Everyday,
        title: "Staging a file",
        on: &["Stage *", "Stage"],
        what: "The index (the staging area) is the next commit being prepared. Staging a file \
               copies its content as the work tree has it now into the index: later edits \
               need staging again. A deleted file's deletion is staged; a new one starts \
               being tracked. The work tree isn't touched.",
        git: "git add --all --pathspec-from-file=- --pathspec-file-nul (literal paths)",
        cases: &[
            (
                "Changed the file since its diff showed?",
                "Staging a whole file takes it as it is now; staging lines refuses (the diff is stale) and reads it again.",
            ),
            (
                "A submodule?",
                "Staging it records the commit it is at now as the one this repository records.",
            ),
            (
                "An ignored file?",
                "It isn't listed, so it can't be staged here: git add --force in a terminal would add it anyway.",
            ),
            (
                "A large file in a repository using LFS?",
                "If .gitattributes gives it to LFS, git stores a small pointer and LFS keeps the content; check it does before committing.",
            ),
        ],
    },
    Topic {
        id: "unstage",
        group: Group::Everyday,
        title: "Unstaging a file",
        on: &["Unstage *", "Unstage"],
        what: "Puts HEAD's version of the file back in the index, so the next commit leaves \
               it as it was. Your edits stay in the work tree, now listed as not staged. A \
               new file goes back to untracked; a staged deletion is undone in the index, \
               the file still deleted on disk.",
        git: "git reset --quiet --pathspec-from-file=- --pathspec-file-nul",
        cases: &[
            (
                "Before the first commit?",
                "There is no HEAD: unstaging just takes the file out of the index (git reset does that; git restore --staged wouldn't).",
            ),
            (
                "A staged rename?",
                "Both paths are unstaged, so the old one is back and the new one is untracked.",
            ),
        ],
    },
    Topic {
        id: "stage-all",
        group: Group::Everyday,
        title: "Stage all",
        on: &["Stage all"],
        what: "Stages every file listed as not staged: changes, deletions, and untracked \
               files alike, exactly the paths the list shows. Ignored files aren't listed, \
               so they stay out.",
        git: "git add --all (the listed paths)",
        cases: &[
            (
                "Swept in a file you didn't mean to commit?",
                "Unstage it before committing; for good, add it to .gitignore.",
            ),
            (
                "Thousands of files?",
                "One git add takes them all; the lists read again after.",
            ),
        ],
    },
    Topic {
        id: "unstage-all",
        group: Group::Everyday,
        title: "Unstage all",
        on: &["Unstage all"],
        what: "Takes every staged change out of the index, back to HEAD's versions. Nothing \
               in the work tree changes: every edit is still there, listed as not staged.",
        git: "git reset --quiet (the listed paths)",
        cases: &[(
            "Did it lose my changes?",
            "No: unstaging never touches the files, only what the next commit would hold.",
        )],
    },
    Topic {
        id: "stage-file",
        group: Group::Everyday,
        title: "Stage file, Unstage file",
        on: &["Stage file", "Unstage file"],
        what: "In a file's view, stages or unstages the whole file, as the per-file buttons \
               in the lists do. For a file in conflict, Stage file marks it resolved with the \
               content the work tree has now, conflict markers included if you left any.",
        git: "git add, or git reset, of that path",
        cases: &[
            (
                "Only some changes should go in?",
                "Select lines in the unified diff, then Stage lines.",
            ),
            (
                "Staged a conflict by mistake?",
                "Its conflict is gone from the index: redo the merge for that file (git checkout -m <path> in a terminal) or fix the file and stage again.",
            ),
        ],
    },
    Topic {
        id: "stage-lines",
        group: Group::Everyday,
        title: "Staging lines",
        on: &["Stage lines", "Unstage lines"],
        what: "Moves only the selected lines' changes across: egit rebuilds the index's copy \
               of the file with those additions and removals applied (or, unstaging, taken \
               back out), and writes it as a new blob. The work tree is never touched. Select \
               lines by dragging in the unified diff.",
        git: "git hash-object -w --no-filters --stdin; git update-index -z --index-info",
        cases: &[
            (
                "Disabled?",
                "No changed line is selected, or the diff is side by side: lines are picked in the unified one.",
            ),
            (
                "Refused as stale?",
                "The file or the index changed since the diff was read: it is read again; select anew.",
            ),
            (
                "A new file?",
                "Staging some of its lines adds it holding just those.",
            ),
        ],
    },
    Topic {
        id: "stage-hunk",
        group: Group::Everyday,
        title: "Staging a hunk",
        on: &["Stage hunk", "Unstage hunk"],
        what: "A hunk is one block of the diff: changed lines and the context around them, \
               under its @@ header. This stages (or unstages) every change of each hunk the \
               selection reaches into, without picking lines one by one.",
        git: "git hash-object -w --no-filters --stdin; git update-index -z --index-info",
        cases: &[(
            "Two changes landed in one hunk?",
            "Select just the lines you want and use Stage lines instead.",
        )],
    },
    Topic {
        id: "stage-folder",
        group: Group::Everyday,
        title: "Staging a folder",
        on: &[
            "Stage the folder *",
            "Unstage the folder *",
            "Stage the folder",
            "Unstage the folder",
        ],
        what: "In the tree of folders, stages or unstages every file the folder holds in that \
               list, however deep, in one go. Folders themselves aren't tracked by git: only \
               files are, so an empty folder never shows.",
        git: "git add --all, or git reset, of each file under it",
        cases: &[(
            "An empty folder to keep?",
            "git can't track it: put a file in it (often named .gitkeep).",
        )],
    },
    Topic {
        id: "discard-changes",
        group: Group::Everyday,
        title: "Discarding changes",
        on: &[
            "Discard changes…",
            "Discard file…",
            "Discard the folder's changes…",
            "Discard all…",
            "Discard",
        ],
        what: "Throws away changes not staged: a tracked file goes back to what the index holds \
               (what is staged, or the last commit where nothing is), and an untracked file is \
               deleted. What is staged stays: changes go down one step at a time, staged to \
               not staged (Unstage), not staged to gone (Discard). egit asks first, and keeps \
               each file's bytes before and after as blobs nothing refers to, so Undo puts \
               them back while the tab is open (unless a discard names over 1,000 files, or one \
               over 64 MiB).",
        git: "git restore --worktree (tracked files); git clean --force (untracked ones); git \
              hash-object -w --no-filters (kept for Undo)",
        cases: &[
            (
                "Throw staged changes away too?",
                "Unstage them first (they come back as not staged), then discard them.",
            ),
            (
                "Disabled for a file in conflict?",
                "Resolve it in the merge editor, or abort the operation.",
            ),
            (
                "Disabled for a submodule?",
                "Its page updates it to the commit recorded here.",
            ),
            (
                "An untracked folder that is a repository of its own?",
                "egit won't delete it: delete its folder yourself.",
            ),
            (
                "Undo says a file changed since?",
                "Something wrote to it after the discard: Undo won't overwrite that.",
            ),
            (
                "A file added with git add -N (intent to add)?",
                "Its index entry is empty, so discarding leaves the file empty.",
            ),
        ],
    },
    Topic {
        id: "ignore",
        group: Group::Everyday,
        title: "Ignoring files",
        on: &[
            "Ignore",
            "This file",
            "This folder",
            "Every * file",
            "Its folder, *",
            "In .gitignore, shared",
            "In info/exclude, only here",
        ],
        what: "Tells git to stop listing files it doesn't track: a pattern naming the file, \
               every file of its extension (in any folder), or a folder and all it holds. In \
               .gitignore (the work tree's top one) it is shared: commit the .gitignore and \
               everyone's git ignores them. In info/exclude it stays in this clone alone. egit \
               writes the pattern so it names exactly that path (characters like * and [ \
               escaped), then asks git whether the file is ignored now. Undo takes the line \
               back off.",
        git: "a line added to .gitignore or to the file git rev-parse --git-path info/exclude \
              names; git check-ignore",
        cases: &[
            (
                "Only offered for some files?",
                "Ignoring changes nothing for a file git already tracks: it stays tracked. Stop tracking it first (git rm --cached), then ignore it.",
            ),
            (
                "Still listed after ignoring it?",
                "A later pattern starting with ! (in .gitignore, a folder's own .gitignore, or info/exclude) brings it back: that one wins.",
            ),
            (
                "Where did .gitignore come from?",
                "egit made it at the work tree's top: it shows as a new file to commit.",
            ),
            (
                "Ignore it on every repository of mine?",
                "That is core.excludesFile (by default ~/.config/git/ignore): add the pattern there.",
            ),
        ],
    },
    Topic {
        id: "reveal",
        group: Group::Everyday,
        title: "Opening files and folders",
        on: &[
            "Open file",
            "Show in folder",
            "Open the folder",
            "Open its folder",
        ],
        what: "Hands a file of the work tree to the desktop: Open file opens it in the app your \
               desktop opens it with (as a double click would), Show in folder opens your file \
               manager with it picked out, and Open the folder opens a folder there. It is \
               always the work tree's file as it is now, even from a commit's file view.",
        git: "nothing: xdg-open (gio open), the file manager's ShowItems over D-Bus; open and \
              open -R on macOS; Explorer (/select) on Windows",
        cases: &[
            (
                "The file is gone from the work tree?",
                "Its nearest folder still there opens instead.",
            ),
            (
                "It opens in the wrong app?",
                "That is the desktop's choice for the file's type: change its default app there.",
            ),
            (
                "Show in folder only opened the folder?",
                "Your file manager doesn't answer the freedesktop ShowItems call over D-Bus, so egit opens the folder instead.",
            ),
        ],
    },
    Topic {
        id: "discard-lines",
        group: Group::Everyday,
        title: "Discarding lines or a hunk",
        on: &["Discard lines", "Discard hunk"],
        what: "Puts the selected lines back as the index has them, in the work tree: removed \
               lines return and added ones go; every other change stays. egit rebuilds the \
               file from the index's copy with every change but those picked, and writes it \
               through the file's filters (line endings, LFS) as git checks a file out. It \
               doesn't ask first: Undo puts the lines back.",
        git: "git cat-file --filters --path=<path> <blob>; git hash-object -w --no-filters (kept \
              for Undo)",
        cases: &[
            (
                "Refused as stale?",
                "The file or the index changed since the diff was read: look at it again and select anew.",
            ),
            (
                "Disabled?",
                "No changed line is selected, the diff is side by side (lines are picked in the unified one), or a task is running.",
            ),
            (
                "A deleted file?",
                "Discarding some of its removed lines brings the file back holding just those.",
            ),
            (
                "Two changes in one hunk?",
                "Discard hunk takes both: select lines to take one.",
            ),
        ],
    },
    Topic {
        id: "show-changes",
        group: Group::Everyday,
        title: "Show changes",
        on: &["Show changes"],
        what: "Opens the file's diff in place of the commit table, as clicking its row does: \
               what isn't staged against the index, or what is staged against HEAD.",
        git: "git diff -- <path>, or git diff --cached -- <path>",
        cases: &[],
    },
    Topic {
        id: "copy-path",
        group: Group::Everyday,
        title: "Copy path",
        on: &["Copy path"],
        what: "Copies the file's (or folder's) path, from the repository's top, as git writes it \
               (forward slashes).",
        git: "",
        cases: &[],
    },
    Topic {
        id: "not-staged",
        group: Group::Everyday,
        title: "Not staged",
        on: &["Not staged"],
        what: "Files whose work tree content differs from the index's: edits not staged yet, \
               deletions, and files git doesn't track (untracked). Their diff compares the \
               index with the work tree, so a file can be here and in Staged at once: its \
               staged part there, its newer edits here.",
        git: "git status --porcelain=v2 -z --untracked-files=all; git diff -- <path>",
        cases: &[
            (
                "Throw a file's changes away?",
                "Right-click it: Discard changes… (asked first), or discard lines or a hunk in its diff. Undo brings them back.",
            ),
            (
                "Listed but you changed nothing?",
                "Its line endings or mode may differ (core.autocrlf, core.fileMode): open it to see.",
            ),
            (
                "Doesn't refresh?",
                "egit watches the work tree; where it can't, it reads again every few seconds and when the window regains focus.",
            ),
        ],
    },
    Topic {
        id: "staged",
        group: Group::Everyday,
        title: "Staged",
        on: &["Staged"],
        what: "What the next commit will hold beyond HEAD: files whose index content differs \
               from HEAD's. Their diff compares HEAD with the index (renames found), not with \
               the work tree, so edits made after staging aren't in it until staged again.",
        git: "git diff --cached -M -- <path>",
        cases: &[
            (
                "Before the first commit?",
                "Everything staged is new: there is no HEAD to compare with.",
            ),
            (
                "Want them out of the next commit?",
                "Unstage them: your edits stay in the work tree.",
            ),
        ],
    },
    Topic {
        id: "work",
        group: Group::Everyday,
        title: "Uncommitted changes",
        on: &[],
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
            (
                "Thousands of untracked files?",
                "Usually build output: add their folder to .gitignore and they leave the list.",
            ),
        ],
    },
    Topic {
        id: "untracked",
        group: Group::Everyday,
        title: "Untracked files",
        on: &[],
        what: "Files in the work tree that git has never been told about and that no \
               .gitignore pattern hides. They aren't in any commit until staged, and a reset, \
               switch, or stash leaves them where they are. Their diff shows every line as \
               added.",
        git: "git status --untracked-files=all; git diff --no-index -- /dev/null <path>",
        cases: &[
            (
                "Gone after a switch?",
                "git never removes untracked files on a switch; it refuses a switch that would overwrite one.",
            ),
            (
                "Stash them too?",
                "The stash dialog's option includes untracked files (git stash --include-untracked).",
            ),
        ],
    },
    Topic {
        id: "ignored-files",
        group: Group::Everyday,
        title: "Ignored files",
        on: &[],
        what: "Patterns in .gitignore files (and .git/info/exclude, and core.excludesFile) \
               hide untracked files from git: they aren't listed, staged, or stashed. A file \
               already tracked isn't affected by a pattern added later.",
        git: "git check-ignore",
        cases: &[
            (
                "Added a pattern but the file still shows?",
                "It is tracked: stop tracking it with git rm --cached <path> in a terminal, then commit.",
            ),
            (
                "Ignored but needed in the commit?",
                "git add --force <path> in a terminal, or an exception pattern starting with !.",
            ),
        ],
    },
    Topic {
        id: "changed-files",
        group: Group::Changes,
        title: "Changed files",
        on: &["Changed files"],
        what: "The files a commit changed against its first parent (against nothing, for the \
               first commit), with how each changed and, once counted, the lines it gained \
               and lost. A merge lists what it brought into the branch it was made on. Click \
               a file to see its diff. In Preferences, the same words choose how such lists \
               show at first.",
        git: "git diff-tree -r -z --raw --no-abbrev -M; git diff-tree --numstat for the lines",
        cases: &[
            (
                "Lines not counted?",
                "In a partial clone, counting would download every file's contents, so egit doesn't.",
            ),
            (
                "A merge shows no files?",
                "It merged cleanly something its first parent already had, or only resolved conflicts the same way.",
            ),
        ],
    },
    Topic {
        id: "file-lists",
        group: Group::Changes,
        title: "A list, or a tree of folders",
        on: &[
            "A list",
            "A tree of folders",
            "Show the files as a list",
            "Show the files in a tree of their folders",
        ],
        what: "How lists of changed files show: each path in a list, or folded into their \
               folders (a folder holding only a folder shows as one, src/core). In a tree, \
               click a folder to fold it; its count is the files inside. The choice is kept, \
               for commits and the work tree alike.",
        git: "",
        cases: &[(
            "Where did a file go?",
            "Its folder is folded: unfold it, or switch back to the list.",
        )],
    },
    Topic {
        id: "status-letters",
        group: Group::Changes,
        title: "A, M, D, R, C, T, U, !",
        on: &[],
        what: "The letter beside a file says how it changed: A added, M modified, D deleted, \
               R renamed (git matched an old path to a new one), C copied, T its type changed \
               (a file became a link or a submodule), U untracked (new, not yet known to \
               git), ! in conflict. Hover for the word.",
        git: "git status --porcelain=v2; git diff-tree --raw",
        cases: &[
            (
                "A rename shows as D and A?",
                "Before staging both sides, git sees a deletion and an untracked file; staged, they become one rename.",
            ),
            (
                "Modified, but the diff is empty?",
                "Only its mode changed (the executable bit), or line endings that git normalizes.",
            ),
        ],
    },
    Topic {
        id: "file-view",
        group: Group::Changes,
        title: "A file's view",
        on: &[],
        what: "A file opened from a commit or the work tree shows in the commit table's place: \
               how it changed and its path in the bar, then its diff or the file itself. \
               History (or Escape) goes back to the commits, the selection kept. From a \
               page or a tool, the same button goes back too.",
        git: "",
        cases: &[(
            "The file changed on disk meanwhile?",
            "A work tree file's diff is read again whenever the status changes.",
        )],
    },
    Topic {
        id: "show-diff",
        group: Group::Changes,
        title: "Diff",
        on: &["Diff"],
        what: "The file's changes: for a commit, against its first parent; for the work \
               tree, what isn't staged (index against work tree) or what is (HEAD against \
               index). Removed lines are red with their old numbers, added ones green with \
               their new numbers, a few unchanged lines around each change.",
        git: "git diff-tree -p --no-textconv --full-index -M; git diff (--cached) for the work tree",
        cases: &[
            (
                "Your diff.algorithm?",
                "egit passes it to git (plumbing wouldn't read it), so diffs match git diff's.",
            ),
            (
                "External diff tools, textconv?",
                "Left out on purpose: the diff is git's own text, which staging lines relies on.",
            ),
        ],
    },
    Topic {
        id: "markdown-preview",
        group: Group::Changes,
        title: "Markdown previewed",
        on: &["Preview", "Previous change", "Next change", "Change * of *"],
        what: "A Markdown file (.md) as it reads, rendered: headings, lists, tables, code, \
               links. A change shows before and after side by side, the blocks that changed \
               tinted (removed on the left, added on the right; a line where the other side \
               has blocks this one hasn't), opening at the first; Previous change and Next \
               change (Alt+↑, Alt+↓) step through them. A new file, or a deleted one, shows \
               the side it has. For the work tree, as the diff's sides are: what is staged \
               against HEAD, or the file against the index.",
        git: "git cat-file (each side's blob), or the file on disk",
        cases: &[
            (
                "Both sides look the same?",
                "Look for the tint: the change may be far down, and each side opens at the first. “Nothing changed in how it reads” means only blank lines or spaces ending lines did, or a reference link's definition.",
            ),
            (
                "A whole paragraph is marked for one word?",
                "Blocks are compared whole (a paragraph, a list's item, a table): the Diff shows the words.",
            ),
            (
                "An image shows as a link?",
                "egit fetches nothing to draw a document (a picture on the web can tell its author who looked): the link opens it in your browser.",
            ),
            (
                "A link does nothing?",
                "egit opens web pages (https) only; a link to another file in the repository isn't followed. A link to a heading (#…) scrolls to it.",
            ),
            (
                "Too large?",
                "Past 1 MiB a document isn't previewed: see it as the File instead.",
            ),
        ],
    },
    Topic {
        id: "show-file",
        group: Group::Changes,
        title: "File",
        on: &["File"],
        what: "The whole file, highlighted, as the commit left it (or, for a deleted file, as \
               it was before), or for the work tree as the index or the work tree holds it. \
               It is read the first time you ask, through git's object reader.",
        git: "git cat-file (the blob), or the file on disk",
        cases: &[
            (
                "Shows a short text with oid and size?",
                "That is an LFS pointer: the content is stored outside the repository.",
            ),
            (
                "Too large to show?",
                "Files over 256 MiB aren't read into the window.",
            ),
        ],
    },
    Topic {
        id: "diff-layout",
        group: Group::Changes,
        title: "Unified or side by side",
        on: &["Unified", "Side by side"],
        what: "Unified shows one column: removed lines then added ones, as git prints them. \
               Side by side puts the old file on the left and the new on the right, each \
               block's removed lines beside its added ones, blanks making up the difference. \
               Preferences sets which a file opens with.",
        git: "",
        cases: &[(
            "Can't select lines to stage?",
            "Lines are picked in the unified diff: switch side by side off.",
        )],
    },
    Topic {
        id: "whole-file",
        group: Group::Changes,
        title: "Whole file",
        on: &["Whole file"],
        what: "Shows every unchanged line around the changes, not just a few: the diff is read \
               again with all of its context. Changes still stand out by colour, and \
               staging lines works the same.",
        git: "git diff --unified=1000000000",
        cases: &[(
            "Slow on a huge file?",
            "It is read in the background; scrolling stays quick, as only what shows is drawn.",
        )],
    },
    Topic {
        id: "ignore-whitespace",
        group: Group::Changes,
        title: "Ignore whitespace",
        on: &["Ignore whitespace"],
        what: "Reads the diff again counting lines that differ only in spaces and tabs as the \
               same, so a change re-indented or reformatted shows what really changed. Only \
               how it is shown changes: the commit, or the file, stays as it is.",
        git: "git diff --ignore-all-space",
        cases: &[
            (
                "Can't stage lines?",
                "A diff ignoring whitespace isn't the file's: turn it off to pick lines, or stage the whole file.",
            ),
            ("Nothing left?", "The change was whitespace alone."),
        ],
    },
    Topic {
        id: "wrap-lines",
        group: Group::Changes,
        title: "Wrap lines",
        on: &["Wrap lines"],
        what: "Long lines continue on the rows below instead of running off to the right. \
               Line numbers stay with each line's first row; nothing in the file changes. \
               Preferences sets whether files open wrapped.",
        git: "",
        cases: &[(
            "Side by side, wrapped?",
            "Both sides take the same rows, so lines stay level.",
        )],
    },
    Topic {
        id: "changed-words",
        group: Group::Changes,
        title: "Changed words",
        on: &[],
        what: "Where a removed line pairs with an added one, the words that differ are marked \
               more strongly, so a one-character fix stands out. egit pairs a block's removed \
               lines with its added ones in order, and compares words only when lines share \
               enough of them.",
        git: "",
        cases: &[(
            "No words marked?",
            "The lines are too different (or very long) to compare usefully, so whole lines show.",
        )],
    },
    Topic {
        id: "renames",
        group: Group::Changes,
        title: "Renames and copies",
        on: &[],
        what: "git records no renames: it finds them, matching a deleted path to an added one \
               whose content is at least half alike. The bar then says from which path, and \
               how alike (from old.rs, 92% alike). A rename with many edits may show as a \
               deletion and an addition instead.",
        git: "git diff -M, git diff-tree -M",
        cases: &[
            (
                "Renamed only by case, on macOS or Windows?",
                "Their file systems ignore case, so git may not notice: git mv -f old New in a terminal.",
            ),
            (
                "Want its history across the rename?",
                "File history follows renames (git log --follow).",
            ),
        ],
    },
    Topic {
        id: "line-endings",
        group: Group::Changes,
        title: "Line endings",
        on: &[],
        what: "Windows writes lines ending in CR LF, others LF. git can convert them as files \
               go in and out (core.autocrlf, or text and eol in .gitattributes). egit shows \
               the work tree's side as git would store it, after that conversion, so a diff \
               shows what a commit would hold.",
        git: "git diff (the clean filters and line-ending conversion applied)",
        cases: &[
            (
                "Every line shows as changed?",
                "The stored line endings change: either a tool rewrote them, or .gitattributes now asks for a conversion (git add --renormalize . in a terminal fixes them in one commit).",
            ),
            (
                "Modified, but nothing shows?",
                "Only the line endings on disk differ from what is stored, which git normalizes away.",
            ),
        ],
    },
    Topic {
        id: "mode-changes",
        group: Group::Changes,
        title: "Mode and type changes",
        on: &[],
        what: "git stores whether a file is executable, a symbolic link, or a submodule, \
               apart from its text. A change of mode alone (chmod +x) has no lines to show: \
               the view says nothing changed in the text. A change of type (T) swaps a file \
               for a link or a submodule.",
        git: "git diff (old mode / new mode lines)",
        cases: &[(
            "Every file changed mode at once?",
            "A file system or copy that loses the executable bit: core.fileMode false makes git ignore it (Windows sets it).",
        )],
    },
    Topic {
        id: "file-placeholder",
        group: Group::Changes,
        title: "Files without text to show",
        on: &[],
        what: "Some files have no lines to compare: binary files (git decides by looking for \
               zero bytes, or by .gitattributes), files over 256 MiB, Git LFS pointers, and \
               submodules. egit says what it is instead; images show their two sides as \
               pictures.",
        git: "git diff-tree -p (Binary files differ)",
        cases: &[
            (
                "A text file called binary?",
                "It holds a zero byte (UTF-16 does), or .gitattributes marks it binary or -diff.",
            ),
            (
                "Still want to know what changed?",
                "Its sizes before and after, or open each version with your own tools.",
            ),
        ],
    },
    Topic {
        id: "image-diff",
        group: Group::Changes,
        title: "Images",
        on: &[],
        what: "A changed PNG, JPEG, GIF, WebP, BMP, or ICO shows as pictures, before and after \
               side by side, each with its size in pixels and bytes, scaled down to fit (never \
               up) over a checkerboard so transparency shows. A new image has no before; a \
               deleted one no after.",
        git: "git cat-file (each side's blob), or the file on disk",
        cases: &[
            (
                "It couldn't be shown?",
                "It is larger than 64 MiB or 20,000 pixels a side, or its data is damaged: the reason shows in its place.",
            ),
            (
                "An LFS image?",
                "The repository holds only its pointer: the pointer's sizes show instead.",
            ),
        ],
    },
    Topic {
        id: "lfs-diff",
        group: Group::Changes,
        title: "A Git LFS file's change",
        on: &["Show the pointer's changes"],
        what: "Files kept in Git LFS are committed as small pointers (a version, the content's \
               oid, its size); the content lives on the LFS server. A diff of the pointer \
               says little, so egit shows the file's sizes before and after. This button \
               shows the pointer's own text diff.",
        git: "git diff-tree -p (the pointer files)",
        cases: &[(
            "Want the real contents compared?",
            "Check both versions out with Git LFS set up, then compare them with your own tools.",
        )],
    },
    Topic {
        id: "submodule-change",
        group: Group::Changes,
        title: "A submodule's change",
        on: &[],
        what: "A submodule is another repository; this one records only which of its commits \
               to check out (a gitlink). A change here moves that record: egit says from which \
               commit to which, and lists the commits that brings, when the submodule is set \
               up here and has them.",
        git: "git log <from>..<to> inside the submodule",
        cases: &[
            (
                "Listed as not staged, but you didn't touch it?",
                "Its checkout moved (you committed inside it, or switched it): stage it to record that, or update it back to the commit recorded.",
            ),
            (
                "No commits listed?",
                "It isn't set up here, or lacks those commits: set it up or fetch inside it.",
            ),
        ],
    },
    Topic {
        id: "diff-algorithm",
        group: Group::Changes,
        title: "How diffs are found",
        on: &[],
        what: "Several diffs can describe the same change. git's default (myers) is fast; \
               patience and histogram line up moved blocks and braces better. egit uses \
               whatever diff.algorithm your configuration sets, for every diff it shows.",
        git: "git diff --diff-algorithm=<diff.algorithm>",
        cases: &[(
            "A diff pairs the wrong braces?",
            "Set diff.algorithm to histogram in your profile's settings.",
        )],
    },
    Topic {
        id: "find-in-file",
        group: Group::Changes,
        title: "Finding in a file",
        on: &[],
        what: "⌘F over a diff or a file opens a find bar at its top right. Every match is \
               marked as you type, the current one stronger; Enter and Shift+Enter step \
               through them. Lower-case text matches either case; any capital makes it exact.",
        git: "",
        cases: &[(
            "Over 100,000 matches?",
            "Only the first 100,000 are kept: type more of what you're looking for.",
        )],
    },
    Topic {
        id: "copy-from-diff",
        group: Group::Changes,
        title: "Selecting and copying",
        on: &[],
        what: "Drag across a diff or a file to select lines; ⌘A selects all of one side, ⌘C \
               copies the bytes as they are (without + and - marks or numbers), Escape clears. \
               Dragging past an edge scrolls. In the unified diff of a work tree file, the \
               same selection picks lines to stage.",
        git: "",
        cases: &[(
            "Strange marks like ^M or <200B>?",
            "Invisible characters are written out so they can be seen: they are in the file, and copying gives the real bytes.",
        )],
    },
    Topic {
        id: "invisible-characters",
        group: Group::Changes,
        title: "Invisible and odd characters",
        on: &[],
        what: "egit draws what would otherwise hide: control characters as ^X, invisible or \
               direction-changing ones (zero-width spaces, bidirectional controls, a byte \
               order mark) as <200B>, and bytes that aren't valid UTF-8 as <FF>. A line \
               ending's CR is hidden. Tabs take the width set in Preferences.",
        git: "",
        cases: &[
            (
                "A line differs but looks the same?",
                "Look for trailing spaces, tabs against spaces, or one of these marks.",
            ),
            (
                "Not highlighted?",
                "Lines that aren't UTF-8, or longer than 4 KiB, are left plain.",
            ),
        ],
    },
    Topic {
        id: "syntax-colours",
        group: Group::Changes,
        title: "Colours of code",
        on: &[],
        what: "Files and diffs are coloured by their language, known from the file's \
               extension or name, or its first line (#!/bin/sh). A large file is coloured as \
               you reach it, the whole parsed once in the background.",
        git: "",
        cases: &[(
            "Not coloured?",
            "Its language isn't known from its name, or the line is over 4 KiB or not UTF-8.",
        )],
    },
    Topic {
        id: "partial-clone-files",
        group: Group::Changes,
        title: "Files in a partial clone",
        on: &[],
        what: "A partial clone (blobless) holds every commit and folder, but a file's contents \
               only once something needs them. Opening a diff or a file then fetches the \
               versions it shows from the remote, which takes a moment and the network.",
        git: "git fetches the missing objects as git diff-tree or cat-file read them",
        cases: &[
            (
                "Offline?",
                "Files never read before can't be shown; those read once stay.",
            ),
            (
                "Lines not counted?",
                "Counting would fetch every changed file, so egit leaves it out.",
            ),
        ],
    },
    Topic {
        id: "first-commit",
        group: Group::Everyday,
        title: "Before the first commit",
        on: &[],
        what: "A new repository has no commits, so HEAD names a branch that doesn't exist \
               yet (an unborn branch). The history is empty and the uncommitted changes show \
               at once. Everything staged is new, Amend has nothing to amend, and the first \
               commit creates the branch.",
        git: "git commit (the branch is made by it)",
        cases: &[
            (
                "Unstaging before the first commit?",
                "It works: egit unstages with git reset, which takes files out of the index when there is no HEAD.",
            ),
            (
                "The branch is master, you wanted main?",
                "Rename it after the first commit, or set init.defaultBranch for new repositories.",
            ),
        ],
    },
    Topic {
        id: "submodule-in-lists",
        group: Group::Everyday,
        title: "A submodule in the lists",
        on: &[],
        what: "A submodule shows as one entry: this repository records only which of its \
               commits to check out. It is listed when it is checked out at another commit \
               than the one recorded, or when it has changes of its own inside. Staging it \
               records its current commit here.",
        git: "git status --porcelain=v2 (the submodule's state); git add <path>",
        cases: &[
            (
                "Changes inside it?",
                "They belong to the submodule: open it in a tab, commit there, then stage it here to record that commit.",
            ),
            (
                "You never meant to move it?",
                "Don't stage it: update it back to the commit recorded from the Submodules menu.",
            ),
        ],
    },
    Topic {
        id: "lines-counted",
        group: Group::Changes,
        title: "Lines gained and lost",
        on: &[],
        what: "The green and red numbers beside a commit's files are the lines each added and \
               removed, as git diff --numstat counts them; the section's title sums them. A \
               changed line counts once each way. They are counted after the list shows, in \
               the background.",
        git: "git diff-tree -r -z --numstat -M",
        cases: &[
            (
                "A file without numbers?",
                "Binary files have no lines to count.",
            ),
            (
                "Lines not counted?",
                "In a partial clone counting would download every file, so egit leaves it.",
            ),
        ],
    },
    Topic {
        id: "symlinks",
        group: Group::Changes,
        title: "Symbolic links",
        on: &[],
        what: "git stores a symbolic link as the path it points to, not the file it reaches. \
               Its diff shows that path changing, and committing a link never commits its \
               target's contents.",
        git: "git diff (mode 120000)",
        cases: &[(
            "On Windows, links became small text files?",
            "Without permission to make links (core.symlinks false), git writes the target path into a plain file.",
        )],
    },
    Topic {
        id: "large-files-in-commits",
        group: Group::Everyday,
        title: "Committing a large file",
        on: &[],
        what: "Every version of every committed file stays in the history: a large file \
               committed once makes every clone larger for good, even after it is deleted in \
               a later commit. Forges refuse pushes of very large files (GitHub: 100 MB a \
               file). Git LFS keeps such files outside the history.",
        git: "",
        cases: &[
            (
                "Just committed it, not pushed?",
                "Undo the commit (or unstage it and amend), then track its kind of file with Git LFS first.",
            ),
            (
                "Already pushed?",
                "Deleting it in a new commit doesn't shrink anything; removing it means rewriting history everyone has.",
            ),
        ],
    },
    Topic {
        id: "secrets-committed",
        group: Group::Everyday,
        title: "Committed a secret",
        on: &[],
        what: "A password or key in a commit is in every copy of the repository that has \
               that commit. Before it is pushed, it can still be taken out: undo the commit \
               or amend it without the file. Once pushed, treat it as public.",
        git: "",
        cases: &[
            (
                "Pushed already?",
                "Change the secret (revoke the key) first: rewriting history afterwards doesn't reach clones and forks already made.",
            ),
            (
                "To keep it out next time?",
                "Add the file to .gitignore before it is ever staged.",
            ),
        ],
    },
    Topic {
        id: "work-tree-watch",
        group: Group::Everyday,
        title: "How egit notices changes",
        on: &[],
        what: "egit watches the work tree's folders (on Linux each folder git tracks, plus new \
               ones not ignored) and reads the status again soon after a file changes, at most \
               once a second while files keep changing, and when the window regains focus. \
               Ignored folders like node_modules cost nothing.",
        git: "git status --porcelain=v2 -z --untracked-files=all",
        cases: &[
            (
                "The status bar says changes outside egit aren't followed?",
                "The system's limit on watches is used up (often by editors): egit reads every 5 seconds instead, and keeps trying.",
            ),
            (
                "An edit doesn't show?",
                "Switch away from egit and back: regaining focus reads the status again.",
            ),
        ],
    },
    Topic {
        id: "index-lock",
        group: Group::Everyday,
        title: "Another git is running",
        on: &[],
        what: "git writes the index under a lock file, .git/index.lock, so two commands can't \
               change it at once. egit's own writes wait and try again while another git (an \
               editor's, a terminal's) holds it, and its background reads never take it.",
        git: "",
        cases: &[(
            "index.lock exists, and nothing is running?",
            "A git that crashed left it: once you are sure no git runs, delete .git/index.lock.",
        )],
    },
    Topic {
        id: "assume-unchanged",
        group: Group::Everyday,
        title: "A file that never shows as changed",
        on: &[],
        what: "git can be told to stop looking at a tracked file (git update-index \
               --assume-unchanged, or --skip-worktree, as sparse checkouts use): edits to it \
               then never show in the status or the lists, and aren't committed.",
        git: "git update-index --no-assume-unchanged / --no-skip-worktree <path>",
        cases: &[(
            "Your edit to a config file is ignored?",
            "Look for those flags with git ls-files -v (lower-case h or S) in a terminal.",
        )],
    },
    Topic {
        id: "hunk-header",
        group: Group::Changes,
        title: "@@ -10,7 +10,8 @@",
        on: &[],
        what: "Each block of a diff starts with a header: where the block starts and how many \
               lines it covers in the old file (-10,7) and the new one (+10,8). After it, git \
               names the function or section the block sits in, when it can tell.",
        git: "git diff",
        cases: &[(
            "The wrong function named?",
            "git guesses from the lines above; .gitattributes can give it a language's rule (diff=python).",
        )],
    },
    Topic {
        id: "diff-gutter",
        group: Group::Changes,
        title: "Line numbers in a diff",
        on: &[],
        what: "Beside each line, its number in the old file and in the new one: a removed line \
               has only the old, an added one only the new, an unchanged one both. The + and \
               - marks say which. Three unchanged lines surround each change unless Whole \
               file is ticked.",
        git: "git diff --unified=3",
        cases: &[(
            "Numbers jump?",
            "Lines between two blocks didn't change and aren't shown: Whole file shows them.",
        )],
    },
    Topic {
        id: "no-newline-at-end",
        group: Group::Changes,
        title: "No newline at end of file",
        on: &[],
        what: "A file whose last line has no line ending is noted by git. Adding that ending \
               later changes the last line, so a diff shows it removed and added again, \
               looking the same. Most tools and editors add it; some projects check for it.",
        git: "git diff (\\ No newline at end of file)",
        cases: &[(
            "Staging lines near the end?",
            "egit keeps the missing ending as it was unless lines picked after it need one.",
        )],
    },
    Topic {
        id: "hooks",
        group: Group::Everyday,
        title: "Git's hooks",
        on: &["Hooks", "Installed"],
        what: "Hooks are programs git runs around what it does: pre-commit and commit-msg before a \
               commit is made (linters, formatters, tests: a failure stops the commit), \
               post-checkout after a switch, pre-push before a push, and more. They live in the \
               repository's hooks folder (.git/hooks, or the one core.hooksPath names, as husky \
               sets). egit runs git, so they run as they would in a terminal; what they print \
               shows in the output at the tab's bottom as it comes.",
        git: "git rev-parse --git-path hooks (where git looks); the hooks themselves, run by git",
        cases: &[
            (
                "The commit seems stuck?",
                "A hook is running (tests take a while): the output shows what it prints, and Cancel stops it, no commit made.",
            ),
            (
                "A hook refused the commit?",
                "Nothing was committed and nothing is lost: what it printed says why. Fix it, or skip the hooks for this commit.",
            ),
            (
                "A hook isn't running?",
                "On Linux and macOS it must be executable (chmod +x); a .sample file never runs. A hooks folder of the team's needs core.hooksPath set (husky does it on install).",
            ),
            (
                "It works in a terminal, not here?",
                "Hooks see egit's environment, not your shell's: a tool your shell's profile adds to PATH (nvm, pyenv) may be missing. Start egit from that shell, or give the hook the full path.",
            ),
            (
                "pre-commit (the framework) wants to install environments?",
                "The first run downloads them and takes a while; its progress shows in the output.",
            ),
        ],
    },
    Topic {
        id: "skip-hooks",
        group: Group::Everyday,
        title: "Skipping hooks",
        on: &["Skip hooks"],
        what: "Commits without running the pre-commit and commit-msg hooks, this commit only \
               (git commit --no-verify). prepare-commit-msg and post-commit run anyway, as git \
               runs them. It is for a commit the hooks can't judge (work in progress, a hook \
               that is broken), not a way around checks a team relies on: the server or CI may \
               still check the same things.",
        git: "git commit --no-verify",
        cases: &[
            (
                "Skipped by mistake?",
                "Amend the commit with the box unticked: the hooks run then.",
            ),
            (
                "Push hooks too?",
                "This box skips only the commit's; a pre-push hook still runs when you push.",
            ),
        ],
    },
    Topic {
        id: "output-panel",
        group: Group::Egit,
        title: "The output",
        on: &[
            "Show the output",
            "Hide the output",
            "Copy the output",
            "Clear the output",
            "Output",
        ],
        what: "What the commands egit ran printed, as a terminal would show it: each command \
               ($ git commit --file=-), then what git and its hooks printed as it came, in the \
               colours they used, then how it ended. One run per task or commit; the bar at the \
               tab's bottom says what ran or runs (its hooks, for how long), and opens into the \
               lines (⌘J). It opens by itself while hooks run past a moment, and when a task fails \
               saying something; it closes again after a run it opened for went well.",
        git: "",
        cases: &[
            (
                "Only the last run?",
                "The list at the bar's right holds the last sixteen; Clear forgets those that ended.",
            ),
            (
                "Doesn't open by itself?",
                "Preferences, Repositories, Output: it can wait for you (⌘J).",
            ),
            (
                "Reads show nothing?",
                "Only commands that change the repository print here: reading it (the history, the status) would drown what matters.",
            ),
            (
                "Copy?",
                "Copies every line of the run shown, to paste into an issue or a chat.",
            ),
        ],
    },
    Topic {
        id: "output-cancel",
        group: Group::Egit,
        title: "Cancelling what runs",
        on: &[],
        what: "Stops the task or commit under way: git and every program it started (its hooks \
               among them) are asked to stop (SIGTERM to the whole group; on Windows, as Git for \
               Windows stops a process), so they remove their lock files. A commit stopped in \
               its pre-commit hook isn't made; a push stopped in its pre-push hook sends \
               nothing.",
        git: "SIGTERM to git's process group",
        cases: &[
            (
                "A hook left something half done?",
                "A hook stopped part-way may leave what it changed (files it formatted, say): look at the work tree before trying again.",
            ),
            (
                "Didn't stop at once?",
                "A program that ignores the signal goes on until it ends; egit waits for it rather than leave it running unseen.",
            ),
        ],
    },
    Topic {
        id: "hooks-folder",
        group: Group::Everyday,
        title: "Where hooks live",
        on: &["Folder"],
        what: "The folder git looks in for hooks: the repository's .git/hooks, unless \
               core.hooksPath names another (a team's shared folder in the repository, such as \
               husky's .husky, set by its install script). Hooks in .git/hooks are yours alone: \
               a clone never brings them.",
        git: "git rev-parse --git-path hooks; git config core.hooksPath",
        cases: &[(
            "A teammate's hooks don't run for me?",
            "Hooks don't travel with a clone: the project's setup (npm install for husky, pre-commit install) puts them in place.",
        )],
    },
];
