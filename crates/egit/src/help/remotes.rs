//! Remotes, and the forges behind them: fetching, pulling, pushing, pull and merge requests, signing in.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    Topic {
        id: "fetch",
        group: Group::Remotes,
        title: "Fetching",
        on: &["Fetch", "Fetch every remote"],
        what: "Downloads every remote's new commits and moves your remote-tracking branches \
               (origin/main) to where the remotes' branches are now. It never changes your own \
               branches, the index, or your files: you see what came before taking it. It \
               prunes too: a remote-tracking branch whose branch was deleted on the remote goes.",
        git: "git fetch --progress --all --prune",
        cases: &[
            (
                "In the background?",
                "egit fetches every few minutes (Preferences), never asking for a password: a fetch that would have to ask fails quietly, and a warning sign beside Fetch says why.",
            ),
            (
                "Something new came?",
                "A line under the top bar says how many commits your branch's upstream gained, with Pull beside it.",
            ),
            (
                "A pruned branch was mine?",
                "Pruning only removes origin/x, the copy of the remote's branch. Your own branch x stays, with every commit on it.",
            ),
            (
                "One remote can't be reached?",
                "git still fetches from the others, then says which failed (a wrong address, the network, refused credentials).",
            ),
            (
                "Tags?",
                "git brings the tags that point into the commits it fetched; tags elsewhere stay on the remote.",
            ),
        ],
    },
    Topic {
        id: "fetch-remote",
        group: Group::Remotes,
        title: "Fetching one remote",
        on: &["Fetch *"],
        what: "Downloads only this remote's new commits and moves its remote-tracking branches \
               (origin/…). Nothing of your own changes. Its branches deleted on the remote stay \
               here as they were, unless you fetch with pruning.",
        git: "git fetch --progress -- <remote>",
        cases: &[
            (
                "Disabled?",
                "The remote isn't configured any more (only its remote-tracking branches are left): add it again, or delete them.",
            ),
            (
                "Asked for a password?",
                "This remote's address needs credentials: answer, or set up a credential helper so git remembers them.",
            ),
        ],
    },
    Topic {
        id: "fetch-prune",
        group: Group::Remotes,
        title: "Fetching and pruning",
        on: &["Fetch *, pruning branches gone there"],
        what: "Fetches this remote and removes its remote-tracking branches (origin/x) whose \
               branches no longer exist there, so the sidebar and the graph stop showing \
               branches others deleted long ago.",
        git: "git fetch --progress --prune -- <remote>",
        cases: &[
            (
                "Does it delete my branches?",
                "No: only the remote-tracking copies. A branch of yours that followed one says “gone” after: its upstream no longer exists.",
            ),
            (
                "Can I get a pruned one back?",
                "Its commits stay as long as something reaches them (your branch, a tag); otherwise the remote must still have them.",
            ),
        ],
    },
    Topic {
        id: "fetched-notice",
        group: Group::Remotes,
        title: "New commits fetched in the background",
        on: &[],
        what: "A background fetch found new commits on your branch's upstream: the line says how \
               many and where. Nothing changed in your branch or files yet. Pull brings them in, \
               Show them selects the upstream's newest commit in the graph, and the cross \
               dismisses the line until something new comes.",
        git: "git fetch --all --prune --no-auto-maintenance (in the background, never asking)",
        cases: &[
            (
                "It went away by itself?",
                "It shows only while your branch is behind its upstream, and goes once you start a task.",
            ),
            (
                "Several fetches came?",
                "Their commits add up into one line, rather than one line each.",
            ),
        ],
    },
    Topic {
        id: "background-fetch",
        group: Group::Remotes,
        title: "Fetching in the background",
        on: &[],
        what: "While a repository is open, egit fetches from its remotes on its own, as often \
               as Preferences says (every 5 minutes at first), so the ↑ ↓ counts and the graph \
               stay current. It never asks for a password, never takes the repository's lock \
               from your own git, and stops at once when you start a task.",
        git: "git fetch --progress --all --prune --no-auto-maintenance (GIT_TERMINAL_PROMPT=0, GCM_INTERACTIVE=never)",
        cases: &[
            (
                "A warning sign beside Fetch?",
                "The last background fetch failed: point at it to see why. Fetching yourself asks for what it needs, and once it goes through the sign goes (a pull or push of yours brings the next background fetch forward, to see).",
            ),
            (
                "Remotes that need a password every time?",
                "Background fetches can't answer: they fail quietly. A credential helper or an ssh agent lets them through, and so does an ssh key you unlocked for a fetch of your own: egit holds it while it runs.",
            ),
            (
                "\"Could not read from remote repository\" in the background?",
                "Most often an ssh key's passphrase nobody has typed since egit started: Fetch once, answer it, and egit holds the key from then on (unless core.sshCommand or GIT_SSH_COMMAND names an ssh of your own, which egit leaves alone).",
            ),
            (
                "Metered or slow connection?",
                "Set it to never in Preferences: Fetch still works when you press it.",
            ),
        ],
    },
    Topic {
        id: "pull",
        group: Group::Remotes,
        title: "Pulling",
        on: &["Pull", "Pull *"],
        what: "Fetches, then brings the upstream into the current branch: by a merge (git's \
               default), a rebase (pull.rebase), or only a fast-forward (pull.ff), as \
               configured. When your branch has nothing the upstream lacks, it simply moves \
               forward.",
        git: "git pull --progress (--rebase, --no-rebase, or --ff-only when chosen)",
        cases: &[
            (
                "The branches diverged?",
                "Both sides have commits of their own: egit asks whether to rebase or merge, and can remember the choice (pull.rebase).",
            ),
            (
                "Uncommitted changes in the way?",
                "git refuses when the pull would overwrite them, and egit offers to stash them, pull, and put them back. Changes to other files are kept as they are.",
            ),
            (
                "Conflicts?",
                "The merge or rebase the pull started stops: resolve them, then Continue, or Abort to go back to where you were.",
            ),
            (
                "Disabled?",
                "The branch has no upstream (it was never pushed, or HEAD is detached): push it, or set one.",
            ),
            (
                "Submodules show as moved after?",
                "The pull brought new commits recorded for them: update them (the banner offers to).",
            ),
            (
                "Can I undo a pull?",
                "Undo takes back what moved your branch (the merge or the rebase); the fetched commits stay as remote-tracking branches, which is harmless.",
            ),
        ],
    },
    Topic {
        id: "pull-stashing",
        group: Group::Remotes,
        title: "Pulling with changes in the way",
        on: &["Stash, pull, and put back", "Stash and pull"],
        what: "Your uncommitted changes touch files the pull changes (or a rebasing pull wants \
               the work tree clean), so git refused it. Stash, pull, and put back stashes them \
               (staged and not), pulls, and applies them again. Files you have that git doesn't \
               track, where the pull brings files of the same names, can't be put back over \
               them: Stash and pull stashes everything (untracked files too) and leaves it in \
               the stash list.",
        git: "git pull --autostash; or git stash push --include-untracked, then git pull",
        cases: &[
            (
                "My changes came back with conflicts?",
                "They met the pull's changes to the same lines: resolve the files marked Conflict and stage them. git keeps your changes in the stash list too, until you drop that stash.",
            ),
            (
                "What I staged is no longer staged?",
                "git puts stashed changes back into the work tree only: stage them again.",
            ),
            (
                "The pull stopped at conflicts of its own?",
                "Your changes wait while you resolve them: Continue or Abort puts them back.",
            ),
            (
                "Undo?",
                "Undo takes back the pull and keeps your changes, unless they touch files the pull changed (git says which: stash them first). After Stash and pull it also pops the stash it made, as long as it is still the latest.",
            ),
        ],
    },
    Topic {
        id: "pull-fast-forward",
        group: Group::Remotes,
        title: "Pulling another branch",
        on: &["Pull * (fast-forward only)"],
        what: "Moves a branch you aren't on forward to its upstream's latest commit, without \
               switching to it. git fetches straight into it, so only a fast-forward is \
               possible: nothing merges, nothing rebases, and your work tree isn't touched.",
        git: "git fetch --porcelain --no-prune <remote> refs/heads/<branch>:refs/heads/<branch>",
        cases: &[
            (
                "Refused?",
                "The branch has commits the upstream hasn't: switch to it and pull there, where a merge or a rebase can bring both together.",
            ),
            (
                "Checked out in another worktree?",
                "git won't move a branch another worktree has checked out: pull in that worktree.",
            ),
            (
                "Disabled?",
                "It has no upstream to pull from: set one, or push it first.",
            ),
        ],
    },
    Topic {
        id: "diverged",
        group: Group::Remotes,
        title: "Diverged branches",
        on: &[
            "Remember this for the repository (pull.rebase)",
            "Rebase",
            "Merge",
        ],
        what: "Your branch and its upstream each have commits the other lacks, and git won't \
               guess how to join them. Rebase replays your commits after theirs: one straight \
               line, your commits get new names. Merge keeps both lines and joins them in a \
               merge commit. Remembering writes pull.rebase into this repository's settings, so \
               the next pull doesn't ask.",
        git: "git pull --rebase or --no-rebase; git config --local pull.rebase true|false",
        cases: &[
            (
                "Which should I choose?",
                "Rebase when the commits are yours alone and unpushed; merge when others built on them, or the project wants merge commits.",
            ),
            (
                "Rebasing commits I already pushed?",
                "They get new names: pushing then needs a force push, and anyone who has the old ones must rebase too.",
            ),
            (
                "Change the remembered choice?",
                "It's pull.rebase in the repository's settings (Repository › Settings); remove it and egit asks again.",
            ),
            (
                "pull.ff is only?",
                "git refuses anything but a fast-forward, so a diverged branch always lands here.",
            ),
        ],
    },
    Topic {
        id: "push",
        group: Group::Remotes,
        title: "Pushing",
        on: &["Push", "Push *"],
        what: "Uploads the current branch's commits and moves its upstream (the remote's branch) \
               to them. Only that branch goes: no tags, no other branches. The remote refuses \
               when it has commits you don't, so nobody's work is overwritten. A branch without \
               an upstream asks where to go the first time.",
        git: "git push --porcelain --progress -- <remote> refs/heads/<branch>:refs/heads/<name>",
        cases: &[
            (
                "Rejected as behind?",
                "Someone pushed first: pull, then push. Force push only if you mean to replace their commits.",
            ),
            (
                "Rewrote pushed commits?",
                "Amending or rebasing gives them new names: only a force push replaces the old ones there.",
            ),
            (
                "Tags?",
                "Pushing a branch pushes no tags: push each tag from its menu.",
            ),
            (
                "Disabled?",
                "HEAD is detached (no branch to push), or the repository has no remote: add one.",
            ),
            (
                "LFS files?",
                "With Git LFS set up, git's pre-push hook uploads the large files the commits need first.",
            ),
            (
                "Undo a push?",
                "egit can't take back a push: the remote has the commits. Revert them and push, or force push an older state (with care).",
            ),
        ],
    },
    Topic {
        id: "push-first",
        group: Group::Remotes,
        title: "A branch's first push",
        on: &["Push *…"],
        what: "The branch has no upstream yet: choose the remote and the name it takes there \
               (its own name, usually). git creates that branch on the remote and records it \
               as the upstream, so later pushes and pulls go there without asking.",
        git: "git push --porcelain --progress --set-upstream -- <remote> refs/heads/<branch>:refs/heads/<name>",
        cases: &[
            (
                "A different name there?",
                "Fine: the upstream remembers it, and Push and Pull use it. The sidebar shows it under the remote with that name.",
            ),
            (
                "A branch of that name exists there already?",
                "If yours includes its commits it moves forward; otherwise the push is refused, as any push would be.",
            ),
            (
                "Which remote?",
                "Usually origin. Pushing to your fork and opening a pull request against the original is the fork workflow.",
            ),
        ],
    },
    Topic {
        id: "push-other",
        group: Group::Remotes,
        title: "Pushing another branch",
        on: &["Push * to */*"],
        what: "Pushes this branch, checked out or not, to its upstream: the remote's branch moves \
               to it. Your work tree isn't involved. As with any push, the remote refuses if it \
               has commits the branch lacks.",
        git: "git push --porcelain --progress -- <remote> refs/heads/<branch>:refs/heads/<name>",
        cases: &[(
            "Refused as behind?",
            "Pull it first (fast-forward only, from its menu) or switch to it to merge, then push again.",
        )],
    },
    Topic {
        id: "push-tag",
        group: Group::Remotes,
        title: "Pushing a tag",
        on: &["Push * to *"],
        what: "Uploads one tag (and the commits it needs) to the remote. Tags aren't pushed with \
               branches: each is pushed on its own. Once a tag is shared, people and machines \
               rely on it never moving.",
        git: "git push --porcelain --progress -- <remote> refs/tags/<tag>:refs/tags/<tag>",
        cases: &[
            (
                "Refused: already exists?",
                "The remote has a tag of that name at another commit. egit never replaces a tag there: pick a new name, or delete it on the remote first if you're sure nobody uses it.",
            ),
            (
                "Released the wrong commit?",
                "Make a new tag (v1.0.1) rather than moving v1.0: whoever fetched the old one keeps it.",
            ),
            (
                "Undo it?",
                "Pushes aren't undone: delete the tag on the remote by hand if it must go.",
            ),
        ],
    },
    Topic {
        id: "push-refused",
        group: Group::Remotes,
        title: "A refused push",
        on: &[],
        what: "The remote didn't move its branch, and said why. “fetch first”: it has commits \
               you haven't fetched. “non-fast-forward”: your branch and its have diverged (an \
               amend or a rebase). “stale info”: a force push found the remote moved since you \
               fetched. Anything else comes from the remote itself, often a protection rule.",
        git: "git push --porcelain (each ref's flag and reason)",
        cases: &[
            (
                "“protected branch” or “pre-receive hook declined”?",
                "The remote's rules forbid it (pushing to main directly, force pushing, unsigned commits): push a branch and open a pull or merge request.",
            ),
            (
                "Permission denied, or 403?",
                "Your credentials can read but not write here: a token without write scope, or no access to the repository.",
            ),
            (
                "Nothing at all was pushed?",
                "A refusal changes nothing on the remote; your commits stay here, ready for the next push.",
            ),
        ],
    },
    Topic {
        id: "force-push",
        group: Group::Remotes,
        title: "Force pushing",
        on: &["Force push…", "Force push"],
        what: "Replaces the remote's branch with yours: commits only the remote had are dropped \
               from it. egit forces with a lease: the push is refused if the remote's branch \
               isn't where you last fetched it, or if that commit was never in your branch, so \
               you don't destroy work you haven't seen.",
        git: "git push --porcelain --force-with-lease --force-if-includes",
        cases: &[
            (
                "Refused even with force?",
                "The lease failed: fetch, look at what came, then decide. Nothing was replaced.",
            ),
            (
                "Why the second check?",
                "egit fetches in the background, which keeps the lease's expected commit current even when you never looked at it. --force-if-includes also asks that your branch once held it.",
            ),
            (
                "Others had pulled the old commits?",
                "Their branches still have them: they must rebase onto the new ones, or reset to them.",
            ),
            (
                "Took back the wrong commits?",
                "The old commits stay in your reflog (and others' clones): reset to one and force push again.",
            ),
        ],
    },
    Topic {
        id: "delete-remote-branch",
        group: Group::Remotes,
        title: "Deleting a branch on a remote",
        on: &["Delete * on *…"],
        what: "Deletes the branch on the remote, for everyone who uses it, and its \
               remote-tracking copy here. Your own branches stay as they are, so a local branch \
               holding the same commits keeps them.",
        git: "git push --porcelain --progress -- <remote> :refs/heads/<branch>",
        cases: &[
            (
                "Undo it?",
                "Push a branch holding those commits under that name again. egit can't take a push back.",
            ),
            (
                "An open pull or merge request used it?",
                "Forges close requests whose branch is deleted, or mark them as having lost their source.",
            ),
            (
                "Refused?",
                "Remotes don't let you delete their default branch, and protection rules can forbid deleting others.",
            ),
            (
                "Others still have it?",
                "Their clones keep origin/x until they fetch with pruning.",
            ),
            (
                "It reappears after a fetch?",
                "Someone pushed it again, or another remote has it.",
            ),
        ],
    },
    Topic {
        id: "upstream",
        group: Group::Remotes,
        title: "Upstreams",
        on: &["Set upstream*", "Forget */*"],
        what: "A branch's upstream is the remote branch it follows: Pull pulls from it, Push \
               pushes to it, and ↑ ↓ count the commits your branch has that it lacks, and the \
               other way. The counts compare with the remote-tracking branch as of the last \
               fetch, not the remote live.",
        git: "git branch --set-upstream-to=<remote>/<branch> <branch>, or --unset-upstream",
        cases: &[
            (
                "It says “gone”?",
                "The remote's branch was deleted (and pruned here): push to make it again, or forget the upstream.",
            ),
            ("“not pushed yet”?", "No upstream: the first push sets one."),
            (
                "↑ 3 ↓ 2?",
                "Diverged: you have 3 commits it lacks, it has 2 you lack. Pull to join them (merge or rebase), then push.",
            ),
            (
                "Counts look stale?",
                "They're as of the last fetch: fetch to see the remote as it is now.",
            ),
            (
                "Detached at 1a2b3c4d?",
                "No branch is checked out, so there is no upstream: switch to a branch, or create one here.",
            ),
        ],
    },
    Topic {
        id: "remote",
        group: Group::Remotes,
        title: "Remotes",
        on: &[],
        what: "Other copies of the repository you fetch from and push to, each named (origin by \
               convention) with an address. Their branches show here as remote-tracking \
               branches (origin/main), moved only by fetching. A fork usually has two: origin, \
               your fork, and upstream, the original.",
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
            (
                "Listed but greyed?",
                "Its remote-tracking branches remain but the remote isn't configured any more: delete them, or add it again.",
            ),
        ],
    },
    Topic {
        id: "remote-add",
        group: Group::Remotes,
        title: "Adding a remote",
        on: &["Add a remote*", "Add remote", "Fetch from it now"],
        what: "Records another repository under a name, so you can fetch from it and push to \
               it: a colleague's fork, or the original yours was forked from. Nothing is \
               downloaded until you fetch; Fetch from it now does that at once, making its \
               remote-tracking branches.",
        git: "git remote add -- <name> <address>; git fetch --progress -- <name>",
        cases: &[
            (
                "What address?",
                "https://host/owner/repo.git (credentials by token), git@host:owner/repo.git (your ssh key), or a folder on this computer.",
            ),
            (
                "The name is refused?",
                "It must be one git takes (no spaces, no ..) and not used by another remote here.",
            ),
            (
                "Its fetch failed?",
                "The remote is added all the same: fix its address (Change its address…) or its credentials, then fetch.",
            ),
        ],
    },
    Topic {
        id: "remote-address",
        group: Group::Remotes,
        title: "A remote's address",
        on: &["Change its address…", "Copy its address"],
        what: "Where git fetches from, and where it pushes to (the same unless set apart). \
               Changing it keeps the remote's name, its remote-tracking branches, and the \
               branches following them: only where git connects changes. An empty push \
               address means pushes go where fetches come from.",
        git: "git remote set-url (--push) -- <name> <address>; git config --unset-all remote.<name>.pushurl",
        cases: &[
            (
                "Switching from https to ssh?",
                "Credentials change with it: ssh uses your key (and asks about the host the first time), https a token or password.",
            ),
            (
                "The repository moved or was renamed?",
                "Point the remote at the new address; forges usually redirect the old one for a while, but not forever.",
            ),
            (
                "It connects somewhere else than written?",
                "A url.<base>.insteadOf setting rewrites addresses as git reaches them.",
            ),
        ],
    },
    Topic {
        id: "remote-rename",
        group: Group::Remotes,
        title: "Renaming a remote",
        on: &[],
        what: "Gives the remote a new name. git renames its remote-tracking branches with it \
               (origin/main becomes upstream/main) and updates every branch following them, so \
               nothing loses its upstream.",
        git: "git remote rename -- <old> <new>",
        cases: &[(
            "Scripts or tools named the old one?",
            "Only this repository's settings change: anything outside that says origin must follow.",
        )],
    },
    Topic {
        id: "remote-remove",
        group: Group::Remotes,
        title: "Removing a remote",
        on: &[],
        what: "Forgets the remote here: its settings and its remote-tracking branches go, and \
               branches that followed them follow nothing after. The remote repository itself \
               is untouched, and so are your branches and their commits.",
        git: "git remote remove -- <name>",
        cases: &[
            (
                "Commits only it had?",
                "They were reachable only through its remote-tracking branches: the reflog keeps them for a while, the remote has them still.",
            ),
            (
                "Undo it?",
                "Add it again with the same name and address, then fetch.",
            ),
        ],
    },
    Topic {
        id: "remote-branch",
        group: Group::Remotes,
        title: "Remote-tracking branches",
        on: &[],
        what: "origin/main is this repository's copy of where main was on origin when you last \
               fetched. Only fetching moves it; you can't commit to it. Switching to it makes a \
               local branch of that name following it, which is where your commits go.",
        git: "refs/remotes/<remote>/<branch>, moved by git fetch",
        cases: &[
            (
                "Delete one?",
                "Its menu deletes the branch on the remote itself, for everyone. To only forget the copy, fetch with pruning after it is gone there.",
            ),
            (
                "Behind the real remote?",
                "It is as of the last fetch: fetch to bring it up to date.",
            ),
            (
                "Merging it?",
                "Merging origin/main brings what you fetched into your branch: a pull is a fetch, then exactly that.",
            ),
            (
                "Gone from the remote, still here?",
                "A fetch with prune (egit's Fetch prunes) removes it; a local branch following it then says “(gone)”.",
            ),
        ],
    },
    Topic {
        id: "cancel-task",
        group: Group::Remotes,
        title: "Cancelling a task",
        on: &[],
        what: "Stops the git command running: egit asks git to stop (SIGTERM to its process \
               group, never a kill), so git removes its lock files and leaves the repository \
               consistent. What already finished stays finished.",
        git: "SIGTERM to git's process group (on Windows, ExitProcess in each process)",
        cases: &[
            (
                "A fetch cancelled?",
                "Nothing of yours changed; remote-tracking branches that were already updated stay, the rest wait for the next fetch.",
            ),
            (
                "A push cancelled?",
                "If the remote had already received it, its branch may have moved: fetch to see.",
            ),
            (
                "A pull cancelled half-way?",
                "During its fetch nothing changed. During its merge or rebase, the operation may be left stopped: the banner offers Continue or Abort.",
            ),
            (
                "A clone cancelled?",
                "git removes the folder it was making.",
            ),
        ],
    },
    Topic {
        id: "clone",
        group: Group::Remotes,
        title: "Cloning",
        on: &["Clone a repository", "Clone", "Choose…"],
        what: "Copies a repository, its whole history, into a new folder: Into is the folder it \
               goes in, Named the folder git makes there (the address's last part, without \
               .git). The remote is recorded as origin and its default branch checked out. The \
               repository opens once git is done.",
        git: "git clone --progress -- <address> <folder> (run outside any repository)",
        cases: &[
            (
                "The folder exists?",
                "git refuses unless it is empty: choose another name.",
            ),
            (
                "Asked for a password or a host?",
                "Questions say “While cloning …”: a private repository needs a token (https) or your ssh key; a new ssh host asks to be trusted.",
            ),
            ("Cancel?", "git stops and removes what it had made."),
            (
                "A local folder as the address?",
                "Works: git copies or links its objects, and origin points at that folder.",
            ),
            (
                "Very large history?",
                "A partial clone downloads files' contents only when needed.",
            ),
        ],
    },
    Topic {
        id: "clone-there",
        group: Group::Remotes,
        title: "Cloning onto another computer",
        on: &["Onto", "This computer", "Another computer", "Computer"],
        what: "Clones onto another computer you reach over SSH (a dev box, a container running \
               sshd), as Home's \"Open there\" opens one: git there clones it, through the one \
               connection egit shares, and it opens in a tab once done. Into is a folder there, \
               typed: ~ is the home folder there, and a relative path is inside it.",
        git: "ssh <computer> /bin/sh, then git clone --progress -- <address> <folder> there",
        cases: &[
            (
                "Which address?",
                "One the other computer reaches: its network, its ssh keys and agent (an agent forwarded from here, ForwardAgent), its credential helpers. A path is a folder there.",
            ),
            (
                "Asked for a password there?",
                "egit can't answer git's own prompts there yet: forward your SSH agent to it, or set up a credential helper there.",
            ),
            ("Cancel?", "git there stops and removes what it had made."),
        ],
    },
    Topic {
        id: "clone-submodules",
        group: Group::Remotes,
        title: "Cloning with submodules",
        on: &["Its submodules too"],
        what: "Clones every submodule too, checked out at the commit the repository records for \
               it, and theirs in turn. Without it, submodules' folders stay empty until you set \
               them up (the banner offers to).",
        git: "git clone --recurse-submodules (with --also-filter-submodules for a partial clone)",
        cases: &[
            (
                "A submodule failed?",
                "The main repository is cloned all the same: a private submodule may need its own credentials. Mend everything retries.",
            ),
            (
                "Relative addresses (../lib.git)?",
                "They resolve against the address you cloned from, as git resolves them.",
            ),
        ],
    },
    Topic {
        id: "clone-partial",
        group: Group::Remotes,
        title: "Partial clones",
        on: &["Partial clone: files' contents are fetched when first needed"],
        what: "Downloads every commit and folder listing but no file contents, except those the \
               checkout needs. Older contents are fetched from the remote the first time \
               something asks for them: a diff of an old commit, a blame, a checkout of an old \
               branch. Much faster for a large history.",
        git: "git clone --filter=blob:none",
        cases: &[
            (
                "Offline?",
                "Anything needing a file's content not downloaded yet fails until the remote is reachable.",
            ),
            (
                "Slower afterwards?",
                "Blaming or searching old history fetches as it goes; egit doesn't count changed lines in a partial clone, which would fetch everything.",
            ),
            (
                "Make it whole later?",
                "It stays partial; git keeps fetching what's missing as it is asked for.",
            ),
        ],
    },
    Topic {
        id: "credentials",
        group: Group::Remotes,
        title: "Passwords and keys",
        on: &["Credentials"],
        what: "When git or ssh needs a password, a passphrase, or a decision about a host, egit \
               shows the question and hands your answer back, wiping it after. Credential \
               helpers (credential.helper) remember passwords and tokens so git asks once; the \
               list shows each, and where it is set. Without one, git asks every time.",
        git: "GIT_ASKPASS, SSH_ASKPASS; git credential fill, approve, reject",
        cases: &[
            (
                "It never asks?",
                "Background fetches never ask: a fetch that would ask fails quietly instead.",
            ),
            (
                "A wrong password was remembered?",
                "git tells the helper to forget a credential the server refuses; otherwise remove it in the helper (the system's keychain, or ~/.git-credentials for store).",
            ),
            (
                "Which helper?",
                "The system's keychain (osxkeychain, Git Credential Manager, libsecret) over store, which keeps them unencrypted in a file.",
            ),
            (
                "ssh keys?",
                "ssh doesn't use credential helpers. Once a key's passphrase is typed, egit adds the key to an agent (yours, or one of its own while it runs), so it isn't asked again.",
            ),
        ],
    },
    Topic {
        id: "credential-keepers",
        group: Group::Remotes,
        title: "Keeping sign-ins",
        on: &[
            "The system keychain (libsecret)",
            "The macOS keychain",
            "Git Credential Manager",
            "Windows' Credential Manager",
            "In memory, for a day (cache)",
            "In a file, unencrypted (store)",
        ],
        what: "Where no credential helper is set up, a sign-in to GitHub or GitLab lasts until \
               egit quits, and git asks for HTTPS passwords every time. egit offers the helpers \
               installed with git: the one chosen is added to your global configuration \
               (credential.helper), and is handed the run's sign-ins at once, so egit stays \
               signed in after it quits and git finds them for the hosts' HTTPS remotes too.",
        git: "git config --global --add credential.helper <helper>; git credential approve",
        cases: &[
            (
                "Which one?",
                "A keychain (libsecret, the macOS keychain, Windows' Credential Manager, Git Credential Manager) keeps them encrypted, unlocked as you log in. cache holds them in memory for a day; store writes them as plain text to ~/.git-credentials.",
            ),
            (
                "libsecret isn't offered?",
                "Its helper comes with git on some distributions and apart on others (Debian and Ubuntu ship its source: build it from /usr/share/doc/git/contrib/credential/libsecret).",
            ),
            (
                "Set up, but it didn't give the token back?",
                "The helper took it and couldn't find it again (a keyring locked, or none running): egit stays signed in until it quits. Choose another.",
            ),
        ],
    },
    Topic {
        id: "prompt-user",
        group: Group::Remotes,
        title: "A user name is needed",
        on: &[],
        what: "git is connecting over https and the server wants to know who you are. Type your \
               user name on that host; the password or token comes next. The line under the \
               title says what git was doing when it asked.",
        git: "GIT_ASKPASS (Username for 'https://host':)",
        cases: &[
            (
                "Using a token?",
                "Many hosts take any user name (or your own) with the token as the password.",
            ),
            (
                "Cancel?",
                "git gets no answer and the command fails; nothing else happens.",
            ),
        ],
    },
    Topic {
        id: "prompt-secret",
        group: Group::Remotes,
        title: "A password or passphrase is needed",
        on: &[],
        what: "git, ssh, or a signing program needs a secret: a password or token for an https \
               remote, the passphrase of an ssh key, or a signing key's. It is hidden as you \
               type, handed to the asker, then wiped from egit's memory.",
        git: "GIT_ASKPASS, SSH_ASKPASS",
        cases: &[
            (
                "My account password is refused?",
                "GitHub and many hosts refuse account passwords over https: create a personal access token and give that.",
            ),
            (
                "Asked for the same passphrase again and again?",
                "egit asks once a connection: GIT_SSH_COMMAND, or core.sshCommand, names an ssh of its own, which egit leaves as it is (see Sharing ssh connections). Or add the key to an ssh agent (ssh-add): it stays unlocked for the session.",
            ),
            (
                "Cancel?",
                "git fails with “could not read Password”; nothing else changes.",
            ),
        ],
    },
    Topic {
        id: "prompt-host-key",
        group: Group::Remotes,
        title: "Trusting an ssh host",
        on: &[],
        what: "ssh is connecting to a host it has never seen and shows the host key's \
               fingerprint. Yes trusts it: ssh records it in ~/.ssh/known_hosts and won't ask \
               again. No refuses, and the command fails.",
        git: "SSH_ASKPASS (yes/no/[fingerprint])",
        cases: &[
            (
                "How do I know it's right?",
                "Compare the fingerprint with the one the host publishes (GitHub and GitLab list theirs in their documentation).",
            ),
            (
                "“REMOTE HOST IDENTIFICATION HAS CHANGED”?",
                "ssh refuses without asking: the key differs from the one recorded. If the host really changed it, remove the old one (ssh-keygen -R host) and connect again.",
            ),
        ],
    },
    Topic {
        id: "prompt-confirm",
        group: Group::Remotes,
        title: "Allowing the use of a key",
        on: &["Allow", "Deny"],
        what: "An ssh agent holds a key added to need confirmation each time it is used \
               (ssh-add -c). Allow lets this one use go ahead; Deny refuses it, and the command \
               fails.",
        git: "SSH_ASKPASS (confirm)",
        cases: &[(
            "Didn't expect it?",
            "Deny: something used your key without you starting it.",
        )],
    },
    Topic {
        id: "prompt-notice",
        group: Group::Remotes,
        title: "Waiting for your security key",
        on: &[],
        what: "ssh is using a hardware security key (FIDO) and waits for you to touch it. \
               Nothing to type: the notice goes once the key answers. Dismiss gives up, and the \
               command fails.",
        git: "SSH_ASKPASS (notice)",
        cases: &[(
            "Nothing happens when I touch it?",
            "Check that it is plugged in and is the key the remote knows; some keys also want a PIN first.",
        )],
    },
    Topic {
        id: "pull-push-settings",
        group: Group::Remotes,
        title: "Pull and push settings",
        on: &["Pull and push"],
        what: "How git pulls and pushes for you, from your settings: whether a pull merges or \
               rebases (pull.rebase), when it may fast-forward (pull.ff), whether a new branch's \
               first push sets up tracking (push.autoSetupRemote), what a bare git push pushes \
               (push.default), and a new repository's first branch (init.defaultBranch). Each \
               says where it is set.",
        git: "git config: pull.rebase, pull.ff, push.autoSetupRemote, push.default, init.defaultBranch",
        cases: &[
            (
                "Does egit follow push.default?",
                "egit always names the branch it pushes, so push.default matters only to git on the command line.",
            ),
            (
                "Set differently in one repository?",
                "A repository's own settings win over yours: its Settings page shows which applies.",
            ),
        ],
    },
    Topic {
        id: "forge-overview",
        group: Group::Remotes,
        title: "A repository on GitHub or GitLab",
        on: &[
            "Recognized as",
            "Account",
            "Pipelines",
            "In this history",
            "Read again",
            "Open Profile",
            "The ssh host of *",
        ],
        what: "The repository's Overview says what egit made of each remote: its address as git \
               reaches it (insteadOf applied), the GitHub repository or GitLab project it is, or \
               nothing when its host isn't github.com, gitlab.com, or one Profile lists; the \
               account the requests are read with; how many are open, or why none could be \
               read; and on GitLab the pipelines read (each project's newest 500), the newest \
               of them, and how many of the commits they ran for this history shows. Read again \
               reads the requests and pipelines again.",
        git: "git ls-remote --get-url <remote>; GitLab's GraphQL API: project { pipelines }",
        cases: &[
            (
                "Recognized as nothing?",
                "Its host isn't one egit knows: list your GitHub Enterprise or GitLab host in Profile (the name its web pages are on).",
            ),
            (
                "An ssh remote on another name (git-ssh.example.com:2222)?",
                "A self-managed GitLab's ssh is often on a name of its own (its gitlab_shell_ssh_host). The Overview offers it as the ssh host of each host Profile lists: choose the one whose web pages hold the project. It is kept as egit.<host>.sshHost in your git settings; Profile lists it under its host, and removes it.",
            ),
            (
                "Pipelines read, but no marks?",
                "In this history says how many of their commits the history shows: none means they ran for commits not fetched here, or for commits GitLab made itself (a merge request's merged results).",
            ),
            (
                "GitLab lists none to this account?",
                "GitLab gives no pipelines at all when CI/CD is turned off for the project, or when your role in it may not see them (a Guest on a private project).",
            ),
            (
                "More detail?",
                "Start egit with EGIT_LOG=egit_forge=debug,egit_app=debug: every request to the forge (its address, status, size, time; never the token) and what each remote was taken for is logged to standard error.",
            ),
        ],
    },
    Topic {
        id: "forge-hosts",
        group: Group::Remotes,
        title: "GitHub and GitLab hosts",
        on: &["GitHub and GitLab"],
        what: "egit reads pull requests from remotes on github.com and merge requests from \
               gitlab.com, and from the GitHub Enterprise and self-managed GitLab hosts listed \
               here. A remote whose address points at one of them (https, ssh, or an ssh alias) \
               is recognized as its repository.",
        git: "git config --global --add egit.githubHost / egit.gitlabHost <host[:port]>",
        cases: &[
            (
                "My company's host isn't recognized?",
                "Add it here, with its port if it isn't the usual one. https only, but for a host on this computer.",
            ),
            (
                "An ssh alias (Host work in ~/.ssh/config)?",
                "egit asks ssh what host it stands for, so the alias counts as the host.",
            ),
            (
                "Can't remove one?",
                "It is set outside your global settings (a repository's, or the system's): remove it there.",
            ),
        ],
    },
    Topic {
        id: "forge-sign-in",
        group: Group::Remotes,
        title: "Signing in to GitHub or GitLab",
        on: &[
            "Sign in",
            "Sign in…",
            "Sign in to *",
            "Create a token…",
            "Copy the link",
            "Token",
            "Sign in through git's credential helper instead",
            "Sign out",
            "Not now",
            "Done",
        ],
        what: "egit reads pull requests (GitHub), and merge requests and CI/CD pipelines \
               (GitLab), with a personal access token for the host. Wherever a remote's host \
               isn't signed in, egit offers to: a banner over the history, the top bar's Sign in \
               to GitHub (or GitLab), a row in the sidebar's requests, and Profile's accounts. \
               The dialog opens the host's page for a new token with the scope egit needs \
               ticked (Create a token…); pasted, the token is checked with the host, which says \
               whose it is, and git's credential helper keeps it under that login, where git \
               finds it for the host's HTTPS remotes too. egit keeps no credentials of its own. \
               Sign out (Profile) has the helper forget it.",
        git: "git credential approve, fill, reject (protocol https, the host, your login)",
        cases: &[
            (
                "Which scopes?",
                "GitHub: repo (or, for a fine-grained token, read access to pull requests and contents; write to open them). GitLab: api, or read_api to only read.",
            ),
            (
                "Not kept past this run?",
                "No credential helper is set up: egit stays signed in until it quits, in every repository open meanwhile. The dialog offers the helpers installed (Profile's Credentials too): choose one, and it keeps the token from then on.",
            ),
            (
                "My GitLab (or GitHub Enterprise) is on our own server?",
                "List its host in Profile, under GitHub and GitLab: only github.com, gitlab.com, and the hosts listed are asked for requests and pipelines.",
            ),
            (
                "Signed in to the host in another browser?",
                "Copy the link, beside Create a token…, and paste it into that browser: the token's page, with the scope egit needs ticked.",
            ),
            (
                "Git Credential Manager?",
                "Sign in through git's credential helper instead: it opens the host's own sign-in page in your browser, no token to make.",
            ),
            (
                "Refused?",
                "The host said the credentials don't work (expired, revoked, or too narrow): egit tells the helper to forget them, so the next sign-in asks again. A token mistyped in the dialog makes the helper forget nothing.",
            ),
            (
                "Not now?",
                "Hides the banner for that host from then on; the top bar's button and Profile still sign in.",
            ),
            (
                "Signing out and git?",
                "The helper forgets the token for every use of the host: git asks again the next time it fetches or pushes over HTTPS (ssh remotes are untouched).",
            ),
            (
                "An organization with single sign-on?",
                "Authorize the token for that organization on the host's token page, or its repositories stay hidden.",
            ),
            (
                "Rate limited?",
                "The host allows only so many requests an hour: egit says when it resets. Signing in raises the limit.",
            ),
        ],
    },
    Topic {
        id: "forge-network",
        group: Group::Remotes,
        title: "egit's own connections",
        on: &[],
        what: "Everything that moves commits is git's. egit itself connects only to GitHub's and \
               GitLab's APIs, to read and open requests: over https only, certificates checked \
               against the system's trust store, through the proxy the environment names \
               (HTTPS_PROXY, NO_PROXY), with no redirects, giving up after 10 seconds to connect \
               and 60 in all.",
        git: "",
        cases: &[
            (
                "Behind a company proxy or its own certificates?",
                "Set HTTPS_PROXY before starting egit, and install the company's certificate in the system's trust store: egit trusts what the system trusts.",
            ),
            (
                "git connects but egit doesn't?",
                "git follows its own http.* settings (http.proxy, http.sslCAInfo); egit follows the system's and the environment's.",
            ),
        ],
    },
    Topic {
        id: "forge-open",
        group: Group::Remotes,
        title: "Opening on the forge",
        on: &["Open on *"],
        what: "Opens the page of this commit, request, or repository on GitHub or GitLab in your \
               browser. The page is the remote's: it shows what was pushed there, not what is \
               only here.",
        git: "",
        cases: &[(
            "Page not found?",
            "The commit hasn't been pushed to that remote yet, or the repository is private and the browser isn't signed in.",
        )],
    },
    Topic {
        id: "forge-link",
        group: Group::Remotes,
        title: "Copying a link",
        on: &["Copy link to * on *"],
        what: "Copies the address of this commit or branch on its remote's forge, to paste into \
               a chat or an issue. The link points at the remote: whoever opens it sees what \
               was pushed there.",
        git: "",
        cases: &[(
            "The link shows nothing?",
            "The commit or branch isn't on that remote yet: push it first.",
        )],
    },
    Topic {
        id: "pull-requests",
        group: Group::Remotes,
        title: "Pull and merge requests",
        on: &[
            "Pull requests",
            "Merge requests",
            "Pull and merge requests",
            "1 pull request",
            "1 merge request",
            "1 pull and merge request",
            "* pull requests",
            "* merge requests",
            "* pull and merge requests",
            "Show the pull request",
            "Show the merge request",
            "Show pull request *",
            "Show merge request *",
        ],
        what: "A request asks a repository's maintainers to merge a branch into another: GitHub \
               calls it a pull request (#12), GitLab a merge request (!12). egit reads the open \
               ones of every remote on a forge, most recently updated first, with their checks \
               and reviews, and shows them in place of the history. They are read again after \
               each fetch or push.",
        git: "the forge's API; git fetch <remote> <its branch> to check one out",
        cases: &[
            (
                "Asked to sign in?",
                "Reading needs a token with read access (repo, read_api) for a private repository, and helps with rate limits on public ones.",
            ),
            (
                "“1,000 of 5,512 open”?",
                "egit reads the 1,000 most recently updated per repository; the rest are on the forge.",
            ),
            (
                "Merged on the forge, but my branch shows nothing?",
                "The forge merged on the remote: pull to bring the merge here.",
            ),
            (
                "One I just opened isn't listed?",
                "Refresh reads them again; the forge may take a moment.",
            ),
            (
                "Not offered?",
                "Only where a remote is on GitHub or GitLab, and, for a branch, once it is pushed.",
            ),
        ],
    },
    Topic {
        id: "pull-request-list",
        group: Group::Remotes,
        title: "A request's row",
        on: &[],
        what: "Each row: its number, its checks' state, its title (and a Draft chip), who opened \
               it, which branch it brings into which (and from which fork), its review decision, \
               and how long since it was last updated. Selecting one reads its details: size, \
               whether it merges cleanly, labels, every check, the latest reviews, and its \
               description.",
        git: "the forge's API",
        cases: &[(
            "“conflicts with its base”?",
            "The forge can't merge it as it is: its author must merge or rebase the base branch into it first (or you, after checking it out).",
        )],
    },
    Topic {
        id: "pull-request-checks",
        group: Group::Remotes,
        title: "A request's checks",
        on: &[],
        what: "What the forge's CI reported for the request's latest commit: passed, failed, \
               running, neither (skipped or neutral), or none at all. On GitLab these are the \
               head pipeline's jobs; a job allowed to fail counts as neutral. Details opens the \
               check's own page on the forge.",
        git: "the forge's API (GitHub's check runs and statuses, GitLab's pipeline jobs)",
        cases: &[
            (
                "Checks never run for a fork's request?",
                "Forges often wait for a maintainer to approve running them on a first-time contributor's commits.",
            ),
            (
                "Failed, but the code is fine?",
                "A flaky check: re-run it on the forge. egit only reads them.",
            ),
        ],
    },
    Topic {
        id: "pull-request-review",
        group: Group::Remotes,
        title: "Reviews",
        on: &[],
        what: "Where the request stands with reviewers: approved, changes requested, or a review \
               still required by the repository's rules. The details list each reviewer's \
               latest review: approved, asked for changes, commented, dismissed, or pending.",
        git: "the forge's API",
        cases: &[(
            "Approved, but still can't be merged?",
            "Other rules may apply: checks that must pass, a number of approvals, or a code owner's.",
        )],
    },
    Topic {
        id: "pull-request-badge",
        group: Group::Remotes,
        title: "Request badges",
        on: &[],
        what: "A badge on a commit's row (#12, !12) marks the head of an open request, in its \
               checks' colour; its title shows under the pointer. A click shows the request; \
               its menu opens it on the forge.",
        git: "",
        cases: &[(
            "A request without a badge?",
            "Its head commit isn't in this history: fetch, or check the request out.",
        )],
    },
    Topic {
        id: "pipeline-mark",
        group: Group::Remotes,
        title: "Pipeline marks",
        on: &["Show the pipeline"],
        what: "Beside a commit's name, the status of the latest GitLab CI/CD pipeline that ran \
               for it, by colour and by icon: green passed, red failed, blue under way (queued \
               or running), orange waiting for someone to start a manual job or for a delayed \
               one's time, grey cancelled or skipped. Its number and status show under the \
               pointer; a click shows its stages and jobs, its menu opens it on GitLab. egit \
               reads each GitLab project's newest 500 pipelines with your GitLab sign-in, again \
               after every fetch, pull, and push, and every 20 seconds while one shown is under \
               way.",
        git: "GitLab's GraphQL API: project { pipelines { sha status } } (nothing of git's own)",
        cases: &[
            (
                "A commit without a mark?",
                "No pipeline ran for it among each project's newest 500, or its project isn't a remote here, or you aren't signed in to its GitLab (the top bar offers to). A GitLab on your own server is asked only once Profile lists its host.",
            ),
            (
                "The pipelines couldn't be read?",
                "The top bar says so where GitLab answered with a problem (a token without the api or read_api scope, CI/CD turned off for the project, GitLab out of reach); the reason shows under the pointer.",
            ),
            (
                "Several pipelines for one commit?",
                "The mark is the latest one's: a branch pipeline and a merge request's for the same commit, or one run again, show as the newest of them.",
            ),
            (
                "GitHub?",
                "GitHub's Actions show as a pull request's checks, on its badge: egit reads pipelines from GitLab alone.",
            ),
            (
                "Passed with warnings?",
                "A job allowed to fail did: the pipeline counts as passed, and says so under the pointer.",
            ),
        ],
    },
    Topic {
        id: "pipeline-page",
        group: Group::Remotes,
        title: "A pipeline",
        on: &["Ran for", "Started", "Ran"],
        what: "A pipeline in the history's place: how it stands, the branch or tag it ran for, \
               its commit (Show the commit selects it in the history), who started it and how, \
               and how long it ran; then its stages in the order they run, each with its jobs. \
               It is read again every few seconds while it is under way. Escape, or History, \
               goes back.",
        git: "GitLab's GraphQL API: project { pipeline(iid) { stages { jobs } } }",
        cases: &[
            (
                "Its logs?",
                "A job opens its page on GitLab, in your browser: its log, artifacts, retrying it, and the rest stay GitLab's.",
            ),
            (
                "A job run again?",
                "Each job shows its latest run; the runs before it are on its page on GitLab.",
            ),
        ],
    },
    Topic {
        id: "pipeline-job",
        group: Group::Remotes,
        title: "A pipeline's jobs",
        on: &[],
        what: "A job of a pipeline's stage: its status (as the marks colour them), its name, \
               whether it may fail without failing the pipeline or waits to be started by hand, \
               and how long it ran. A click opens its page on GitLab, in your browser.",
        git: "",
        cases: &[
            (
                "Start a manual job?",
                "From its page on GitLab: egit only shows that it waits.",
            ),
            (
                "A trigger job?",
                "It starts another pipeline (a child's, or another project's): a click opens that one.",
            ),
        ],
    },
    Topic {
        id: "pull-request-check-out",
        group: Group::Remotes,
        title: "Checking a request out",
        on: &["Check out"],
        what: "Fetches the request's branch and switches to it, to run or review it here. From \
               the same repository: its remote-tracking branch is fetched and a local branch of \
               that name, following it, is checked out. From a fork: the forge's copy of its \
               head is fetched into pr/12 (GitHub) or mr/12 (GitLab), then checked out.",
        git: "git fetch --no-tags <remote> +refs/heads/<b>:refs/remotes/<remote>/<b> (or refs/pull/12/head:refs/heads/pr/12); git switch",
        cases: &[
            (
                "Uncommitted changes in the way?",
                "egit offers to stash them, then switch.",
            ),
            (
                "My local branch of that name is older?",
                "Switching doesn't move it: pull after, to bring it up to the request.",
            ),
            (
                "The fork's author force pushed, and pr/12 is refused?",
                "Delete pr/12 (after switching away), then check it out again.",
            ),
            (
                "Can I push to a fork's request?",
                "pr/12 follows nothing: pushing there needs the fork as a remote and its author's permission (“allow edits from maintainers”).",
            ),
        ],
    },
    Topic {
        id: "pull-request-create",
        group: Group::Remotes,
        title: "Opening a request",
        on: &[
            "Create pull request…",
            "Create merge request…",
            "Create pull request",
            "Create merge request",
        ],
        what: "Opens a pull or merge request from the branch as pushed to its remote: Into is \
               the repository it asks (a fork's parent, by default), Base branch the branch it \
               would merge into (the default branch). When the branch has one commit of its own, \
               its message fills the title and description.",
        git: "the forge's API (GitHub: POST /repos/…/pulls; GitLab: POST /projects/…/merge_requests)",
        cases: &[
            (
                "Disabled in the menu?",
                "Push the branch to a remote on GitHub or GitLab first: a request is made from what the remote has.",
            ),
            (
                "Commits made since the push?",
                "They aren't in it until you push them; pushing later adds them to the request.",
            ),
            (
                "Refused: one already exists?",
                "A branch can have one open request per base: Show it from the branch's menu.",
            ),
            (
                "Refused: no commits between?",
                "The branch has nothing the base lacks: commit, push, then open it.",
            ),
        ],
    },
    Topic {
        id: "pull-request-draft",
        group: Group::Remotes,
        title: "Drafts",
        on: &["Draft"],
        what: "A draft request is open for comments but marked not ready: it can't be merged \
               until marked ready on the forge, and reviewers aren't asked yet. On GitHub it is \
               a draft pull request; on GitLab the title starts with Draft:.",
        git: "the forge's API (draft: true; GitLab's Draft: title)",
        cases: &[(
            "Refused as a draft?",
            "GitHub allows drafts in private repositories only on paid plans: open it without.",
        )],
    },
    Topic {
        id: "pull-request-fork",
        group: Group::Remotes,
        title: "Requests from forks",
        on: &[],
        what: "A request whose branch lives in someone's fork rather than the repository \
               itself. Its branch isn't on your remotes, so the forge keeps a copy of its head \
               in the repository (refs/pull/12/head, refs/merge-requests/12/head), which egit \
               fetches to check it out.",
        git: "git fetch --no-tags <remote> refs/pull/12/head:refs/heads/pr/12",
        cases: &[(
            "“from a deleted fork”?",
            "The fork is gone, but the forge's copy of the request's head remains: it can still be checked out.",
        )],
    },
    Topic {
        id: "pull-request-target",
        group: Group::Remotes,
        title: "Where a request goes",
        on: &[],
        what: "When the branch's repository is a fork, a request can ask the original (its \
               parent) or the fork itself; the original is chosen first, as forks are usually \
               made to contribute back. The base branch is where it would be merged, the chosen \
               repository's default branch until you change it.",
        git: "the forge's API (the repository's default branch and parent)",
        cases: &[(
            "Opened against the wrong one?",
            "Close it on the forge and open another: a request can't move between repositories.",
        )],
    },
    Topic {
        id: "ssh-or-https",
        group: Group::Remotes,
        title: "ssh or https addresses",
        on: &[],
        what: "A remote's address decides how git connects and proves who you are. https \
               (https://host/owner/repo.git) uses a password or token, kept by a credential \
               helper. ssh (git@host:owner/repo.git, or ssh://…) uses your ssh key, unlocked by \
               an agent, and asks to trust each new host once.",
        git: "git remote set-url; ssh; git credential",
        cases: &[
            (
                "Port 22 is blocked?",
                "GitHub answers ssh on ssh.github.com port 443, GitLab on altssh.gitlab.com: egit recognizes both.",
            ),
            (
                "Permission denied (publickey)?",
                "The host doesn't know your key: add the public key to your account there, or check that the agent holds it.",
            ),
        ],
    },
    Topic {
        id: "ssh-sharing",
        group: Group::Remotes,
        title: "Holding ssh keys, and sharing connections",
        on: &[],
        what: "Each fetch, pull, and push over ssh would connect and prove who you are again, \
               asking a key's passphrase each time unless an agent holds it. egit's ssh adds a \
               key to an agent once you type its passphrase, as ssh-add would: your agent \
               (SSH_AUTH_SOCK) where one answers, else one of egit's own, for git alone, which \
               forgets the key and ends when egit does. Later commands, background fetches \
               too, are let in by it without asking. And one connection to each host is shared \
               while it is used (faster, and a password is asked once): it closes when egit \
               quits, after an hour unused, or within a minute of the network going away.",
        git: "GIT_SSH=<egit's wrapper>: ssh -o AddKeysToAgent=yes -o ControlMaster=auto -o ControlPersist=1h -o ServerAliveInterval=15 -o ControlPath=<a private folder>/%C (SSH_AUTH_SOCK=<egit's agent> where you have none)",
        cases: &[
            (
                "Turn it off?",
                "Start egit with GIT_SSH_COMMAND set, or set core.sshCommand (for you or one repository): git runs that ssh instead, as it would without egit.",
            ),
            (
                "A fetch failed after the computer slept?",
                "The shared connection may have gone with the network. It closes itself within a minute; the next command connects afresh, with the key held (a password is asked once more).",
            ),
            (
                "My own agent got my key?",
                "Yes, where egit found yours running: as ssh-add would have added it. ssh-add -d forgets it there; egit's own agent, used where you have none, forgets it when egit quits.",
            ),
            (
                "Windows?",
                "OpenSSH for Windows can't share connections: an ssh agent (the OpenSSH Authentication Agent service, or Pageant) keeps keys unlocked instead.",
            ),
        ],
    },
    Topic {
        id: "shallow-clone",
        group: Group::Remotes,
        title: "Shallow clones",
        on: &[],
        what: "A repository cloned with --depth has only its latest commits: the graph ends \
               where the history was cut, and that commit shows no parents though it has them \
               on the remote. egit never makes one, but opens them as git does.",
        git: "git clone --depth; git fetch --unshallow",
        cases: &[
            (
                "Blame or a merge goes wrong at the cut?",
                "git can't see past it: run git fetch --unshallow to download the rest.",
            ),
            (
                "Can I push from it?",
                "Usually: the remote has the history the clone lacks.",
            ),
        ],
    },
];
