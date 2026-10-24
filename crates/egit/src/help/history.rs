//! Reading the history: the graph and its columns, a commit's details, searching, the reflog,
//! blame and a file's history, undo; egit's own ways around (the palette, the help, the
//! sidebar); and the common words many places share (Cancel, OK, Next, Name …).

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "start",
        group: Group::Egit,
        title: "How egit works",
        on: &[],
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
            (
                "Can I keep using git in a terminal?",
                "Yes, at the same time: egit watches the repository and catches up with whatever git does there.",
            ),
        ],
    },
    Topic {
        id: "history",
        group: Group::Looking,
        title: "The commit graph",
        on: &[],
        what: "Every commit, newest first, each joined to its parents by lines: a branch's \
               commits run down one lane, a merge joins two (its node hollow), and the commit \
               checked out (HEAD) is ringed. Badges are the branches and tags pointing at a \
               commit. The row above the newest shows changes not committed yet. When the \
               column is narrower than the graph, it shows “‹ Graph ›” and pans to the lanes \
               on screen.",
        git: "git rev-list --topo-order --parents; refs from for-each-ref",
        cases: &[
            (
                "Why is an older commit above a newer one?",
                "The order is topological: a commit is always above its parents, whatever their dates say. A rebased commit keeps its old author date.",
            ),
            (
                "Too many branches?",
                "Hide some with their eye in the sidebar, show only one, or pin one to the left: only what is shown changes.",
            ),
            (
                "Slow on a huge repository?",
                "Write a commit-graph file (egit offers one past 10,000 commits): git walks history far faster with it.",
            ),
            (
                "A shallow clone?",
                "Its history stops at the depth it was cloned with: the oldest commits shown have parents the clone doesn't hold.",
            ),
        ],
    },
    Topic {
        id: "message-column",
        group: Group::Looking,
        title: "The Message column",
        on: &[],
        what: "Each commit's subject: its message's first paragraph, on one line, as git's \
               %s gives it. Before it, the badges of the branches and tags at that commit and \
               of an open pull or merge request; a seal when the commit is signed. The whole \
               message is in the details.",
        git: "each row's commit read as it scrolls into view (gitoxide, or git cat-file)",
        cases: &[
            (
                "Two sentences run together?",
                "The message has no blank line after its first line, so git counts both as the subject.",
            ),
            (
                "Cut short?",
                "Drag the line at the column's right, or select the commit: the details show it whole.",
            ),
            (
                "“+2” among the badges?",
                "More branches or tags than fit: the details list them all.",
            ),
        ],
    },
    Topic {
        id: "author-column",
        group: Group::Looking,
        title: "Author",
        on: &["Author"],
        what: "Who wrote the change: the name and email recorded in the commit when it was \
               made (their user.name and user.email then), with their initials as an avatar \
               (drawn, never fetched). It is not who committed it, pushed it, or merged it: \
               after a rebase, a cherry-pick, or an applied patch, the author stays and the \
               committer is whoever did that. Co-authors come from Co-authored-by lines.",
        git: "the commit's author header (git log --format=%an %ae)",
        cases: &[
            (
                "My commits show the wrong name or email?",
                "Set them in your Profile (user.name, user.email). Commits already made keep theirs; amending keeps the author too.",
            ),
            (
                "One person, two names?",
                "Different emails or spellings over time. The column shows what each commit says; a .mailmap file unites them in git's listings and the co-author picker.",
            ),
            (
                "Finding someone's commits?",
                "Search, by Author: their name or email, any part of it.",
            ),
        ],
    },
    Topic {
        id: "date-column",
        group: Group::Looking,
        title: "Date",
        on: &["Date"],
        what: "When the author wrote the commit (the author date), in your time zone, which \
               the column's title names: “Date (CEST)”. Each commit is shown at the offset \
               your zone had at that moment, summer time or not. The details show the same, \
               and on hover the commit's own clock, as git log shows it.",
        git: "the commit's author date; your zone from TZ or the system's setting",
        cases: &[
            (
                "Dates out of order?",
                "The graph follows parents, not dates. A rebase or cherry-pick keeps the old author date, and a wrong clock writes wrong dates.",
            ),
            (
                "When was it committed, then?",
                "The details show a Committer and Committed date when they differ from the author's.",
            ),
            ("Rather “5 minutes ago”?", "Preferences, Commit dates."),
        ],
    },
    Topic {
        id: "commit-column",
        group: Group::Looking,
        title: "The Commit column",
        on: &[],
        what: "Each commit's name, shortened to its first 8 digits: enough to tell commits \
               apart in nearly every repository, and what git accepts wherever a commit is \
               named. The details show it longer, and copy it whole.",
        git: "",
        cases: &[(
            "Copy one?",
            "Right-click the row: Copy commit name. It copies the whole name.",
        )],
    },
    Topic {
        id: "commit-name",
        group: Group::Looking,
        title: "Commit names",
        on: &["Copy the commit's name"],
        what: "A commit is named by a hash of everything in it: its files, its parents, its \
               author, committer, dates, and message. 40 hex digits (SHA-1), or 64 in a \
               SHA-256 repository; the first 7 or 8 are usually enough to name it. Change \
               anything (amend, rebase, reword) and it is a new commit with a new name; the \
               old one still exists until git cleans it away.",
        git: "git rev-parse",
        cases: &[
            (
                "Same changes, different name?",
                "A cherry-pick or rebase gives the commit another parent and committer date, so another name.",
            ),
            (
                "A short name is ambiguous?",
                "Two objects share those first digits: git asks for more. Copying gives the whole name.",
            ),
            (
                "Find a commit by its name?",
                "Search, by Commit: four or more digits, or a branch or tag name.",
            ),
        ],
    },
    Topic {
        id: "committer",
        group: Group::Looking,
        title: "Committer and Committed",
        on: &["Committer", "Committed"],
        what: "Who made this commit object, and when. Usually the author, at the same moment, \
               so egit shows them only when they differ: after a rebase, a cherry-pick, an \
               amend, a patch applied with git am, or a merge made on a forge, the author and \
               the author date stay and the committer is whoever, whenever, that was.",
        git: "the commit's committer header (git log --format=%cn %cd)",
        cases: &[
            (
                "Committed by GitHub or GitLab?",
                "Edits and merges made on the website are committed by the forge, often signed with its key.",
            ),
            (
                "Why did rebasing change it?",
                "A rebase makes new commits: their committer date is when the rebase ran.",
            ),
        ],
    },
    Topic {
        id: "parents",
        group: Group::Looking,
        title: "Parents",
        on: &["Parent", "Parents"],
        what: "The commit or commits this one was made on. One for an ordinary commit; two or \
               more for a merge, the first being the branch that was checked out and the \
               others what was merged into it; none for a first commit. Click one to go to \
               it. A commit's changed files are listed against its first parent.",
        git: "the commit's parent lines (git rev-list --parents)",
        cases: &[
            (
                "Which parent of a merge is mine?",
                "The first: the branch you were on when you merged. Its changes listed are what the merge brought in.",
            ),
            (
                "Several commits have no parent?",
                "Each is a root: a first commit, an orphan branch (gh-pages), or a history grafted in by merging unrelated histories.",
            ),
            (
                "Clicking a parent does nothing?",
                "The graph doesn't show it: its branch is hidden, only some branches are shown, or a shallow clone doesn't hold it.",
            ),
        ],
    },
    Topic {
        id: "refs-fact",
        group: Group::Looking,
        title: "Refs",
        on: &["Refs"],
        what: "The branches, remote-tracking branches (origin/main), and tags pointing at this \
               exact commit, as badges, each with its menu as in the graph. A remote-tracking \
               branch is where that remote's branch was at your last fetch, not where it is \
               now.",
        git: "git for-each-ref",
        cases: &[
            (
                "origin/main isn't where GitHub shows main?",
                "Fetch: remote-tracking branches move only when you fetch, pull, or push.",
            ),
            (
                "A tag someone pushed is missing?",
                "Fetching brings tags on the commits it fetches; a tag on a commit you don't have arrives with that commit.",
            ),
        ],
    },
    Topic {
        id: "signature-check",
        group: Group::Looking,
        title: "Signature",
        on: &["Signature"],
        what: "What checking the commit's signature found, as git log --show-signature checks \
               it, with your own keys and programs: good; good but by a key not known to be \
               its owner's; expired; by a key since revoked; bad (the commit doesn't match its \
               signature); or not checked, when the key or the program is missing here. Who \
               signed and with which key follow.",
        git: "git log --format=%G? %GS %GK (gpg, gpgsm, or ssh-keygen, as gpg.format says)",
        cases: &[
            (
                "Signed, but not checked?",
                "OpenPGP: import the signer's public key into gpg. SSH: set gpg.ssh.allowedSignersFile (Profile) to a file listing who may sign with which key.",
            ),
            (
                "The forge says Verified, egit doesn't?",
                "The forge checks against keys uploaded to it; here only your own keyring and allowed signers count.",
            ),
            (
                "Bad?",
                "The commit or its signature was altered after signing: don't trust it.",
            ),
        ],
    },
    Topic {
        id: "commit-graph",
        group: Group::Looking,
        title: "Writing a commit-graph",
        on: &["Write a commit-graph"],
        what: "A commit-graph file indexes every commit's parents and dates, so git can walk \
               and sort history without reading each commit. Without one, git reads a long \
               history whole before showing its first commit; with one, the graph starts at \
               once. It is a cache: safe to write, safe to delete, never shared.",
        git: "git commit-graph write --reachable",
        cases: &[
            (
                "Does it go stale?",
                "Commits made after it are read the usual way. git gc and git maintenance write it again; so do fetches with fetch.writeCommitGraph.",
            ),
            (
                "How large?",
                "A few dozen bytes a commit, in .git/objects/info.",
            ),
        ],
    },
    Topic {
        id: "show-only",
        group: Group::Looking,
        title: "Showing only some branches",
        on: &[
            "Hide *",
            "Show every branch",
            "Show every branch in the graph",
            "Show only *",
            "Show * too",
        ],
        what: "Shows the history of only the branches chosen: git walks from their tips alone, \
               so commits only other branches reach leave the graph, and so do their badges. \
               Show another too, or hide one of those shown; Show every branch puts the whole \
               history back. Only the view changes: no branch moves.",
        git: "git rev-list from those branches only (read again)",
        cases: &[
            (
                "Where did the other commits go?",
                "Nowhere: they are only not shown. Show every branch, from the line under the top bar.",
            ),
            (
                "Comparing two branches?",
                "Show only one, then the other too: the graph is just those two.",
            ),
        ],
    },
    Topic {
        id: "hide-branch",
        group: Group::Looking,
        title: "Hiding a branch",
        on: &["Hide * from the graph"],
        what: "Leaves a branch out of the graph (the eye beside it in the sidebar does the \
               same): git walks every branch but those hidden, so commits only they reach \
               disappear; commits other branches reach too stay. The branch checked out \
               always shows.",
        git: "git rev-list --exclude=<branch> --all",
        cases: &[
            (
                "Bots open dozens of branches?",
                "Hide them (dependabot/…, renovate/…): the graph shows yours.",
            ),
            (
                "Shown again?",
                "Its eye in the sidebar, or Show them under the top bar.",
            ),
        ],
    },
    Topic {
        id: "pin",
        group: Group::Looking,
        title: "Pinning a branch to the left",
        on: &["Unpin", "Unpin *", "Pin * to the left"],
        what: "Keeps one branch's line in the leftmost lane: its tip and every first parent \
               down from it, the commits made on it and the merges into it. Other branches \
               lay out to its right, so the main line reads straight however much else \
               happens. Unpin lets the lanes fall where they come.",
        git: "",
        cases: &[(
            "A topic's commits appear in the pinned line?",
            "It was fast-forwarded or rebased into the branch: its commits became the branch's own, with no merge between.",
        )],
    },
    Topic {
        id: "show-them",
        group: Group::Egit,
        title: "Show them",
        on: &["Show them"],
        what: "Under the top bar, after branches were hidden: shows them in the graph again. On \
               the notice that a background fetch brought new commits: selects the newest on \
               your branch's upstream, to read before pulling. On the submodules banner: opens \
               the submodules' page.",
        git: "",
        cases: &[(
            "Nothing seems to happen?",
            "The commit or page shows in the commit table's place: look there, or press Escape to come back.",
        )],
    },
    Topic {
        id: "search",
        group: Group::Looking,
        title: "Searching the history",
        on: &["Search"],
        what: "Finds commits by their message, their author (name or email), a path they \
               changed, or their name (four or more digits, or a branch or tag). Every commit \
               the graph shows is searched, newest first; matches are marked, the first \
               selected, and Enter and Shift+Enter step through them. Words are matched as \
               typed, not as patterns.",
        git: "messages and authors read through gitoxide (else git rev-list --fixed-strings --grep, --author); git rev-list -- <path>; git rev-parse --verify",
        cases: &[
            (
                "Upper or lower case?",
                "Smart: all lower case matches any case; one capital and it matches exactly.",
            ),
            (
                "A path search misses commits?",
                "Like git log -- <path>, merges that only carried the change along are left out.",
            ),
            (
                "Path search refused?",
                "A partial clone without trees would have to download them all: clone with blob:none, or search by message.",
            ),
            ("Hidden branches?", "Only what the graph shows is searched."),
        ],
    },
    Topic {
        id: "next-previous",
        group: Group::Egit,
        title: "Next and Previous",
        on: &["Next", "Previous"],
        what: "Step through what was found, wrapping at either end: the commits a search \
               matched (Enter, Shift+Enter), the matches of a find in a file (⌘F), or, in the \
               merge editor, from conflict to conflict (Alt+↓, Alt+↑).",
        git: "",
        cases: &[("Disabled?", "Nothing was found yet, or nothing matches.")],
    },
    Topic {
        id: "close",
        group: Group::Egit,
        title: "Close",
        on: &["Close"],
        what: "Closes a search bar, a find bar, or this help (Escape does too). A search's marks \
               go with it; nothing in the repository changes.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "reflog",
        group: Group::Looking,
        title: "The reflog",
        on: &["Reflog"],
        what: "Every move of HEAD, or of one branch, newest first: each commit, switch, reset, \
               rebase, merge, and amend, and where it left things. Commits “lost” to a reset, \
               a rebase, an amend, or a deleted branch are still listed here: select one to see \
               it, then make a branch there to keep it for good.",
        git: "git log --walk-reflogs --max-count=10000",
        cases: &[
            (
                "Is it shared?",
                "No: it is yours alone, never pushed or fetched. A fresh clone's reflog is empty.",
            ),
            (
                "How far back?",
                "git keeps entries 90 days (gc.reflogExpire), and those no branch reaches 30 days, then git gc forgets them.",
            ),
            (
                "A deleted branch's reflog?",
                "Deleted with it. HEAD's reflog still has every commit that was checked out on it.",
            ),
            (
                "Undo a bad rebase?",
                "Find the move before “rebase (start)”, then Reset your branch to here.",
            ),
        ],
    },
    Topic {
        id: "reflog-reset",
        group: Group::Looking,
        title: "Reset to here, from the reflog",
        on: &["Reset * to here"],
        what: "Moves your current branch back (or forward) to where the selected move left \
               HEAD: the way out of a rebase, reset, or amend gone wrong. Soft keeps the \
               difference staged, Mixed keeps it unstaged, Keep moves only if your \
               uncommitted changes allow, Hard discards them.",
        git: "git reset --soft, --mixed, --keep, or --hard <commit>",
        cases: &[
            (
                "Already pushed the bad version?",
                "The remote has it: after resetting, pushing needs a force push, and anyone who pulled it has to reset too.",
            ),
            (
                "Changed my mind?",
                "Undo, or the reflog again: the reset itself is a move it lists.",
            ),
        ],
    },
    Topic {
        id: "blame",
        group: Group::Looking,
        title: "Blame",
        on: &["Blame"],
        what: "The file with each line's last commit beside it: who changed it, when, and in \
               which commit. Renames of the whole file are followed. Lines not committed yet \
               say so. Select a line to see its commit; the newest lines are shaded strongest.",
        git: "git blame --incremental",
        cases: &[
            (
                "A reformat hides the real change?",
                "Blame before this commit, or list such commits in a file and set blame.ignoreRevsFile to it: git skips them.",
            ),
            (
                "A line moved up from further down shows the move?",
                "Turn on Follow moved lines.",
            ),
            (
                "A huge file is slow?",
                "The first lines show while git works; following moved lines is far slower.",
            ),
        ],
    },
    Topic {
        id: "follow-moved-lines",
        group: Group::Looking,
        title: "Follow moved lines",
        on: &["Follow moved lines"],
        what: "Blames lines moved or copied within the file to the commit that wrote them, \
               rather than the one that moved them. Without it, a block moved from one place \
               to another is blamed on the move.",
        git: "git blame -M",
        cases: &[
            (
                "Moved from another file?",
                "Not followed: git blame -C does that, far more slowly, and egit doesn't ask for it.",
            ),
            (
                "Much slower?",
                "On large files, yes: git compares every block against the rest.",
            ),
        ],
    },
    Topic {
        id: "blame-before",
        group: Group::Looking,
        title: "Blame before this commit",
        on: &["Blame before this commit"],
        what: "Blames the file as it was just before the selected line's commit (in its \
               parent, under the path it had there): peels that change away to show who \
               wrote the line before it. Repeat to walk a line's story back.",
        git: "git blame <commit>^ -- <old path>",
        cases: &[(
            "Not offered?",
            "The line's commit has no parent here: it is where the line, or the history, begins.",
        )],
    },
    Topic {
        id: "file-history",
        group: Group::Looking,
        title: "File history",
        on: &["File history"],
        what: "Every commit that changed this file, newest first, following it across renames, \
               each with its change to the file below. A commit that only renamed it says so.",
        git: "git log --follow -M --name-status -- <path>",
        cases: &[
            (
                "History stops at a rename?",
                "git follows renames by similarity: a file renamed and heavily changed in one commit can lose the trail.",
            ),
            (
                "A merge isn't listed?",
                "git log lists a merge only when it changed the file itself, not when it only brought a change along.",
            ),
        ],
    },
    Topic {
        id: "blame-it-here",
        group: Group::Looking,
        title: "Blame it here",
        on: &["Blame it here"],
        what: "Blames the file as the selected commit left it: whose lines it held then. Not \
               offered for the commit that deleted it.",
        git: "git blame --incremental <commit> -- <path>",
        cases: &[],
    },
    Topic {
        id: "show-the-commit",
        group: Group::Looking,
        title: "Show the commit",
        on: &["Show the commit"],
        what: "Goes back to the commit table with this commit selected and scrolled to: from the \
               reflog, a blamed line, a file's history, a pull or merge request, or a bisection \
               that found its first bad commit.",
        git: "",
        cases: &[(
            "Not offered?",
            "The graph doesn't hold it: no branch shown reaches it (a commit only the reflog knows, a hidden branch), or a request's head isn't fetched.",
        )],
    },
    Topic {
        id: "back-to-history",
        group: Group::Egit,
        title: "History",
        on: &["History"],
        what: "Back to the commit table from a file, a tool (blame, the reflog, the rebase \
               editor, patches, requests), or a page (overview, settings, submodules, Git \
               LFS). Escape does the same.",
        git: "",
        cases: &[(
            "Does it cancel anything?",
            "Nothing that runs: a task carries on, its progress at the top bar's right.",
        )],
    },
    Topic {
        id: "undo",
        group: Group::Egit,
        title: "Undo",
        on: &["Undo"],
        what: "Takes back the last thing egit did here: a commit (its changes staged again), a \
               merge, rebase, pull, cherry-pick, revert, reset, switch, a branch or tag made, \
               moved, renamed, or deleted, a stash pushed or dropped. Each step is checked \
               first, so nothing moved since is overwritten. The last 100 are kept.",
        git: "git reset --soft/--keep, switch, branch, tag, update-ref, stash store, as each needs",
        cases: &[
            (
                "Pushes and fetches?",
                "Not undone: they changed another repository too. Undoing a pushed commit locally leaves the remote with it.",
            ),
            (
                "Undo refused?",
                "The repository moved on since (another commit, a switch outside egit): it says what, and won't guess.",
            ),
            (
                "Uncommitted changes when undoing a merge?",
                "egit moves back keeping them (reset --keep); where they'd be overwritten, it refuses instead.",
            ),
            (
                "What git did on the command line?",
                "Only egit's own actions are undone; for the rest, the reflog has every move.",
            ),
        ],
    },
    Topic {
        id: "redo",
        group: Group::Egit,
        title: "Redo",
        on: &["Redo"],
        what: "Does again what Undo took back, checking first that nothing moved since. Doing \
               anything new forgets what could be redone.",
        git: "the same commands as the action itself",
        cases: &[(
            "Redo refused?",
            "Something changed since the undo: redoing would overwrite it.",
        )],
    },
    Topic {
        id: "repository-menu",
        group: Group::Egit,
        title: "Repository",
        on: &["Repository"],
        what: "The repository itself: its Overview (where it lives, its branch, remotes, who \
               you are here), its Settings (every configuration value, and where each comes \
               from), its submodules and Git LFS pages, and reading it all again.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "read-again",
        group: Group::Egit,
        title: "Read it again",
        on: &["Read it again"],
        what: "Reads everything from git again: the status, the configuration, the history, \
               stashes, worktrees, submodules. Rarely needed: egit watches the repository and \
               follows changes made anywhere.",
        git: "git status, git config --list, git rev-list, …",
        cases: &[(
            "When is it needed?",
            "When the status bar says outside changes aren't followed (too many watches in use), on some network drives, or after editing files under .git by hand.",
        )],
    },
    Topic {
        id: "palette",
        group: Group::Egit,
        title: "Commands",
        on: &["Commands", "Every command"],
        what: "The command palette (⌘K or ⇧⌘P): every command, a switch to each branch, and \
               the recent repositories, found by typing a few letters of their words in \
               order. A command that can't run now says why. ↑ and ↓ choose, Enter runs.",
        git: "",
        cases: &[(
            "Typed letters find nothing?",
            "They must come in order: “pll” finds Pull, “llp” doesn't.",
        )],
    },
    Topic {
        id: "tabs",
        group: Group::Egit,
        title: "Tabs",
        on: &[
            "Close the tab",
            "Close the other tabs",
            "Open or clone a repository",
        ],
        what: "Each open repository has a tab of its own beside Home, Profile, and Preferences. \
               A middle click or ⌘W closes one (Home stays), its menu closes the others, copies \
               a repository's path, or opens its folder, and + goes Home to open, clone, or \
               create another. Tabs drag beside each other to show two at once; ⌘1 … ⌘9 bring \
               one forward.",
        git: "",
        cases: &[(
            "Does closing a tab stop what it was doing?",
            "Yes: a task under way (a fetch, a push) is cancelled as Cancel would, and so are \
             downloads. The tabs open when egit closes open again next time.",
        )],
    },
    Topic {
        id: "help-window",
        group: Group::Egit,
        title: "Help",
        on: &["Help"],
        what: "What everything does, at git's level, and the surprises it can bring: hold Alt \
               and point at anything to see its topic beside it, or press F1 for every topic, \
               searchable by any word.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "preferences-button",
        group: Group::Egit,
        title: "Preferences",
        on: &["Preferences"],
        what: "egit's own preferences (⌘,): text size, commit dates, the background fetch, the \
               sidebar, how diffs and files first show, every shortcut. They are egit's alone: \
               git's settings are in your Profile and in each repository's Settings.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "system-theme",
        group: Group::Egit,
        title: "System theme",
        on: &["System theme"],
        what: "Follows your desktop's light or dark appearance, changing when it does; Light \
               and Dark keep one whatever the desktop says.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "sidebar-shown",
        group: Group::Egit,
        title: "The sidebar",
        on: &["Hide the sidebar", "Show the sidebar"],
        what: "The panel at the left listing branches, remotes, requests, tags, stashes, \
               worktrees, and submodules. Hiding it leaves a strip to show it again; ⌘B does \
               either. Preferences say whether it shows when a repository opens.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "cancel",
        group: Group::Egit,
        title: "Cancel",
        on: &["Cancel"],
        what: "In a dialog or an editor: closes it, nothing done. While a task runs: stops git \
               the way git expects to be stopped (it removes its lock files), never killed \
               mid-write. In git's question for a password: refuses to answer, and the \
               command fails.",
        git: "SIGTERM to git's process group (on Windows, as Git for Windows stops a process)",
        cases: &[
            (
                "Cancelled a fetch or a pull part-way?",
                "Nothing is half-done: branches move only once everything arrived. What was downloaded may stay as loose data git cleans later.",
            ),
            (
                "Cancelled a push?",
                "The remote moved its branch or it didn't: fetch to see which.",
            ),
            (
                "Cancelled a clone?",
                "git removes the folder it was making.",
            ),
            (
                "Cancelled during a rebase or merge?",
                "If git had started, the operation stays in progress: the banner offers Continue or Abort.",
            ),
        ],
    },
    Topic {
        id: "dismiss",
        group: Group::Egit,
        title: "Dismiss",
        on: &["Dismiss"],
        what: "Puts away a message: a task's outcome, a problem, a notice, a banner. Nothing is \
               undone and nothing in the repository changes; a banner comes back if what it \
               says changes.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "ok",
        group: Group::Egit,
        title: "Answering a prompt",
        on: &["OK"],
        what: "Hands git (or ssh, or a signing program) what you typed: a user name, a \
               password or token, a key's passphrase. egit passes it straight on and wipes it; \
               it never keeps it. Whether git remembers it is up to your credential helper \
               (Profile, Credentials).",
        git: "GIT_ASKPASS and SSH_ASKPASS, answered by egit",
        cases: &[
            (
                "Asked again and again?",
                "No credential helper keeps it: set one (Profile), or use an SSH key with an agent.",
            ),
            (
                "GitHub refuses a password?",
                "It takes a personal access token in the password's place, or an SSH key.",
            ),
        ],
    },
    Topic {
        id: "yes-no",
        group: Group::Egit,
        title: "Yes and No",
        on: &["Yes", "No", "Trust and connect", "Don't connect"],
        what: "Answers git's or ssh's question. Most often ssh's, connecting to a server for \
               the first time: trust this host? Compare the fingerprint with the one the host \
               publishes (GitHub and GitLab list theirs) before Trust and connect: ssh then \
               remembers it in ~/.ssh/known_hosts. Don't connect stops the command.",
        git: "",
        cases: &[(
            "ssh refuses: the host key changed?",
            "It doesn't ask then. The server was reinstalled, or someone is in between: check with the host, then remove the old key (ssh-keygen -R <host>).",
        )],
    },
    Topic {
        id: "more",
        group: Group::Conflicts,
        title: "More",
        on: &["More"],
        what: "The merge editor's other choices: both sides with the incoming first, reopening \
               a conflict already chosen, taking every conflict's current or incoming side at \
               once, and whether both sides show whole above the merged file.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "copy",
        group: Group::Egit,
        title: "Copy",
        on: &["Copy"],
        what: "Puts a name, a subject, a path, or a link on the clipboard. Nothing in the \
               repository changes.",
        git: "",
        cases: &[(
            "A link to the forge?",
            "Offered where a remote is on GitHub or GitLab; it points at what the remote has, so push first.",
        )],
    },
    Topic {
        id: "add",
        group: Group::Egit,
        title: "Add",
        on: &["Add"],
        what: "In your Profile: adds a GitHub Enterprise or self-managed GitLab host, so egit \
               knows its remotes are on a forge and can show their pull or merge requests. \
               Kept in your global git configuration.",
        git: "git config --global --add egit.githubHost (or egit.gitlabHost)",
        cases: &[],
    },
    Topic {
        id: "create",
        group: Group::Egit,
        title: "Create",
        on: &["Create"],
        what: "Makes the branch named, at the commit the dialog says, switching to it if asked. \
               Only a name is written: no commit is copied, and nothing is pushed.",
        git: "git branch <name> <commit> (git switch -c to switch too)",
        cases: &[(
            "Disabled?",
            "The name isn't one git takes (spaces, “..”, a trailing “.lock”) or a branch has it already: the dialog says which.",
        )],
    },
    Topic {
        id: "delete",
        group: Group::Egit,
        title: "Delete",
        on: &["Delete"],
        what: "Deletes what the dialog names: a branch or tag here (Undo brings it back), or a \
               branch on a remote, which goes for everyone using it. The commits stay: only \
               the name pointing at them goes.",
        git: "git branch --delete, git tag --delete, git push --delete",
        cases: &[(
            "Deleted a remote's branch by mistake?",
            "Push it again from a branch that still has its commits: anyone's clone, or yours.",
        )],
    },
    Topic {
        id: "remove",
        group: Group::Egit,
        title: "Remove",
        on: &["Remove", "Remove…"],
        what: "Removes what the dialog names. A remote: its remote-tracking branches go here, \
               the remote itself is untouched. A worktree: its folder is deleted, its branch \
               and commits stay. A host in your Profile: egit stops treating it as a forge.",
        git: "git remote remove, git worktree remove, git config --unset",
        cases: &[],
    },
    Topic {
        id: "open",
        group: Group::Egit,
        title: "Open",
        on: &["Open"],
        what: "Opens the folder typed as a repository: any folder inside one finds its top, \
               as git does. It opens in a tab of its own, kept for next time.",
        git: "git rev-parse --show-toplevel",
        cases: &[
            (
                "Not a git repository?",
                "Neither the folder nor any above it holds a .git: clone it, or git init it.",
            ),
            (
                "Refused as unsafe?",
                "Another user owns the folder: git refuses until it is trusted (safe.directory), which egit offers.",
            ),
        ],
    },
    Topic {
        id: "save",
        group: Group::Egit,
        title: "Save",
        on: &["Save"],
        what: "Writes what was changed: in your Profile, into your global git configuration \
               (~/.gitconfig), where every repository reads it; in a remote's dialog, into the \
               repository's own .git/config.",
        git: "git config --global (or --local)",
        cases: &[(
            "A repository ignores my setting?",
            "Its own configuration sets it too, and wins: its Settings show where each value comes from.",
        )],
    },
    Topic {
        id: "refresh",
        group: Group::Egit,
        title: "Refresh",
        on: &["Refresh", "Try again"],
        what: "Reads again what is shown: the open pull or merge requests from the forge, or \
               the repository's overview and settings from git.",
        git: "",
        cases: &[(
            "Rate limited?",
            "The forge counts requests per hour: egit says when it lets you read again.",
        )],
    },
    Topic {
        id: "name",
        group: Group::Inside,
        title: "Name",
        on: &["Name"],
        what: "A submodule's name: its key in .gitmodules (submodule.<name>.path, .url, \
               .branch). Usually its path, but not always, and it stays the same when the \
               submodule moves.",
        git: "git config --file .gitmodules",
        cases: &[],
    },
    Topic {
        id: "address",
        group: Group::Remotes,
        title: "Address",
        on: &["Address"],
        what: "Where a repository is: an https URL, an ssh one (git@host:owner/name.git), or a \
               folder. Cloning copies the repository there; a submodule's address is the one \
               .gitmodules records, a relative one (../lib.git) taken from where this \
               repository's origin is.",
        git: "",
        cases: &[(
            "https or ssh?",
            "https asks for a token (a credential helper keeps it); ssh uses your SSH key. Either reaches the same repository.",
        )],
    },
    Topic {
        id: "branch-word",
        group: Group::Branches,
        title: "Branch",
        on: &["Branch"],
        what: "In the overview: the branch checked out, and the one it pulls from and pushes to. \
               Inside a submodule: the branch its checkout is on, when it is on one; usually \
               none, as git leaves a submodule detached at the commit recorded. Adding one: the \
               branch it follows (Follows), whose latest commit Update to latest brings.",
        git: "git config submodule.<name>.branch",
        cases: &[(
            "Committing inside a submodule?",
            "Switch it to a branch first, or the commits belong to no branch there.",
        )],
    },
    Topic {
        id: "into",
        group: Group::Egit,
        title: "Into",
        on: &["Into"],
        what: "Cloning: the folder the clone goes inside, as a folder of its own. Opening a \
               pull or merge request: the branch it asks to be merged into, on the repository \
               it goes to.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "on-word",
        group: Group::Branches,
        title: "On",
        on: &["On"],
        what: "The branch checked out here: the one a commit adds to, and the one Pull and Push \
               work on.",
        git: "git symbolic-ref HEAD",
        cases: &[],
    },
    Topic {
        id: "named",
        group: Group::Remotes,
        title: "Named",
        on: &["Named"],
        what: "The new folder's name, taken from the address's last part as git takes it \
               (without .git) until you change it. The folder must not exist yet, or be empty.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "files-word",
        group: Group::Inside,
        title: "Files",
        on: &["Files"],
        what: "The files Git LFS keeps at the commit checked out: each one's size, and whether \
               it is here whole or only as a pointer waiting to be downloaded.",
        git: "git lfs ls-files --json",
        cases: &[],
    },
    Topic {
        id: "picked-commits",
        group: Group::Looking,
        title: "Selecting several commits",
        on: &[
            "Bulk actions on * commits",
            "Bulk actions",
            "Selected",
            "Copy their names",
            "Clear the selection",
        ],
        what: "Shift-click picks every commit from the one last clicked to this one; Ctrl-click \
               (⌘ on a Mac) adds one or takes it away; Shift with ↑ or ↓ stretches the range. \
               While several are picked, the details list them, and a right click on one offers \
               what can be done to them all: signing them, comparing two, saving or copying them \
               as patches or a changelog, copying their names (one commit's menu has signing \
               too, under Bulk actions). Picking changes nothing in the repository.",
        git: "",
        cases: &[
            (
                "Rows in between picked too?",
                "A range is every row between, as the graph lays them out, other branches' too: Ctrl-click takes one away.",
            ),
            (
                "Lost the selection?",
                "A plain click, or Escape, lets them go; reading the history again keeps those still in it.",
            ),
            (
                "Copy their names?",
                "Their full names, one a line, the newest first: ready for git cherry-pick or a script.",
            ),
        ],
    },
    Topic {
        id: "compare-two",
        group: Group::Looking,
        title: "Comparing two commits",
        on: &[
            "Compare these two",
            "Compare with *",
            "Comparing two commits",
            "Close the comparison",
            "Swap them",
            "From",
            "To",
        ],
        what: "With exactly two commits picked, the details show what changed from the older to \
               the newer: every file that differs between them, with the lines gained and lost, \
               each opening its diff. It is the difference between the two snapshots, whatever \
               lies between them; they needn't be on one branch. Swap compares the other way \
               round. A commit's, branch's, or tag's menu compares it with the current branch \
               (Compare with main): from HEAD to it, the two picked in the history.",
        git: "git diff-tree -r -M <from> <to>; git diff <from> <to> -- <file>",
        cases: &[
            (
                "Commits on two branches?",
                "Still their snapshots' difference: what one has that the other hasn't, either way.",
            ),
            (
                "Gone when I clicked a row?",
                "A plain click picks one commit again: Shift- or Ctrl-click the two back, and Compare.",
            ),
        ],
    },
    Topic {
        id: "export",
        group: Group::Looking,
        title: "Exporting commits",
        // Either way's "Copy to the clipboard" points to its own topic when Alt is held.
        on: &["Export", "Copy to the clipboard"],
        what: "Commits picked together, as patches or a changelog: their changes as a single \
               patch, separate patches (one each, as git mails them), each saved or copied, or \
               a changelog of their subjects copied. Nothing changes in the repository.",
        git: "git format-patch --no-walk; git diff; git log --no-walk",
        cases: &[],
    },
    Topic {
        id: "export-one-patch",
        group: Group::Looking,
        title: "As a single patch",
        on: &["As a single patch", "Save to a file…"],
        what: "Every picked commit's changes together, as one diff git apply takes (under a list \
               of the commits, which git apply passes over): from the oldest one's parent to the \
               newest when each follows the one before; otherwise each commit's changes merged \
               in turn onto those before it, the oldest first, as cherry-picking them would. It \
               holds no messages or authors: separate patches keep those.",
        git: "git diff --binary <base> <result>; git merge-tree --write-tree (commits apart)",
        cases: &[
            (
                "Refused as a conflict?",
                "A commit changes lines a commit left out between them changed first: pick that one too, or save separate patches.",
            ),
            (
                "Applying it?",
                "Patches, Apply from files, as changes; or git apply in a terminal.",
            ),
        ],
    },
    Topic {
        id: "export-patches",
        group: Group::Looking,
        title: "As separate patches",
        on: &["As separate patches", "Save to a folder…"],
        what: "One patch each, the oldest first, as git format-patch mails them: each commit's \
               message, author, and date with its changes, which git am applies as commits \
               again. Saved to a folder they are 0001-…, 0002-…; copied, one text holding them \
               all (a mailbox).",
        git: "git format-patch --no-walk=unsorted --stdin (-o <folder>, or --stdout)",
        cases: &[
            (
                "A merge picked?",
                "Left out, as git leaves merges: a patch holds one parent's changes.",
            ),
            (
                "Applying them elsewhere?",
                "Patches, Apply from files, as commits; or git am in a terminal.",
            ),
        ],
    },
    Topic {
        id: "export-changelog",
        group: Group::Looking,
        title: "Copy as a changelog",
        on: &["Copy as a changelog"],
        what: "A Markdown list of the picked commits' subjects, the oldest first, each with its \
               short name, linked to its page when the repository is on GitHub or GitLab: \
               ready for release notes or a pull request's description.",
        git: "git log --no-walk=unsorted --stdin --format=%H%x1f%P%x1f%s",
        cases: &[(
            "Merges missing?",
            "They are left out (Merge branch … says little in a changelog), unless only merges were picked.",
        )],
    },
    Topic {
        id: "stats",
        group: Group::Looking,
        title: "Statistics",
        on: &["Stats: * commits", "Stats: 1 commit"],
        what: "The history shown, in figures and charts, in place of the commit table: how many \
               commits and by whom, when they were made, the files at HEAD by language, and the \
               lines the newest commits changed. Every commit is read once, in the background \
               (through gitoxide on several threads); closed and opened again, the page shows \
               at once, and reads again only once the history has.",
        git: "each commit read (gitoxide, else git cat-file); git check-mailmap --stdin; git \
              ls-tree -r -l HEAD; git log --no-walk --shortstat (the newest 10,000)",
        cases: &[
            (
                "Only some branches counted?",
                "It counts what the history shows: with branches hidden or one shown alone, those it shows.",
            ),
            (
                "Someone counted twice?",
                "The same person under two emails: a .mailmap line joins them (Name <email> <old email>).",
            ),
            (
                "Lines not counted?",
                "Not in a partial clone: git would fetch every file's contents to count them.",
            ),
            (
                "A huge repository?",
                "Reading its commits takes a second or two for a million; the lines are counted for the newest 10,000 only, after the rest.",
            ),
        ],
    },
    Topic {
        id: "stats-commits",
        group: Group::Looking,
        title: "Commits each month",
        on: &["Commits each month"],
        what: "Every commit the history shows, by the month its author made it (the author's \
               date, which a rebase keeps; the committer's would be when it was last made \
               again). Rest the pointer on a month for its count.",
        git: "",
        cases: &[(
            "Commits in 1970, or in the future?",
            "Their authors' clocks said so (an import, a wrong clock): git keeps the date as given.",
        )],
    },
    Topic {
        id: "stats-calendar",
        group: Group::Looking,
        title: "Contributions",
        on: &["Contributions"],
        what: "The year up to the latest commit, a square a day and a column a week (Monday at \
               the top), darker the more commits that day had, by its authors' own clocks.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "stats-punch",
        group: Group::Looking,
        title: "When commits are made",
        on: &["When commits are made"],
        what: "Each weekday's hours, a circle each, larger the more commits were made then, as \
               their authors' clocks said: when people work, wherever they are.",
        git: "",
        cases: &[],
    },
    Topic {
        id: "stats-active",
        group: Group::Looking,
        title: "Authors active each month",
        on: &["Authors active each month"],
        what: "How many people made at least one commit each month, through the mailmap: how \
               the project's team grew and shrank.",
        git: "git check-mailmap --stdin",
        cases: &[],
    },
    Topic {
        id: "stats-authors",
        group: Group::Looking,
        title: "Most commits",
        on: &["Most commits"],
        what: "The people with the most commits, each with the years they made them, and (once \
               counted) the lines they added and removed in the newest commits. One person \
               under several names or emails is one, through the repository's .mailmap.",
        git: "git check-mailmap --stdin",
        cases: &[],
    },
    Topic {
        id: "stats-languages",
        group: Group::Looking,
        title: "Languages at HEAD",
        on: &["Languages at HEAD"],
        what: "The files the current commit holds, by language from their names (extensions, \
               and names such as Makefile), by size; by count in a partial clone, where sizes \
               aren't known. Submodules are left out.",
        git: "git ls-tree -r -z -l HEAD",
        cases: &[],
    },
    Topic {
        id: "stats-largest",
        group: Group::Looking,
        title: "Largest files at HEAD",
        on: &["Largest files at HEAD"],
        what: "The largest files the current commit holds, as git stores them (an LFS file's \
               pointer, not the file it stands for).",
        git: "git ls-tree -r -z -l HEAD",
        cases: &[],
    },
    Topic {
        id: "stats-lines",
        group: Group::Looking,
        title: "Lines changed",
        on: &["Lines changed"],
        what: "The lines the newest commits added (above) and removed (below) each month, each \
               commit against its first parent; merges add none. Counted after the rest, for \
               the newest 10,000 commits at most: each is diffed.",
        git: "git log --no-walk=unsorted --stdin --shortstat",
        cases: &[(
            "A commit of a million lines?",
            "Often generated or vendored files committed whole: How large commits are shows how rare that is.",
        )],
    },
    Topic {
        id: "stats-sizes",
        group: Group::Looking,
        title: "How large commits are",
        on: &["How large commits are"],
        what: "How many of the newest commits changed no line, under ten, under a hundred, and so \
               on: lines added and removed together.",
        git: "git log --no-walk=unsorted --stdin --shortstat",
        cases: &[],
    },
    Topic {
        id: "details-split",
        group: Group::Egit,
        title: "The details' two parts",
        on: &[],
        what: "The line between a commit's message and facts and its changed files: drag it down \
               for a long message, up for a commit of many files. It holds where it is put, for \
               every commit and the next time egit opens; a double click puts it back (the \
               message as tall as it needs, up to half the panel). What doesn't fit either side \
               scrolls there.",
        git: "",
        cases: &[],
    },
];
