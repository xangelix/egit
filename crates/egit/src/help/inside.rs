//! Repositories inside and beside one: submodules, Git LFS, worktrees; and egit's own pages:
//! preferences, the profile, Home, and a repository's overview and settings.

use super::{Group, Topic};

/// The topics.
pub(super) const TOPICS: &[Topic] = &[
    // Submodules.
    Topic {
        id: "submodules",
        group: Group::Inside,
        title: "Submodules",
        on: &["Submodules", "Submodules…", "Repositories inside this one"],
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
                "A fresh clone's submodules are all empty?",
                "git clone leaves them alone unless asked (egit's clone offers “Its submodules too”): Mend everything sets them up.",
            ),
            (
                "It's “at another commit” after a pull?",
                "The pull recorded a newer commit without moving it: update it to the commit recorded.",
            ),
            (
                "It's at another commit and I committed in it?",
                "Record its commit here (stage its path) and commit: that's how a submodule moves on.",
            ),
            (
                "Fetching it fails after its address changed?",
                "Sync addresses: the address set up here is out of date.",
            ),
            (
                "Forget it here, or remove it?",
                "Forgetting (deinit) only empties it here; removing deletes it from the repository (to commit).",
            ),
            (
                "Tired of updating them by hand?",
                "“Update them on every switch and pull” sets submodule.recurse: git moves them along.",
            ),
        ],
    },
    Topic {
        id: "submodule-states",
        group: Group::Inside,
        title: "Where submodules stand",
        on: &[
            "All set up, at the commits recorded",
            "* not set up*",
            "*behind the commit* recorded*",
            "*moved past the commit* recorded, to record*",
            "* in conflict",
            "* in conflict · *",
            "*address* out of date*",
        ],
        what: "The Submodules menu's first line counts them by where they stand, from git's \
               status and a look inside each: not set up (an empty folder), behind the commit \
               recorded (a pull or switch here recorded a newer one than it has checked out), \
               moved past it (someone committed or checked out inside it: work to record), in \
               conflict (a merge gave it two commits), or set up with an address .gitmodules no \
               longer gives.",
        git: "git submodule status --recursive; inside each, git rev-list --left-right --count",
        cases: &[
            (
                "Moved past isn't counted as trouble?",
                "It's usually your own work in it: record its commit here to keep it. Updating it would leave those commits only in its reflog.",
            ),
            (
                "Behind again after every pull?",
                "Turn on “Update them on every switch and pull” (submodule.recurse), or update them after pulling.",
            ),
            (
                "The counts look stale?",
                "They're read again whenever the work tree's status changes; Read it again (⌘R) reads everything.",
            ),
        ],
    },
    Topic {
        id: "submodule-mend",
        group: Group::Inside,
        title: "Mending submodules",
        on: &["Mend everything"],
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
            (
                "Offline?",
                "Submodules whose commits are already here are checked out; the others fail until the network is back.",
            ),
        ],
    },
    Topic {
        id: "submodule-update-recorded",
        group: Group::Inside,
        title: "Updating to the commits recorded",
        on: &[
            "Update all to the commits recorded",
            "Update to the commit recorded",
            "Update every submodule",
        ],
        what: "Checks out, in each submodule, the commit this repository records for it, \
               cloning those not set up yet, nested ones too: what a fresh checkout of this \
               commit should hold. Nothing in this repository changes; a submodule left on a \
               branch ends up detached at that commit.",
        git: "git submodule update --init --recursive --progress",
        cases: &[
            (
                "Commits I made inside it?",
                "If it was past the commit recorded, they're left behind (in its reflog, and on any branch you made there): record its commit here first to keep them.",
            ),
            (
                "Uncommitted changes inside it?",
                "git refuses for that submodule when checking out would overwrite them: commit or stash them inside it (open it in a tab).",
            ),
            (
                "The commit recorded can't be fetched?",
                "Someone recorded a commit they never pushed in the submodule: ask them to push it there.",
            ),
        ],
    },
    Topic {
        id: "submodule-update-latest",
        group: Group::Inside,
        title: "Updating to their branches' latest",
        on: &[
            "Update all to their branches' latest",
            "Update to its branch's latest",
        ],
        what: "Fetches each submodule's branch (the one it follows, or its remote's default) \
               and checks out its newest commit. This repository then sees each moved past the \
               commit it records: record them here and commit, so everyone gets the same \
               commits.",
        git: "git submodule update --init --recursive --remote --progress",
        cases: &[
            (
                "Nothing happens for one?",
                "Its branch has nothing newer than what it has checked out.",
            ),
            (
                "Which branch?",
                "submodule.<name>.branch in .gitmodules (“Follows” on its page); none means the remote's default branch.",
            ),
            (
                "Others still see the old commits?",
                "Until you record the new ones here, commit, and push, only your checkout moved.",
            ),
        ],
    },
    Topic {
        id: "submodule-update",
        group: Group::Inside,
        title: "Updating a submodule",
        on: &["Update *"],
        what: "Brings this submodule to the commit this repository records, setting it up \
               first if it isn't, and the submodules inside it along with it. From the \
               sidebar's menu, it's the submodule on that row.",
        git: "git submodule update --init --recursive --progress -- <path>",
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
        id: "submodule-set-up",
        group: Group::Inside,
        title: "Setting a submodule up",
        on: &["Set it up", "Set up *"],
        what: "Registers the submodule in this repository's configuration (its address from \
               .gitmodules), clones it into .git/modules (or reuses the clone already there), \
               and checks out the commit recorded in its folder.",
        git: "git submodule update --init --recursive --progress -- <path>",
        cases: &[
            (
                "It asks for a password?",
                "Its address needs credentials: egit relays git's question. A private repository you can't reach can't be set up.",
            ),
            (
                "Its folder isn't empty?",
                "git won't check out into a folder holding files: move them away first.",
            ),
            (
                "Set it up again after forgetting it?",
                "Quick: its clone stayed in .git/modules, so nothing is downloaded again but what's new.",
            ),
        ],
    },
    Topic {
        id: "submodule-sync",
        group: Group::Inside,
        title: "Syncing addresses",
        on: &["Sync addresses", "Sync its address", "Set up with"],
        what: "Copies each submodule's address from .gitmodules into this repository's \
               configuration and into the submodule's own remote, where git actually fetches \
               from. It's needed after .gitmodules changes an address: until then git keeps \
               fetching from the old one. “Set up with” shows the address set up here when it's \
               out of date.",
        git: "git submodule sync --recursive",
        cases: &[
            (
                "When is an address out of date?",
                "When .gitmodules (after a pull, say) gives another address than the one set up here. A relative one (../lib.git) is resolved against origin's address, as git does.",
            ),
            (
                "Does it fetch?",
                "No: it only changes where the next fetch goes. Update afterwards.",
            ),
            (
                "I pointed it at my own fork?",
                "Syncing puts .gitmodules' address back: for yourself alone, add your fork as another remote inside the submodule instead.",
            ),
        ],
    },
    Topic {
        id: "submodule-recurse",
        group: Group::Inside,
        title: "Updating submodules on every switch and pull",
        on: &["Update them on every switch and pull"],
        what: "Sets submodule.recurse in this repository's configuration: switching, checking \
               out, resetting, and pulling then move each submodule to the commit recorded as \
               they go, and fetching fetches in them too, so they don't fall behind. Off, they \
               stay where they are until updated.",
        git: "git config --local submodule.recurse true (unset when off)",
        cases: &[
            (
                "Will it throw away my work in a submodule?",
                "git refuses to move a submodule whose changes it would overwrite; commits you made in it are left behind (in its reflog) unless recorded here first.",
            ),
            (
                "Only for this repository?",
                "Yes: it's written to this repository's .git/config. Set it globally from a terminal to have it everywhere.",
            ),
            (
                "Switching got slower?",
                "Each switch now also checks out in every submodule, which takes as long as they need.",
            ),
        ],
    },
    Topic {
        id: "submodule-record",
        group: Group::Inside,
        title: "Recording a submodule's commit",
        on: &["Record its commit here"],
        what: "Stages the submodule's path: the commit it has checked out becomes the one this \
               repository records (its gitlink), to commit. That's how a submodule moves on for \
               everyone, after updating it to its branch's latest or committing inside it.",
        git: "git add -- <path>",
        cases: &[
            (
                "Did I push the submodule's commit?",
                "Push it inside the submodule first: others can't fetch a recorded commit that only exists on your machine.",
            ),
            (
                "Resolving a submodule's conflict?",
                "Check out the commit to keep inside it, then record it here: that resolves the conflict.",
            ),
            (
                "Changes not committed inside it?",
                "Only its commit is recorded: commit inside it first, or those changes stay out.",
            ),
        ],
    },
    Topic {
        id: "submodule-forget",
        group: Group::Inside,
        title: "Forgetting a submodule here",
        on: &["Forget it here"],
        what: "Empties the submodule's folder and removes its setup from this repository's \
               configuration (deinit). It stays recorded in the repository, for everyone else, \
               and its clone stays in .git/modules, so setting it up again is quick.",
        git: "git submodule deinit -- <path>",
        cases: &[
            (
                "Changes inside it?",
                "git refuses while it has changes not committed: commit, stash, or drop them inside it first.",
            ),
            (
                "Want it gone for everyone?",
                "Remove it instead: that's a change to commit.",
            ),
            (
                "It shows as not set up afterwards?",
                "That's what forgetting does: Set it up brings it back.",
            ),
        ],
    },
    Topic {
        id: "submodule-remove",
        group: Group::Inside,
        title: "Removing a submodule",
        on: &["Remove it"],
        what: "Deletes the submodule from this repository: its folder, its gitlink, and its \
               section of .gitmodules, all staged, to commit. Its clone stays in .git/modules, \
               so checking out an older commit that has it brings it back without cloning.",
        git: "git add .gitmodules, then git rm -- <path>",
        cases: &[
            (
                "Changes inside it?",
                "git refuses: commit or drop them inside it first.",
            ),
            (
                "Changed my mind?",
                "Before committing, unstage both changes and restore .gitmodules, then set it up again; after committing, revert that commit.",
            ),
            (
                "What happens in others' checkouts?",
                "When they pull the commit, git stops tracking it there; its folder may stay on their disk until they delete it.",
            ),
        ],
    },
    Topic {
        id: "submodule-add",
        group: Group::Inside,
        title: "Adding a submodule",
        on: &["Add a submodule…", "Add a submodule", "Add it"],
        what: "Clones another repository into a folder here and records it as a submodule at \
               the commit it checked out: a .gitmodules section (its address, and the branch it \
               follows when one is given) and a gitlink, both staged, to commit. The folder is \
               named from the address unless typed.",
        git: "git submodule add --progress [--branch <branch>] -- <address> <folder>",
        cases: &[
            (
                "A relative address (../lib.git)?",
                "It's resolved against this repository's origin: handy when both live on the same server, as everyone then reaches it their own way (SSH or HTTPS).",
            ),
            (
                "The folder already holds files?",
                "git refuses: pick a new or empty folder. A folder this repository already tracks can't become a submodule.",
            ),
            (
                "Others see an empty folder?",
                "Commit and push .gitmodules and the gitlink; they then set it up (Mend everything, or a clone with its submodules).",
            ),
        ],
    },
    Topic {
        id: "submodule-address",
        group: Group::Inside,
        title: "Changing a submodule's address",
        on: &["Change address"],
        what: "Points the submodule at another address, in .gitmodules (for everyone, once \
               committed) and in this repository's configuration (for the next fetch here).",
        git: "git submodule set-url -- <path> <address>",
        cases: &[
            (
                "The repository moved hosts?",
                "Change the address, commit .gitmodules, and push: others then sync addresses.",
            ),
            (
                "Only for me (a fork)?",
                "Add a remote inside the submodule instead (open it in a tab): .gitmodules stays as it is for everyone.",
            ),
        ],
    },
    Topic {
        id: "submodule-branch",
        group: Group::Inside,
        title: "The branch a submodule follows",
        on: &["Follow it", "Follows"],
        what: "The branch whose latest commit updating to the branches' latest fetches: \
               submodule.<name>.branch in .gitmodules, to commit. Empty means the remote's \
               default branch. Updating to the commits recorded ignores it.",
        git: "git submodule set-branch --branch <branch> -- <path> (--default when empty)",
        cases: &[
            (
                "Does it check that branch out?",
                "No: updating still leaves the submodule detached at that branch's latest commit, as submodules usually are.",
            ),
            (
                "A tag instead?",
                "Branches only: to pin a tag, check it out inside the submodule and record its commit here.",
            ),
        ],
    },
    Topic {
        id: "submodule-recorded",
        group: Group::Inside,
        title: "The commit recorded",
        on: &["Recorded"],
        what: "The commit this repository's current commit records for the submodule (its \
               gitlink): what anyone checking out this commit should have in it. Switching or \
               pulling here can change it without moving the submodule.",
        git: "git ls-files -s -- <path> (mode 160000)",
        cases: &[
            (
                "Recorded and checked out differ?",
                "Update it to the commit recorded, or record the one checked out, whichever is right.",
            ),
            (
                "Where does that commit live?",
                "In the submodule's own repository, which must fetch it: this repository holds only its name.",
            ),
        ],
    },
    Topic {
        id: "submodule-checked-out",
        group: Group::Inside,
        title: "The commit checked out",
        on: &["Checked out"],
        what: "The commit the submodule's own HEAD is at in its folder, with git describe's \
               name for it when there is one (a tag nearby). When it isn't the commit recorded, \
               the submodule is at another commit.",
        git: "git submodule status (inside it, git rev-parse HEAD)",
        cases: &[(
            "On a branch, or detached?",
            "“On” below says. Detached is normal for a submodule; checking out a branch inside it is how you work in it.",
        )],
    },
    Topic {
        id: "submodule-against",
        group: Group::Inside,
        title: "Past and behind the commit recorded",
        on: &["Against it"],
        what: "How the commit checked out compares with the commit recorded: commits it has that \
               the recorded one doesn't (past it: usually your work, to record), and commits the \
               recorded one has that it lacks (behind it: update it). Those it has beyond are \
               listed below, newest first.",
        git: "git rev-list --left-right --count HEAD...<recorded> (inside it)",
        cases: &[
            (
                "Both past and behind?",
                "They diverged: someone recorded other commits while you committed in it. Merge or rebase inside the submodule, then record the result.",
            ),
            (
                "“It doesn't have the commit recorded yet”?",
                "It must fetch that commit first: updating it does.",
            ),
        ],
    },
    Topic {
        id: "submodules-page",
        group: Group::Inside,
        title: "The submodules' page",
        on: &["Each one, and adding one…", "Each one"],
        what: "Every submodule (nested ones under theirs) with where it stands; a click shows one: \
               what its state means, its addresses, the branch it follows, the commits recorded \
               and checked out, and what can be done to it. The card above mends, updates, or \
               syncs them all at once.",
        git: "git submodule status --recursive; in the one selected, git rev-parse, rev-list, log",
        cases: &[
            (
                "A nested one offers little?",
                "Its parent records it: open the parent in a tab, or update the parent from here and it comes along.",
            ),
            ("Back to the history?", "History, or Escape."),
        ],
    },
    Topic {
        id: "open-in-a-tab",
        group: Group::Inside,
        title: "Opening it in a tab",
        on: &["Open in a tab"],
        what: "Opens a submodule or another worktree as a repository of its own, in a tab beside \
               this one: its history, branches, and changes, as any repository. What you commit \
               there is that repository's; this one only sees where it moved.",
        git: "git rev-parse in its folder, as opening any repository",
        cases: &[
            (
                "Disabled?",
                "A submodule not set up has no repository yet (set it up first); a worktree whose folder is missing can't open; this tab's own worktree is open already.",
            ),
            (
                "Committed in the submodule's tab?",
                "Come back and record its commit here, or this repository keeps recording the old one.",
            ),
        ],
    },
    Topic {
        id: "copy-its-path",
        group: Group::Inside,
        title: "Copying its path",
        on: &["Copy its path"],
        what: "Copies the folder's path to the clipboard: a worktree's full path on this \
               machine, or a submodule's path inside this repository, as git writes it.",
        git: "",
        cases: &[],
    },
    // Git LFS.
    Topic {
        id: "lfs",
        group: Group::Inside,
        title: "Git LFS",
        on: &[
            "LFS",
            "Git LFS",
            "Large files, kept apart",
            "Patterns, files, and locks…",
            "Open Git LFS",
        ],
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
                "Tracking only applies to files added from now on: add them again (or rewrite history with git lfs migrate).",
            ),
            (
                "A lockable file is read-only?",
                "Lock it first: whoever holds the lock may change and push it.",
            ),
            (
                "Cloned without git-lfs installed?",
                "Install it, set it up here, then Pull LFS files: the pointers become files.",
            ),
        ],
    },
    Topic {
        id: "lfs-state",
        group: Group::Inside,
        title: "Git LFS here",
        on: &[
            "git-lfs *, set up here",
            "Not set up here",
            "git-lfs isn't installed",
        ],
        what: "Whether Git LFS can work in this repository: git-lfs installed (its version \
               shown) and set up here (its filters in the configuration, its hooks in \
               .git/hooks). Without either, files LFS tracks stay pointers in the work tree, \
               and pushes leave their contents behind.",
        git: "git lfs version; filter.lfs.* in the configuration",
        cases: &[
            (
                "Installed but not set up?",
                "Each clone needs it once (or once for your user, from a terminal): Set up Git LFS here.",
            ),
            (
                "Installed after cloning?",
                "Set it up, then Pull LFS files: the pointers checked out before become files.",
            ),
        ],
    },
    Topic {
        id: "lfs-set-up",
        group: Group::Inside,
        title: "Setting up Git LFS",
        on: &["Set up Git LFS here"],
        what: "Adds Git LFS's filters to this repository's configuration and its hooks to \
               .git/hooks, so checking out turns pointers into files and pushing uploads them. \
               It changes nothing in the history or the work tree by itself.",
        git: "git lfs install --local",
        cases: &[
            (
                "Pointers still there afterwards?",
                "Files checked out before stay pointers: Pull LFS files (or Check out LFS files, when they're downloaded).",
            ),
            (
                "Hooks of your own in .git/hooks?",
                "git lfs won't replace a hook it didn't write and says so: add its line to that hook by hand (git lfs update --manual shows it).",
            ),
            (
                "For every repository at once?",
                "From a terminal, git lfs install (without --local) sets it up for your user.",
            ),
        ],
    },
    Topic {
        id: "lfs-pull",
        group: Group::Inside,
        title: "Pulling LFS files",
        on: &["Pull LFS files"],
        what: "Downloads the LFS files the current checkout needs from the remote's LFS store \
               and puts them in the work tree in place of their pointers. Commits and branches \
               don't move: it's Git LFS's own pull, not git's.",
        git: "git lfs pull",
        cases: &[
            (
                "After cloning with LFS skipped (GIT_LFS_SKIP_SMUDGE)?",
                "This is the step that fills the pointers in.",
            ),
            (
                "Asked for a password?",
                "The LFS server may want credentials of its own: egit relays the question.",
            ),
            (
                "A file is still a pointer?",
                "The store doesn't have it: someone pushed the commit without its LFS file. Whoever has it pushes it (Push LFS files).",
            ),
        ],
    },
    Topic {
        id: "lfs-fetch",
        group: Group::Inside,
        title: "Fetching LFS files",
        on: &["Fetch LFS files"],
        what: "Downloads the LFS files the current branch's recent commits need into \
               .git/lfs, leaving the work tree as it is: ready to check out later, or to work \
               offline.",
        git: "git lfs fetch",
        cases: &[
            (
                "Still pointers in the work tree?",
                "Fetching doesn't touch it: Check out LFS files puts what was downloaded in place.",
            ),
            (
                "Other branches' files?",
                "Fetch every file downloads all of history's.",
            ),
        ],
    },
    Topic {
        id: "lfs-fetch-all",
        group: Group::Inside,
        title: "Fetching every LFS file",
        on: &["Fetch every file"],
        what: "Downloads the LFS files every branch and every commit refers to: all of \
               history's, not only the checkout's. Useful before going offline or archiving; it \
               can take a lot of disk.",
        git: "git lfs fetch --all",
        cases: &[
            (
                "How much will it take?",
                "As much as the store holds for this repository's history; Files held counts only the current commit's.",
            ),
            (
                "Want the space back?",
                "Prune deletes again what nothing recent needs.",
            ),
        ],
    },
    Topic {
        id: "lfs-checkout",
        group: Group::Inside,
        title: "Checking out LFS files",
        on: &["Check out LFS files"],
        what: "Replaces pointers in the work tree with the files already downloaded into \
               .git/lfs, without downloading anything. Files not downloaded yet stay pointers.",
        git: "git lfs checkout",
        cases: &[
            (
                "Nothing changed?",
                "The files weren't downloaded: Fetch LFS files first, or Pull LFS files, which does both.",
            ),
            (
                "They show as changed afterwards?",
                "They shouldn't once LFS is set up here: git sees a file and its pointer as the same. If they do, set it up.",
            ),
        ],
    },
    Topic {
        id: "lfs-push",
        group: Group::Inside,
        title: "Pushing LFS files",
        on: &["Push LFS files to *"],
        what: "Uploads to that remote's LFS store the files every local branch refers to. \
               Pushing commits does this already for what they push, through LFS's pre-push \
               hook; this covers what that missed, such as a push made where LFS wasn't set up.",
        git: "git lfs push --all <remote>",
        cases: &[
            (
                "Others get pointers they can't download?",
                "Someone pushed commits without their LFS files: whoever has the files pushes them this way.",
            ),
            (
                "Refused?",
                "The server may need a sign-in with write access, or have a storage quota: its words say which.",
            ),
        ],
    },
    Topic {
        id: "lfs-prune",
        group: Group::Inside,
        title: "Pruning LFS files",
        on: &["Prune LFS files", "Prune"],
        what: "Deletes downloaded LFS files from .git/lfs that nothing recent needs: not the \
               checkout's, recent branches' and commits', or anything not pushed yet. The work \
               tree is left alone.",
        git: "git lfs prune",
        cases: &[
            (
                "Deleted something I need?",
                "Only local copies go: fetching brings them back from the store.",
            ),
            (
                "Files of commits not pushed yet?",
                "Kept: pruning never deletes the only copy.",
            ),
        ],
    },
    Topic {
        id: "lfs-track",
        group: Group::Inside,
        title: "Tracking a pattern",
        on: &["Track", "Tracked patterns"],
        what: "Tells Git LFS to keep files matching a pattern (*.psd, assets/**) apart: a line \
               in .gitattributes giving them LFS's filter, to commit. Files added or changed \
               from then on are stored as pointers; files already committed stay in git's \
               history as they are.",
        git: "git lfs track [--lockable] -- <pattern>",
        cases: &[
            (
                "Existing files are still in git?",
                "Tracking applies as files are added: stage them again (git add --renormalize) for the next commit. Older commits keep them unless history is rewritten (git lfs migrate).",
            ),
            (
                "Do others get it?",
                "Once .gitattributes is committed and pushed.",
            ),
            (
                "Which files does a pattern hold?",
                "Files held lists the current commit's LFS files.",
            ),
        ],
    },
    Topic {
        id: "lfs-lockable",
        group: Group::Inside,
        title: "Lockable files",
        on: &["Lockable"],
        what: "Tracks the pattern as lockable: matching files check out read-only until someone \
               locks them on the LFS server, so two people don't change a file that can't be \
               merged (an image, a 3D model) at once.",
        git: "git lfs track --lockable -- <pattern>",
        cases: &[
            (
                "A file I can't save?",
                "Lock it first (Locks, on this page): it's writable while you hold the lock.",
            ),
            (
                "A server without locks?",
                "Locking needs an LFS server that keeps them (GitHub's and GitLab's do).",
            ),
        ],
    },
    Topic {
        id: "lfs-untrack",
        group: Group::Inside,
        title: "Stopping tracking",
        on: &["Stop tracking"],
        what: "Removes the pattern's line from .gitattributes, to commit: git keeps matching \
               files itself from then on. Files already stored in LFS stay pointers in the \
               commits that have them.",
        git: "git lfs untrack -- <pattern>",
        cases: &[(
            "Still committed as pointers?",
            "Stage them again (git add --renormalize) so git stores their contents in the next commit.",
        )],
    },
    Topic {
        id: "lfs-files-held",
        group: Group::Inside,
        title: "Files held in LFS",
        on: &["Files held*"],
        what: "The LFS files the current commit refers to, each with its size and whether it's \
               here: checked out whole, downloaded but a pointer in the work tree, or not \
               downloaded. The line above sums their size and counts those not whole.",
        git: "git lfs ls-files --json",
        cases: &[
            (
                "Many not downloaded?",
                "Pull LFS files fetches and checks them out.",
            ),
            (
                "Only the current commit's?",
                "Yes: another branch's files are listed once it's checked out.",
            ),
        ],
    },
    Topic {
        id: "lfs-locks",
        group: Group::Inside,
        title: "Locks",
        on: &["Locks", "Read the locks", "Lock"],
        what: "Locks live on the LFS server: whoever holds a file's lock may change and push \
               it, and git lfs checks others' pushes against it. Read the locks lists them (it \
               asks the server); Lock takes one on a path, making the file writable here.",
        git: "git lfs locks --json; git lfs lock -- <path>",
        cases: &[
            (
                "Nothing listed?",
                "No file is locked, or the server doesn't keep locks.",
            ),
            (
                "A push refused over a lock?",
                "Someone else holds it: ask them to unlock it, or (with the right) force unlock it.",
            ),
            (
                "Locking a file that isn't lockable?",
                "It works, but nothing makes it read-only for others: lockable patterns do that.",
            ),
        ],
    },
    Topic {
        id: "lfs-unlock",
        group: Group::Inside,
        title: "Unlocking",
        on: &["Force unlock"],
        what: "Unlock gives back a lock you hold, once your change is pushed; a lockable file \
               turns read-only again. Force unlock takes away someone else's lock, which the \
               server allows only to those with the right: their change may then clash with \
               yours.",
        git: "git lfs unlock -- <path> (--force)",
        cases: &[(
            "Refused to unlock?",
            "git lfs won't while the file has changes not committed: commit them, or force it.",
        )],
    },
    // Worktrees.
    Topic {
        id: "worktree",
        group: Group::Inside,
        title: "Worktrees",
        on: &["Worktrees"],
        what: "More work trees for the same repository, each with its own folder, branch, \
               index, and uncommitted changes: work on two branches at once without stashing \
               or cloning again. They share one store of objects, so commits, branches, tags, \
               and stashes made in one show in all.",
        git: "git worktree list --porcelain -z; add, remove, lock, unlock, prune",
        cases: &[
            (
                "A branch can't be checked out?",
                "It's checked out in another worktree: git allows a branch in one worktree at a time. Switch that one away, or make a new branch.",
            ),
            (
                "The main worktree?",
                "The repository's own folder, holding .git: it can't be removed or locked.",
            ),
            (
                "Marked missing?",
                "Its folder is gone (deleted, or on a drive not mounted): forget those whose folders are gone, or lock it if the drive comes back.",
            ),
            (
                "Hooks and settings?",
                "Shared: one configuration and one hooks folder serve every worktree.",
            ),
        ],
    },
    Topic {
        id: "worktree-lock",
        group: Group::Inside,
        title: "Locking a worktree",
        on: &["Lock…", "Unlock"],
        what: "Locking marks a worktree so git won't prune it while its folder is missing (on a \
               removable or network drive, say), nor move or remove it; a reason can be noted. \
               Unlocking lifts that.",
        git: "git worktree lock [--reason <why>] <path>; git worktree unlock <path>",
        cases: &[
            (
                "Remove is disabled?",
                "A locked worktree must be unlocked first.",
            ),
            (
                "Does it stop commits there?",
                "No: locking protects the worktree's registration, not its files or branch.",
            ),
        ],
    },
    Topic {
        id: "worktree-remove",
        group: Group::Inside,
        title: "Removing a worktree",
        on: &["Remove anyway"],
        what: "Deletes the worktree's folder and its registration; its branch, and every commit \
               on it, stay in the repository. egit asks again when it has changes not committed \
               or files git doesn't track, which go with the folder: Remove anyway forces it.",
        git: "git worktree remove <path> (--force when asked again)",
        cases: &[
            (
                "Ignored files in it (build output)?",
                "git doesn't count them as changes: they go with the folder either way.",
            ),
            (
                "Submodules set up inside it?",
                "git removes such a worktree only when forced: Remove anyway.",
            ),
            (
                "Changed my mind?",
                "Its branch is still there: create a worktree from it again. Changes removed with it are gone.",
            ),
        ],
    },
    Topic {
        id: "worktree-prune",
        group: Group::Inside,
        title: "Forgetting worktrees whose folders are gone",
        on: &["Forget those whose folders are gone"],
        what: "Removes the registration of every worktree whose folder no longer exists \
               (deleted by hand, say), so its branch can be checked out elsewhere again. Locked \
               ones are kept.",
        git: "git worktree prune",
        cases: &[(
            "On a drive not plugged in?",
            "Lock it first, or pruning forgets it, and its folder, once back, is no longer a worktree git knows.",
        )],
    },
    // Preferences.
    Topic {
        id: "preferences",
        group: Group::Egit,
        title: "Preferences",
        on: &["Appearance", "Repositories", "Diffs and files"],
        what: "egit's own settings, in effect at once and kept for next time in egit's settings \
               file (not git's configuration): how it looks, what each repository does on its \
               own, and how files and diffs first show.",
        git: "",
        cases: &[
            (
                "Where are git's settings?",
                "Your profile (git's global configuration), and each repository's Settings page.",
            ),
            (
                "Shared between machines?",
                "No: they live with egit on this machine.",
            ),
        ],
    },
    Topic {
        id: "pref-theme",
        group: Group::Egit,
        title: "Theme",
        on: &["Theme", "As the desktop's", "Light", "Dark"],
        what: "egit's colours: as the desktop's (following the system's light or dark setting \
               as it changes), or always light or dark. The status bar's theme button switches \
               it too.",
        git: "",
        cases: &[
            (
                "Diffs coloured differently?",
                "Each theme highlights code its own way: base16-ocean in dark, InspiredGitHub in light.",
            ),
            (
                "As the desktop's doesn't follow?",
                "Some desktops don't tell applications their setting: pick Light or Dark.",
            ),
        ],
    },
    Topic {
        id: "pref-text-size",
        group: Group::Egit,
        title: "Text size",
        on: &["Text size"],
        what: "Scales everything egit draws, from 70% to 200%: text, controls, and spacing \
               alike. ⌘+ and ⌘− change it and ⌘0 puts it back at 100% (Ctrl on Linux and \
               Windows), as Reset does.",
        git: "",
        cases: &[(
            "Long lines no longer fit?",
            "Turn on wrapping (Long lines), or scroll sideways.",
        )],
    },
    Topic {
        id: "pref-dates",
        group: Group::Egit,
        title: "Commit dates",
        on: &["Commit dates", "2026-10-17 14:05", "5 minutes ago"],
        what: "How the commit table shows when each commit was written: the date and time in \
               your time zone (the Date column names it), or how long ago. git records each \
               date in its author's own time zone; the details show the author's clock on \
               hover.",
        git: "",
        cases: &[
            (
                "Author date or commit date?",
                "The author's: when the change was first written. A rebase or amend keeps it and changes the committer's date (in the details).",
            ),
            (
                "Dates out of order?",
                "The table follows the history's shape, not time: a commit rebased or cherry-picked later keeps its earlier author date.",
            ),
            (
                "A date in the future?",
                "git stores whatever the author's clock said: a wrong clock makes wrong dates.",
            ),
        ],
    },
    Topic {
        id: "pref-fetch",
        group: Group::Egit,
        title: "Fetching in the background",
        on: &["Fetch in the background"],
        what: "How often egit fetches every remote of an open repository on its own: never, or \
               every 1, 5 (the default), 15, or 30 minutes, the first 10 seconds after opening. \
               It never asks for a password: a remote that needs one waits for your own Fetch. \
               A line under the top bar says when your branch's upstream gained commits.",
        git: "git fetch --all --prune --no-auto-maintenance (never prompting)",
        cases: &[
            (
                "Does it change my branches?",
                "No: only remote-tracking branches (origin/main) move; yours wait for a pull.",
            ),
            (
                "On a metered connection?",
                "Set it to Never: fetches then happen only when you ask.",
            ),
            (
                "While I'm working?",
                "It stops as soon as you start a task of your own.",
            ),
        ],
    },
    Topic {
        id: "pref-sidebar",
        group: Group::Egit,
        title: "The sidebar",
        on: &["Sidebar", "Shown when a repository opens"],
        what: "Whether a repository's tab opens with the sidebar (branches, remotes, tags, \
               stashes, worktrees, submodules) beside its history. Each tab can still show or \
               hide it: ⌘B, or the button at the sidebar's top.",
        git: "",
        cases: &[(
            "Hidden, but I need a branch?",
            "⌘K finds every branch by name.",
        )],
    },
    Topic {
        id: "pref-diffs",
        group: Group::Egit,
        title: "How diffs first show",
        on: &["Diffs"],
        what: "Whether a file's diff opens unified (removed and added lines in one column) or \
               side by side (the old file beside the new). Each file view can switch while it's \
               open; this is how the next one starts.",
        git: "",
        cases: &[(
            "A narrow window?",
            "Side by side halves the width: long lines wrap or scroll sideways.",
        )],
    },
    Topic {
        id: "pref-long-lines",
        group: Group::Egit,
        title: "Long lines",
        on: &["Long lines", "Wrapped"],
        what: "Whether lines too long for a file's view wrap onto further rows or run past its \
               edge, to scroll sideways. A file view can switch while it's open; this is how the \
               next one starts.",
        git: "",
        cases: &[(
            "A minified file?",
            "Lines megabytes long draw quickly either way; wrapped, they take many rows.",
        )],
    },
    Topic {
        id: "pref-merge-panes",
        group: Group::Egit,
        title: "The merge editor's sides",
        on: &["Conflicts", "Both sides whole above the merged file"],
        what: "Whether the merge editor shows the current and incoming versions of a file in \
               conflict whole, side by side above the merged result, each scrolled to the \
               conflict selected. Off, only the merged file shows, each conflict's sides inside \
               it under their headers.",
        git: "",
        cases: &[(
            "Short on room?",
            "Turn it off, or turn off Both sides above in the editor's More menu while resolving.",
        )],
    },
    Topic {
        id: "pref-tab-width",
        group: Group::Egit,
        title: "Tab width",
        on: &["Tab width"],
        what: "How many cells a tab character takes in diffs and files: 2, 4, or 8 (git's and \
               most terminals' default). Only the display changes, never the files; text \
               already open takes it when opened again.",
        git: "",
        cases: &[(
            "Code lines up oddly?",
            "It was written with another tab width: try 4 or 2.",
        )],
    },
    Topic {
        id: "pref-shortcuts",
        group: Group::Egit,
        title: "Keyboard shortcuts",
        on: &["Keyboard shortcuts"],
        what: "Every shortcut egit answers, as your system writes them: ⌘ on macOS, Ctrl on \
               Linux and Windows. ⌘K (or ⇧⌘P) finds any command by name, and Alt held over \
               anything explains it.",
        git: "",
        cases: &[
            (
                "A shortcut does nothing?",
                "Most work only while no text field has the keys, and Undo only with no dialog open: click outside the field first.",
            ),
            ("Can I change them?", "Not yet: they're fixed."),
        ],
    },
    // The profile.
    Topic {
        id: "profile",
        group: Group::Egit,
        title: "Your git profile",
        on: &["Edit your profile"],
        what: "Who you are to git everywhere: settings in your global configuration \
               (~/.gitconfig) that every repository on this machine uses unless it sets its \
               own. egit edits them with git config --global, so the terminal and other tools \
               see the same.",
        git: "git config --global; git config --list --show-origin --show-scope",
        cases: &[
            (
                "Different at work?",
                "A repository's own configuration wins: set user.email there, or use git's includeIf in ~/.gitconfig for a whole folder.",
            ),
            (
                "Set by a file I didn't expect?",
                "Every setting lists where each value comes from; the last one read wins.",
            ),
        ],
    },
    Topic {
        id: "profile-identity",
        group: Group::Egit,
        title: "Your name and email",
        on: &["Identity"],
        what: "The name and email every commit you make records as its author and committer \
               (user.name, user.email). They're text, not an account: forges match commits to \
               people by the email.",
        git: "git config --global user.name, user.email",
        cases: &[
            (
                "Commits not linked to my account on GitHub or GitLab?",
                "The email isn't one your account lists: add it there, or use the address it gives you (a noreply one keeps yours private).",
            ),
            (
                "Already committed with the wrong one?",
                "A change here affects new commits only; old ones keep theirs unless history is rewritten.",
            ),
            (
                "git refuses to commit?",
                "With user.useConfigOnly set, git won't guess an identity: set both here.",
            ),
        ],
    },
    Topic {
        id: "profile-signing",
        group: Group::Egit,
        title: "Signing",
        on: &["Signing"],
        what: "A signature shows a commit or tag came from the holder of a key. git doesn't \
               sign by itself: it asks gpg (OpenPGP and X.509) or ssh-keygen (SSH) to, with the \
               key named here (user.signingKey). Others see a signature as good only if they \
               trust that key.",
        git: "git config --global gpg.format, user.signingKey, commit.gpgSign, tag.gpgSign",
        cases: &[
            (
                "Asked for a passphrase?",
                "Your key's tool asks; egit shows the question (an agent remembers it for a while).",
            ),
            (
                "Signed, but the forge says unverified?",
                "Upload the public key to your account there, with the email the commits carry.",
            ),
            (
                "Signing fails?",
                "The problem shows the tool's words: often a key that doesn't exist, or gpg not finding its agent.",
            ),
        ],
    },
    Topic {
        id: "profile-formats",
        group: Group::Egit,
        title: "Signature formats",
        on: &["OpenPGP", "SSH", "X.509"],
        what: "OpenPGP signs with a gpg key: a key id, or empty to let gpg pick one by your \
               email. SSH signs with an SSH key, often the one you push with: its public key \
               file, or the key itself. X.509 signs with a certificate through gpgsm, as \
               companies issue them. The choice is gpg.format.",
        git: "git config --global gpg.format openpgp|ssh|x509",
        cases: &[
            (
                "SSH signatures show as unchecked?",
                "Checking them needs gpg.ssh.allowedSignersFile: a file of the keys you trust, by email.",
            ),
            (
                "The key is in an SSH agent?",
                "Give its public key: ssh-keygen asks the agent to sign, with no file holding the secret.",
            ),
            (
                "Switching format?",
                "Older signatures stay as they were, checked by their own format's tool.",
            ),
        ],
    },
    Topic {
        id: "profile-sign-commits",
        group: Group::Egit,
        title: "Signing every commit",
        on: &["Sign every commit"],
        what: "Sets commit.gpgSign: git signs each commit you make, wherever this setting \
               reaches. The commit form's Sign starts checked then; unchecking it there skips \
               signing for that commit alone.",
        git: "git config --global commit.gpgSign true",
        cases: &[
            (
                "Commits made by a rebase?",
                "A rebase, amend, or cherry-pick makes new commits, signed again with your key, even when someone else wrote them.",
            ),
            (
                "No key set?",
                "Signing fails and no commit is made: set the key above, or uncheck this.",
            ),
        ],
    },
    Topic {
        id: "profile-sign-tags",
        group: Group::Egit,
        title: "Signing every annotated tag",
        on: &["Sign every annotated tag"],
        what: "Sets tag.gpgSign: each annotated tag (one with a message) is signed with your \
               key. Lightweight tags (a name only) aren't signed.",
        git: "git config --global tag.gpgSign true",
        cases: &[(
            "Lightweight tags while it's on?",
            "egit makes them with --no-sign: with tag.gpgSign on, git would otherwise ask for a message and sign one.",
        )],
    },
    Topic {
        id: "profile-save",
        group: Group::Egit,
        title: "Saving your profile",
        on: &[],
        what: "Save writes the fields you changed into your global configuration, each with git \
               config --global, then reads every setting again (the open repositories' too). \
               Revert puts the fields back as they were saved. A repository that sets a value \
               itself keeps its own.",
        git: "git config --global --replace-all <key> <value> (--unset-all when emptied)",
        cases: &[
            (
                "Saved, but a repository still shows the old name?",
                "It sets its own (its Settings page shows where): change it there.",
            ),
            (
                "Save is disabled?",
                "Nothing changed, or a field can't be saved as it is (a name with < or >, an email with spaces): it says which.",
            ),
        ],
    },
    Topic {
        id: "every-setting",
        group: Group::Egit,
        title: "Every setting",
        on: &["Every setting"],
        what: "Every value git reads, in the order it reads them: the system's, then your \
               global ones (and, on a repository's Settings page, the repository's own), each \
               with the file it comes from. For a key set more than once, the last value \
               applies, but for keys that take many (remotes' fetch lines, credential helpers).",
        git: "git config --list --show-origin --show-scope --null",
        cases: &[
            (
                "A value I can't place?",
                "Its origin names the file, an included one too.",
            ),
            (
                "Edit one here?",
                "Not in the table: use the profile's fields, or git config in a terminal.",
            ),
        ],
    },
    // Home.
    Topic {
        id: "home-open",
        group: Group::Egit,
        title: "Opening a repository",
        on: &["Open a repository", "Open folder…"],
        what: "Opens a repository in a tab: any folder inside one opens the whole repository \
               (git finds its top). A path can be typed instead (~ is your home folder). A bare \
               repository, which has no work tree, isn't opened, nor is a folder inside .git.",
        git: "git rev-parse, in that folder",
        cases: &[
            (
                "Not a repository?",
                "git found no repository at or above the folder: clone one, or run git init there from a terminal.",
            ),
            (
                "Dubious ownership?",
                "The folder belongs to another user: trust it (safe.directory) if you know who.",
            ),
            (
                "Opened a submodule instead of its parent?",
                "A submodule's folder is a repository of its own: open the parent from a folder above it.",
            ),
        ],
    },
    Topic {
        id: "home-trust",
        group: Group::Egit,
        title: "Trusting a folder",
        on: &["Trust this folder"],
        what: "git refuses repositories owned by another user (a shared drive, a USB stick, a \
               container's volume): their configuration and hooks could run commands as you. \
               Trusting adds the folder to safe.directory in your global configuration, after \
               which git, and egit, open it.",
        git: "git config --global --add safe.directory <path>",
        cases: &[
            (
                "Is it safe?",
                "If you know who owns the folder: a repository someone else controls could run their hooks as you.",
            ),
            (
                "Undo it?",
                "Remove that safe.directory line from your global configuration (~/.gitconfig).",
            ),
        ],
    },
    Topic {
        id: "home-recent",
        group: Group::Egit,
        title: "Recent repositories",
        on: &["Recent repositories"],
        what: "Repositories you opened, kept in egit's settings on this machine; a double click \
               opens one. The tabs open when egit closed come back by themselves.",
        git: "",
        cases: &[(
            "One moved or was deleted?",
            "Opening it says it's not a repository there: open it from its new folder.",
        )],
    },
    Topic {
        id: "home-git",
        group: Group::Egit,
        title: "Finding git",
        on: &[],
        what: "egit runs your installed git for everything, so it needs git 2.43.0 or newer on \
               the PATH (Git for Windows on Windows). Try again looks for it once more, after \
               you install or update it.",
        git: "git --version",
        cases: &[
            (
                "Installed, but not found?",
                "egit looks on the PATH it was started with: start it again from a session that has git on its PATH.",
            ),
            (
                "Too old?",
                "Update git: Ubuntu 24.04's packaged 2.43 is new enough.",
            ),
        ],
    },
    // A repository's pages.
    Topic {
        id: "repo-overview",
        group: Group::Egit,
        title: "The repository's overview",
        on: &["Overview"],
        what: "The repository at a glance: its branch and where it stands against its upstream, \
               an operation in progress, who commits here (the identity in effect in this \
               repository), and where it lives on disk.",
        git: "git status, git config --list, git rev-parse",
        cases: &[("Back to the history?", "History, or Escape.")],
    },
    Topic {
        id: "repo-settings",
        group: Group::Egit,
        title: "The repository's settings",
        on: &["Settings"],
        what: "Every setting in effect in this repository: the system's, your global ones, and \
               its own (.git/config), in the order git reads them, each with the file it comes \
               from. For a key set more than once, the last value applies.",
        git: "git config --list --show-origin --show-scope --null",
        cases: &[
            (
                "Why does this repository behave differently?",
                "Look for its own (local) values overriding your global ones.",
            ),
            (
                "Change one?",
                "From a terminal: git config --local <key> <value>.",
            ),
        ],
    },
    Topic {
        id: "repo-you",
        group: Group::Egit,
        title: "You, in this repository",
        on: &["You, here"],
        what: "The identity commits made in this repository record (user.name and user.email as \
               git resolves them here, its own values winning over your global ones) and \
               whether they're signed, each with where it's set.",
        git: "git config --list --show-origin, in this repository",
        cases: &[
            (
                "Not set?",
                "git refuses to commit without a name and email: set them in your profile, or in this repository alone.",
            ),
            (
                "A work email for this repository only?",
                "git config --local user.email there: it shows here at once.",
            ),
        ],
    },
    Topic {
        id: "repo-where",
        group: Group::Egit,
        title: "Where the repository lives",
        on: &["Where it lives"],
        what: "Its work tree (the folder of files), its git directory (where history and \
               settings are), the directory a linked worktree shares with the main one, and how \
               objects are named: SHA-1 (most repositories) or SHA-256.",
        git: "git rev-parse --show-toplevel --git-dir --git-common-dir --show-object-format",
        cases: &[
            (
                "A .git file instead of a folder?",
                "Submodules and linked worktrees keep their git directory elsewhere: the .git file points there.",
            ),
            (
                "SHA-256?",
                "A newer format: such a repository can't exchange commits with SHA-1 ones.",
            ),
        ],
    },
    Topic {
        id: "pref-pictures",
        group: Group::Egit,
        title: "People's pictures",
        on: &["People", "Their pictures", "Their initials"],
        what: "With Their pictures, each author's picture is fetched once and drawn in place of \
               their initials: GitHub's for its no-reply addresses (12345+login@users.\
               noreply.github.com), else Gravatar's. Those services are sent the author's email \
               (Gravatar, its SHA-256), never anything else. Each picture is kept on this \
               computer, 128 pixels square; someone without one keeps their initials, asked again \
               after a week. Their initials fetches nothing.",
        git: "",
        cases: &[
            (
                "Someone's picture is wrong or old?",
                "It is what Gravatar or GitHub has for that email. Delete egit's pictures folder in your cache folder to fetch them all again.",
            ),
            (
                "Offline?",
                "Initials show, and the pictures are asked for again later; those fetched before come from the disk.",
            ),
            (
                "Two names, one picture?",
                "Pictures follow the email: a person committing with two addresses may show two.",
            ),
        ],
    },
];
