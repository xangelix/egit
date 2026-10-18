# AGENTS.md

## What this is

**egit** — a native git client in egui, built to stay fast on the largest repositories
(GitKraken's ground, without Electron). The system's `git` binary does every repository
operation; egit runs it, reads what it prints, and draws only what is on screen.
`docs/ROADMAP.md` is the plan (five MVP phases, then one at a time) and the decisions behind
it.

## Status

| Phase | Scope | State |
|---|---|---|
| 1 | Foundation: shell, running git, askpass, profile, opening repositories | done |
| 2 | Commit graph: lanes, the commit table, details and changed files | done |
| 3 | Text grid and read-only diff viewer | done |
| 4 | Working tree, staging, signed commits | done |
| 5 | Clone, pull, push | done; checks against GitHub left to the owner |
| 6 | The merge editor | done |
| 7 | The graph's actions: branches, commits brought in, tags, stashes | done |
| 8 | Interactive rebase | done |
| 9 | Blame and file history | done |
| 10 | Pull requests on GitHub | done |
| 11 | Merge requests on GitLab | done; awaiting the owner's review |
| 12 | gitoxide as a read accelerator | done; awaiting the owner's review |
| 13 | Searching the history | done; awaiting the owner's review |
| 14 | The menus' remaining actions | done; awaiting the owner's review |
| 15 | Patches | done; awaiting the owner's review |
| 16 | Undo, and the reflog | done; awaiting the owner's review |
| 17 | The sidebar, and the repository's plumbing | done; awaiting the owner's review |
| 18 | Bisect, and rebasing merges | done; awaiting the owner's review |
| 19 | Polish: preferences, a command palette and shortcuts, the accessibility audit | done; awaiting the owner's review |
| 20 | macOS and Windows | done; linted for both from Linux, CI's runs left to the owner |
| 21 | Ergonomics: one look for every control | done; awaiting the owner's review |
| 22 | Readability: weights, surfaces, a tree, menus that scan | done; awaiting the owner's review |
| 23 | Alignment: to the point, edges, corners, padding | done; awaiting the owner's review |
| 24 | Follow-ups: scrolling, tabs, the details' head, rewording in place | done; awaiting the owner's review |
| 25 | The work in progress, people, trees, pictures, LFS, submodules, and Help | done; awaiting the owner's review |

## Toolchain & commands

- Pinned toolchain: `rust-toolchain.toml` → `1.99.1` (rustfmt, clippy); edition 2024.
- Build: `cargo build --workspace --all-targets`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` — must stay clean. Workspace
  lints deny `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
  `arithmetic_side_effects`, `as_conversions`, lossy casts, printing, and `missing_docs`, and
  warn on pedantic + nursery + cargo; `clippy.toml` relaxes the first four inside tests.
- Format: `cargo fmt --all`
- Tests: `cargo test --workspace`. Tests against git run in a sandbox
  (`egit_git::testing::Sandbox`: a temporary `HOME`, no system or user configuration).
- The git floor: `scripts/build-git.sh 2.43.0` builds that release into `target/git/2.43.0`;
  `EGIT_TEST_GIT=$PWD/target/git/2.43.0/bin/git cargo test --workspace` runs every test against
  it. Do this after touching anything that runs git (CI does it in `ubuntu:24.04`).
- Supply chain: `cargo deny check` (`deny.toml`) and `cargo shear` (unused dependencies).
- Other platforms: `scripts/check-platforms.sh` lints every target for Windows
  (`x86_64-pc-windows-gnu`, mingw-w64's gcc for the C dependencies) and macOS
  (`aarch64-apple-darwin`, zig's `cc` carrying Apple's libc headers) from Linux; nothing is
  linked or run there. Do this after touching anything behind a `cfg` of a platform. CI tests
  on both (the platforms job).
- Text corpus: `cargo test -p egit --release --lib -- --ignored --nocapture --test-threads=1
  corpus` draws the text grid through the roadmap's worst cases (1M lines of code, plain and
  highlighted, a 10 MB line wrapped and not, CJK and emoji, resizing wrapped at line 900k),
  then the whole application on a commit moving 100,000 files (listed in 105 ms, counted in
  430 ms; as a tree of their folders, arranged 4.9 ms after asking (in a job, its longest frame
  meanwhile 1.3 ms), the 100,000 folded away in a frame of 0.75 ms, scrolled at 0.49 ms p99),
  and prints frame times, laid out and tessellated (it fails past 8 ms at p99);
  renders `corpus-*.png` where it can. Every case stays under 2 ms but the highlighted ones
  (3.7 ms jumping, up to 2.6 ms scrolling: colouring takes up to 3 ms a frame).
  `corpus_merge`: the merge editor on 100,000 lines of C with 1,000 conflicts: merged by git
  in 220 ms, the document made in 9 ms (open, and again after a pick), stepping from conflict
  to conflict highlighted at 3.4 ms p99. `corpus_rebase`: the interactive rebase editor on
  10,000 commits: read and shown in 50 ms, scrolled at 0.43 ms p99, stepped at 0.52 ms.
  `corpus_blame`: 100,000 lines of C rewritten by 100 commits: the first lines labelled in
  47 ms, all in 0.65 s, jumping at 3.6 ms p99 while git blames (3.8 ms after: colouring).
  `corpus_pull_requests`: 1,000 open pull requests from a fake GitHub, then merge requests
  from a fake GitLab, on 10,000 commits: the first page shown 28 ms after opening, all in
  34 ms; the badged commit table scrolled at 0.54 ms p99, the list at 0.28 ms, stepped
  through (each one's details read) at 0.40 ms (each forge alike). `corpus_sidebar`: 100,000
  refs (50,000 branches in 100 folders, 30,000 of a remote's, 20,000 tags) on 1,000 commits:
  listed 280 ms after opening (its longest frame meanwhile 5.6 ms: they are sorted in a job),
  scrolled at 0.7–1.1 ms p99, the tags unfolded in a frame of 1.2 ms, each letter typed in the
  filter 0.6 ms at most (the rows made in a job, landing 3–5 ms after the last letter); then the command palette over its 50,001 branches: opened in a frame
  of 2.8 ms, each letter typed 4.1 ms at most; peak 179 MB.
- Scale: `EGIT_SCALE_REPO=<a large repository> cargo test -p egit --release --lib -- --ignored
  --nocapture scale` opens it headless and prints time to first rows, to the whole history,
  frame times jumping and wheel-scrolling through it, and peak memory, then how much of what
  it holds is allocated and how much is files mapped (it fails past 8 ms a frame). Linux
  (1.49M commits): first rows 85–100 ms, all 1.2–1.55 s, slowest frame 2.9 ms, peak 220 MB:
  78 MB allocated, the rest mostly packs gix read, mapped (page cache, which git's `cat-file`
  held in its own process: 91 MB with `EGIT_GIX=0`, first rows in 112–119 ms); with
  rendering 368 MB (124 MB allocated; 227 MB with `EGIT_GIX=0`), and `scale-*.png` screens.
  Then it searches the history (each kind) and prints how soon the first commit was found,
  how long all took, and the memory held: Linux's messages or authors in 0.6 s (the first in
  4 ms), peaking at 1 GB while the packs are read (mapped: released as it ends); a name at
  once (17 ms, the index built). Last, the history narrowed to the current branch and pinned,
  then whole again: each read again and shown once whole (Linux: 1.4–1.5 s).
- gitoxide's reads: `EGIT_SCALE_REPO=<a large repository> cargo test -p egit-gix --release
  --test measure -- --ignored --nocapture` times each against git's (the walk both ways,
  commits read fresh and warm, the walk's tips, references); `EGIT_GIX=0` leaves the
  application's every read to git, to compare.

## Supported platforms

Linux first: the floor is Ubuntu 24.04 LTS, glibc 2.39 and git 2.43.0. Never use a git feature
newer than 2.43.0 without a fallback, and check its release notes when unsure. macOS (11 or
newer) and Windows (10 or newer, with Git for Windows) too, since phase 20: what differs lives
behind `cfg`s in a few places (git's processes, the askpass transport, a watch of the work
tree, font folders, the browser opener); bytes git prints become paths through bstr's
`to_path_lossy`/`to_os_str_lossy` (byte for byte on Unix, UTF-8 on Windows, as Git for Windows
writes paths), never `OsStrExt`.

## Rules for running git

- Every repository operation runs the system `git`, but for the reads `egit-gix` makes in
  process through gitoxide (references, the walk's tips, objects), each only where it gives
  git's answers (checked against git's in its tests) and was measured faster; gix never
  writes, and a repository it wouldn't read as git does is left to git. Never read or write
  files under `.git` directly, except to watch them and to read the few state files git
  documents (`MERGE_HEAD`, `rebase-merge/`, …). No libgit2 (`deny.toml` bans it).
- Machine-readable output only: `-z`, `--porcelain`, explicit `--format`s, and `-c` overrides
  for any setting that changes a format the parser reads.
- Reads pass `--no-optional-locks`, so a background refresh never takes `index.lock` from the
  user's own git. Commands that write go through the repository's single write queue.
- Never SIGKILL git: cancellation is SIGTERM to its process group, which lets git remove its
  lock files (on Windows, which has no SIGTERM, Git for Windows' own way: `ExitProcess` run in
  each process, so git's exit handlers remove them).
- A command about one repository runs in its work tree (`current_dir`); one that must not see
  any repository (the user's global configuration) runs in `/` (on Windows the system drive's
  root: `outside_repositories`), never in egit's own working directory, which may be inside
  some repository.

## Repository layout

- `crates/egit-git` — the git binary: running it, and parsing what it prints. No egui.
  - `git.rs` — `Git`: the executable (found on `PATH` (`git.exe` on Windows) or given), its version checked against
    `GitVersion::MINIMUM` (`version.rs`), and the environment every git process gets
    (`env.rs`: repository variables (`git rev-parse --local-env-vars`) removed, `LC_MESSAGES=C`
    with the character set kept, no pager, editor `:`, `GIT_TERMINAL_PROMPT=0`, no `GPG_TTY`).
  - `command.rs` — `GitCommand` from `Git::read` / `Git::write`: `-c` settings, arguments,
    directory, stdin, and a `CancelToken`; `output` (collected up to `stdout_limit`, failing
    unless git succeeds), `output_unchecked` (any exit), `stream` (chunks through one reused
    buffer; the consumer may stop it). Reads add `--no-optional-locks`; writes are retried
    (25 ms … 800 ms) while `Stderr::held_lock` finds another git's lock. Every command gets
    `-c color.ui=never -c core.quotePath=false`. Stderr is drained on a helper thread into a
    64 KiB tail (`output.rs`), and into a `Progress` when the command has one (`progress`);
    stdin is fed from another. Secrets: `secret_stdin` and `secret_output` keep them in
    zeroizing buffers (growing never leaves a copy), never shown in debug output.
  - `progress.rs` — `Progress`: a long command's `--progress` lines read as they come, each
    replacing the last (git rewrites them with `\r`): the phase, done and total, the transfer
    (`1.20 MiB | 2.30 MiB/s`), whether the remote is doing it (`remote: `), and the other
    lines as notes (the latest fifty).
  - `process.rs` — git leading a group of its own, stopped and waited on safely, per platform
    (with the askpass's `getpeereid`, the crate's only `unsafe`). `process/unix.rs`: `setsid`
    in `pre_exec` (no controlling terminal; git leads its process group), SIGTERM to the
    group, the leader waited for with `waitid(WNOWAIT)` so a cancel can't signal a reused id.
    `process/windows.rs`: a new process group without a console window
    (`CREATE_NEW_PROCESS_GROUP`, `CREATE_NO_WINDOW`); stopped as Git for Windows' `kill` stops
    a process, a thread running kernel32's `ExitProcess` injected into each (its exit
    handlers run: git's remove its lock files), the processes git started first (found by
    their parents in a Toolhelp snapshot, each born after its parent so a reused id can't
    pass), terminated only where a thread can't be injected (another architecture); git's id
    stays reserved while egit holds its `Child`. `cancel.rs` holds the running groups under
    one lock.
  - `error.rs` — `GitError`, with `Failure` (exit status + stderr tail, `message()`).
  - `object_id.rs` — `ObjectId` (SHA-1 or SHA-256, by length; never assume 40 digits) and
    `ObjectFormat` (`zero`, the name of no object).
  - `repo.rs` — `Repository::discover` (two `rev-parse`s; `DiscoverError` names not a
    repository, `safe.directory` refusals (`trust_directory` adds one), bare repositories, and
    paths inside `.git`), `head` (`symbolic-ref`, then `for-each-ref` on that exact ref for the
    commit and `%(upstream:track)`, or `log -1` when detached), `head_ref` (`HeadRef`: just
    the branch's full name and the commit, by `symbolic-ref` and `rev-parse`, without the
    upstream's ahead and behind), `upstream_tip` (the current branch's upstream, its full name
    and commit, by `rev-parse --symbolic-full-name @{upstream}`; none when detached, without
    one, or gone), `head_reaches` (whether `HEAD`'s history holds a commit:
    `merge-base --is-ancestor`), and `operations` (merge, rebase, am, cherry-pick, revert,
    bisect from their state files in the git dir).
  - `config.rs` — `ConfigSnapshot::read(git, ConfigPlace::{User, Repository})`: one
    `git config --list --show-origin --show-scope --null`, kept as one buffer the entries point
    into (u32 spans); entries in the command scope (egit's own `-c`) are dropped. `get` is the
    last value (git's rule), keys compare like git's (section and name in any case, subsection
    exactly); `is_partial_clone` (a promisor remote or a clone's filter), `has_every_tree`
    (not partial, or each filter `blob:…`: walking trees fetches nothing). `set_config`
    (`--replace-all`) and `unset_config` (`--unset-all`; exit 5 is fine) write `--global` from
    `/` or `--local` in a work tree; `add_config_value` and `unset_config_value` add and remove
    one value of a key that has several (`--fixed-value`: a value is never a pattern).
  - `profile.rs` — `Profile` over a snapshot: identity, `Signing` (format, key, defaults,
    program — `gpg.program` and `gpg.openpgp.program` are one setting, the later wins —,
    allowed signers), credential helpers (an empty value resets the list), pull and push
    settings; each as a `Setting` or `ConfigEntry` that knows its scope and file.
  - `askpass/` — prompts from git's and ssh's helpers relayed to the window. `AskpassServer`
    makes `egit-<uid>/<pid>-<nonce>/` (0700, ownership checked) under `$XDG_RUNTIME_DIR` or
    the temporary directory, holding a Unix socket and `egit-askpass`, a link to egit's
    executable; `environment()` sets `GIT_ASKPASS`, `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force`,
    the socket, and a random token (`getrandom`). Started under that name, egit is the helper
    (`run_helper`). Connections must come from the same uid (`SO_PEERCRED`; `getpeereid` on
    macOS) and carry the token; fields are bounded (`protocol.rs`); answers are zeroized. Its
    accept loop polls the listener and a socket pair of its own, so stopping never depends on
    the socket's path. Each platform's part: `askpass/unix.rs`; `askpass/windows.rs`, where
    the standard library has no local sockets: a port on the loopback address that the token
    alone lets a helper into, `egit-<user>` under the user's temporary directory, the helper
    `egit-askpass.exe` a hard link (or a copy on another volume), and a file held open without
    sharing marking a server alive (another egit sweeps directories whose file it can remove). `Prompt`s
    (`PromptKind`: visible, secret, yes/no, confirm, notice) are answered, refused, or dropped
    (refused); `is_abandoned` notices an asker that went away. `GitCommand::prompt_context`
    labels a command's prompts; `non_interactive` makes them fail at once (and
    `GCM_INTERACTIVE=never`). `tests/askpass.rs` runs without libtest so its executable can be
    the helper git starts.
  - `batch.rs` — `GitProcess` from `GitCommand::spawn`: a git kept running for a batch
    protocol (stdin requests, stdout answers); dropping it closes stdin, SIGTERMs its group,
    reaps it.
  - `objects.rs` — `ObjectReader`: one `cat-file --batch-command --buffer` per reader;
    requests go 128 at a time then `flush` (both pipes stay under their buffers), answers are
    read into one reused buffer. `commit.rs` — `Commit::parse` (raw commit objects; `subject`
    is `%s`; `co_authors`, each `Person` its `Co-authored-by` trailers name, in any case,
    once). `people.rs` — `recent_authors`: who wrote the last commits of every branch (`log
    --all --no-merges`, the stash left out), through the mailmap (`%aN`, `%aE`), most commits
    first, each email once.
  - `history.rs` — `walk_history` (`rev-list --topo-order --parents`, streamed, lines split
    across chunks without copying; with a commit-graph it starts from `merge-base
    --independent` of `--all`'s tips, since git's topological walk first reads down to the
    oldest tip: Linux's first commit in 60 ms instead of 700; `walk_history_from` starts from
    those tips listed already; `history_tips` lists them with some refs hidden: `--exclude=`
    each and `--all` on standard input, however many), `list_references` (`for-each-ref`, symbolic refs skipped, tags
    peeled through every tag: git 2.43 peels a tag of a tag one level, so those are peeled by
    one `rev-parse <name>^{}`), `commit_changes` (`diff-tree -r -z
    --raw --no-abbrev -M` against the first parent, or `--root`: each side's mode and object),
    `line_counts` (the same with `--numstat`, in the same order: every file diffed, so it
    follows the list; `GIT_NO_LAZY_FETCH` where git honours it, never prompting),
    `write_commit_graph`, `commits_between` (`rev-list <tip> ^<base>`, newest first, at most
    a limit: what a branch would bring in), `count_between` (the same, `--count`);
    `Repository::has_commit_graph` (without one, a topological walk outputs nothing until
    git has read all history: 6 s for Linux's 1.5M commits).
  - `diff.rs` — `file_diff`: one file's patch from `diff-tree -p` (plumbing, so no colour,
    prefix, external-diff, or textconv setting applies; `--no-textconv --full-index
    --src-prefix=a/ --dst-prefix=b/`; `GIT_LITERAL_PATHSPECS`; a rename's both paths;
    `DiffOptions`: context (`WHOLE_FILE` for all of it) and the user's `diff.algorithm`, which
    plumbing doesn't read). `uncommitted_diff`: a file's `Uncommitted` changes from `git
    diff` (`Unstaged`; `Staged`, `--cached -M`, before the first commit too; `Untracked`,
    `--no-index` against `/dev/null`, which exits 1 when they differ), with every setting that
    changes the patch pinned (`--no-relative`, prefixes, colour, external diff, textconv); the
    work tree's side comes through clean filters and line-ending conversion, in the form git
    stores. Parsed as it streams into `FileDiff`: each line's text without
    its prefix (hunk headers as lines), its `DiffLineKind`, old and new numbers, and a
    missing final newline; a binary file is flagged and has no lines. `lines.rs` — `Lines`,
    the streamed-output splitter `walk_history` and `file_diff` share.
  - `patch.rs` — staging or unstaging picked lines. `apply_lines`: the index's blob with the
    picked lines of a diff moved across (`Direction::Stage`: the index against the work tree,
    picked additions go in and removals go out; `Unstage`: `HEAD` against the index, the other
    way); base lines are copied in bulk, every line the diff says the index holds is checked
    (`LinesError::Stale` otherwise), and a line left without a newline gains one when picked
    lines follow it. `write_index_content`: `ls-files --stage` must still show the entry the
    diff was made from, then `hash-object -w --no-filters --stdin` and
    `update-index -z --index-info`. `tests/lines.rs` checks both directions against
    `git apply --cached` on random edits and picks (250 cases each way; zero-context cases
    touching a missing final newline are skipped, as git's `--unidiff-zero` mishandles them).
  - `status.rs` — `Status::read`: `git status --porcelain=v2 -z --untracked-files=all` (each
    untracked file on its own), kept as git's output plus a table of byte ranges into it
    (`span.rs`, shared with `config.rs`); modes and object names are checked once and decoded
    when asked for, so 100,000 untracked files cost a few megabytes. `StatusEntry`: its kind
    (changed, renamed, copied, unmerged, untracked), path and a rename's old one, `staged` and
    `unstaged` `Change`s, `head` and `index` tree entries (the index's blob is what lines are
    staged against), the work tree's mode, a conflict's sides, a submodule's changes.
  - `stage.rs` — `stage` (`git add --all`) and `unstage` (`git reset --quiet`, which, unlike
    `restore --staged`, works before the first commit) of whole files: paths on standard
    input (`--pathspec-from-file`, NUL-separated) as literal pathspecs; no paths do nothing
    (`reset` would otherwise unstage everything).
  - `tree.rs` — for watching a work tree's folders: `tree_folders` (a commit's folders,
    `ls-tree -r -d`) and `ignored_paths` (`check-ignore -z --stdin`, which exits 1 when none
    is; folders end with `/` so folder-only patterns match them); and `tree_entry`, one path's
    entry at a commit (`ls-tree -z --full-tree`, a literal pathspec), or none. `git_path`: a
    path relative to a work tree as git writes it (`/` between folders, on Windows too).
  - `lfs.rs` — Git LFS through `git lfs`: `lfs_version` (none when it isn't installed),
    `lfs_set_up` (its filters in a configuration), `lfs_patterns` (`track --json`, git-lfs 3.4
    and newer; its listing otherwise: tracked, excluded, lockable), `lfs_files` (`ls-files
    --json` at `HEAD`: size, checked out, downloaded), `lfs_install` (`--local`),
    `lfs_track` (`--lockable`) and `lfs_untrack`, `lfs_transfer` (`LfsTransfer`: fetch, all
    or the current branch's; check out; pull; push `--all` to a remote; progress, labelled
    prompts), `lfs_prune`, `lfs_locks` (`locks --json`), `lfs_lock` (lock, unlock, force).
    `tests/lfs.rs` pushes through `file://` and pulls into a clone that had pointers
    (skipped without git-lfs unless `EGIT_REQUIRE_LFS`, which CI sets).
  - `committing.rs` — `create_commit`: `git commit --file=- --cleanup=whitespace` (a line
    starting with `#` is kept), `NewCommit`'s amend, sign-off, trailers (`--trailer`, placed
    by git's rules: co-authors), and signing (`-S`,
    `--no-gpg-sign`, or the configuration's choice); a passphrase is asked through askpass,
    labelled "Signing the commit"; `Committed::output` is what git and its hooks printed (a
    refusal's whole standard error is on the failure). `reword_head`: `HEAD`'s message alone
    (`commit --amend --only --allow-empty`: what is staged stays staged, out of it).
  - `signatures.rs` — `check_signatures`: some commits' signatures checked as `git log
    --show-signature` checks them (`log --no-walk=unsorted --stdin -z
    --format=%H%x1f%G?%x1f%GS%x1f%GK`, `log.showSignature` off), each by its format's program;
    a `Verdict` (good, untrusted, bad, expired, revoked, unchecked, unsigned), signer, and key.
    `tests/signing.rs` signs commits made by `create_commit` each way and checks them both
    there and in `git log --show-signature`: an SSH key file, an SSH agent holding the key
    (the file deleted), a signing program of the user's own, OpenPGP, and X.509 (gpgsm
    certifying a key gpg made, since gpgsm can't make one without a passphrase; trusted by
    `trustlist.txt`). A test whose tools are missing is skipped, unless
    `EGIT_REQUIRE_SIGNERS` is set (CI installs GnuPG, gpgsm, and OpenSSH and sets it).
  - `remote.rs` — moving commits, each with `--progress` into a `Progress`, a `CancelToken`,
    and askpass prompts labelled by what is done: `clone` (from `/`; `CloneOptions`:
    submodules, partial `--filter=blob:none`, both with `--also-filter-submodules`),
    `fetch` (`FetchOptions`: `--all`, `--prune`, and in the background `non_interactive` and
    `--no-auto-maintenance`), `pull` (`PullMode`: as configured, merge, rebase, fast-forward
    only; `Pulled::Diverged` when git says the branches diverged, `Pulled::Conflicts` when a
    merge or rebase stopped (from the repository's state)), `push` (`--porcelain`, one branch
    to a remote branch; `--set-upstream`; `force` is `--force-with-lease
    --force-if-includes`; each ref's `PushFlag` and git's reason for a refusal, other lines as
    notes; a refusal isn't an error), `remotes`, `remote_urls` (each remote's URL as git
    reaches it, `insteadOf` applied: `ls-remote --get-url`), `fetch_refspecs` (just those
    refspecs, no tags: a pull request's `refs/pull/<n>/head`, a lone branch), `fetch_remote`
    (one remote alone, pruned or not), and
    `abort_operation` /
    `continue_operation` for a merge, rebase, am, cherry-pick, revert (bisect: `reset`);
    `skip_operation` (`--skip` of a rebase, am, cherry-pick, revert: `Integrated` as it then
    ended, done or stopped again).
    `push_tag` and `delete_remote_branch` push one refspec the same way (`Push::run`).
    `tests/remote.rs` runs each against a bare repository through `file://` (and a clone
    cancelled while its remote, git's `ext::` transport running `sleep`, hasn't answered:
    git removes its folder).
  - `conflict.rs` — a file in conflict. `conflict_sides`: its `ConflictSides` from
    `ls-files -u` (stage 1 the base, 2 current, 3 incoming; a side missing where it deleted
    the file; `merged_mode`). `merge_sides`: two texts merged as `git merge` merges them
    (`merge-file --object-id --zdiff3`, `--diff-algorithm=histogram` from git 2.44: ort's
    diff; 2.43 merges with Myers), the sides read first through `cat-file` for binary contents
    (`Merge::WholeOnly`, with deleted sides, links, and submodules), a last line without a
    newline, and the longest run of a marker character starting a line, so the markers asked
    for are longer than any line of the file can pass for; `MergedText` is git's output and
    `Piece`s (ranges into it), and `resolve`/`picked` take each conflict's `Pick` (a side,
    the base, both in either order, or text), a last line keeping its missing newline. An
    add/add conflict merges from the empty blob (written first). `resolve_conflict`: a
    `Resolution` (content, a side, or deletion) saved if `ls-files -u` still shows the same
    sides: `hash-object -w --no-filters`, a stage-0 entry through `update-index --index-info`
    (it replaces the stages), and `checkout-index --force` writing the work tree through the
    filters; deletion is `git rm --force`. `incoming_commit`: the commit merged, picked,
    reverted, or replayed (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`),
    with the branches at it (`%D`, `--decorate-refs`). `MergedText::parse` reads `merge-file`
    output of one's own. `tests/conflict.rs` resolves conflicts from each operation and continues it, and checks
    that taking one side everywhere is `merge-file --ours`/`--theirs` on 250 random cases
    (lines like markers, missing final newlines).
  - `branch.rs` — `check_ref_name` (git's `check-ref-format` rules, so a dialog says what is
    wrong at once: `BadRefName`); `switch` (`git switch --no-guess`) to a `SwitchTo`: a
    branch, a remote branch through a new local one tracking it, a new branch at a commit, or a
    commit detached; changes in the way are `SwitchError::WouldOverwrite` with their paths
    (local or untracked, from git's message). `create_branch`, `rename_branch` (`--move`),
    `delete_branch` (`DeleteBranchError::NotMerged` unless forced), `create_tag` (annotated
    from a message on standard input, `--cleanup=whitespace`; lightweight `--no-sign`, as
    `tag.gpgSign` would ask for a message), `delete_tag`, `set_upstream` (`branch
    --set-upstream-to`, or `--unset-upstream`), `move_branch` (`branch --force`: git refuses
    a branch checked out).
  - `forward.rs` — fast-forwards, never a merge: `fast_forward_current` (`merge --ff-only`:
    "Already up to date", "Not possible to fast-forward" read as `Forwarded`), and
    `fast_forward_branch` for a branch not checked out (`fetch --porcelain --no-prune` with a
    refspec without `+`, from a remote's branch, a pull without a merge, or from a ref here,
    `.`): `<flag> <old> <new> <ref>` lines, `!` refused, nothing for a ref already there.
  - `worktree.rs` — `add_worktree`: `git worktree add` of a `WorktreeFrom` (a branch, a new
    branch at a ref or commit, `--track` for a remote's, or a commit `--detach`) into a folder.
    `worktrees`: `worktree list --porcelain -z`, each `Worktree` (path, `HEAD` (none for an
    unborn branch's zeros), branch, main, bare, why locked, why prunable); `remove_worktree`
    (`RemoveWorktreeError::Changes` unless forced, `Locked`), `lock_worktree` (a reason),
    `unlock_worktree`, `prune_worktrees`.
  - `remote_setup.rs` — remotes set up by `git remote`: `add_remote`, `rename_remote` (its
    branches and their followers along), `remove_remote`, `set_remote_url` (fetching, or
    `--push`).
  - `submodule.rs` — `submodules`: those `.gitmodules` names (`config --file .gitmodules
    --get-regexp`) that the index records (`ls-files -s` of their literal paths: mode 160000),
    each `Submodule` (name, path, address, branch, the commit recorded, `initialized` with
    `configured_url`, the address set up here: `.gitmodules` may have moved on, which `sync`
    follows); `submodule_status` (`submodule status`, `--recursive`: each `SubmoduleStatus`'s
    `Checkout`, not set up, at the commit recorded, elsewhere, or in conflict, its commit and
    describe); `submodule_details` (inside one: its `HEAD`, a branch if it is on one, whether
    it has the commit recorded, ahead and behind it); `submodule_commits` (`log from..to` in
    it: what moving the commit recorded brings; none when it isn't here or lacks them);
    `update_submodules_to` (`UpdateTo`: the commits recorded, or each one's branch's latest,
    `--remote`; `--init --recursive --progress`, these paths or all) and `update_submodules`;
    `change_submodules` (`SubmoduleChange`: init, sync `--recursive`, deinit, forced or not,
    `--all` for none named); `add_submodule` (`--branch`, progress, labelled prompts),
    `remove_submodule` (`git rm`, `.gitmodules` staged first, as git asks),
    `set_submodule_url`, `set_submodule_branch` (`--default` for none). `tests/submodules.rs`
    adds one following a branch (holding one of its own), moves it to its branch's latest and
    lists what that brings, records it again, changes and syncs addresses, sets branches,
    deinits and sets it up again, and removes it. `tests/plumbing.rs` lists,
    locks, removes (refused with changes or locked), and prunes worktrees; adds, renames,
    re-points, fetches alone (pruned), and removes remotes; lists submodules (in a clone, not
    set up) and sets them up; and starts a walk with branches hidden.
  - `format_patch.rs` — `format_patch` (`format-patch -1 --stdout --default-prefix`: `a/` and
    `b/` whatever `diff.noprefix` says), `patch_file_name` (as git names one: `0001-` and the
    subject's letters, 64 bytes at most); `format_patches` (the commits from one, in `HEAD`'s
    history, up to `HEAD` into a folder, `--root` from the first; at most `MAX_PLANNED`);
    `changes_patch` (`Changes::Staged`, `git diff --cached`, or `All`, `git diff HEAD`:
    `--binary`, prefixes, no external diff or textconv, from the top).
  - `applying.rs` — patches to apply. `patch_kind`: a `Mailbox` (an mbox's `From ` line, or
    mail headers with a `Subject:`) or a `Diff`, or not a patch. `summarize_patch`, read
    without applying: a mailbox's commits (`mailsplit` into a temporary folder, `mailinfo`
    each: subject, author, date; the first `MAX_SUMMARIZED`), the files (`apply --numstat -z`,
    summed per file; `--summary` for added and deleted; renames), and `Applies` from `apply
    --check`, then `--check --3way` (merging, with or without conflicts; not, git's reason;
    a series unchecked, its commits building on each other). `apply_patch` as `ApplyAs`:
    commits (`am --3way`, `--signoff`) or changes (`apply`, `--3way` when staged), ending
    `Integrated::Done`, `Conflicts`, or `Stopped` (an `am` left at a patch that doesn't apply,
    nothing to resolve). `Repository::mailbox_progress` (`rebase-apply/next`, `last`, and
    `info`'s subject), `current_patch` (`am --show-current-patch=diff`), `looks_like_patch`
    (`.patch`, `.diff`, `.mbox`, `.eml`).
  - `integrate.rs` — commits brought into the current branch: `merge` (`--no-edit`),
    `rebase` (keeping merges, `--rebase-merges`, when asked), `cherry_pick`, `revert` (`--no-edit`; a merge's against its first parent, by
    `rev-list --parents`), each `Integrated::Done`, `Conflicts` (failed with the operation in
    progress and `ls-files -u` not empty), `Empty` (a pick that is "now empty", skipped), or
    `Stopped` (an operation left with nothing to resolve); `reset` (`ResetMode`: soft, mixed,
    hard, keep: uncommitted changes kept, nothing moved when they're in the way).
  - `stash.rs` — `stashes` (`stash list -z`, `%gd %H %ct %gs`: index, commit, time, git's
    message), `stash_push` (a message, untracked files with `--include-untracked`; whether
    anything was stashed, from `refs/stash` before and after, as `--quiet` says nothing),
    `stash_apply` (and pop; `Integrated::Conflicts` keeps a popped stash) and `stash_drop`,
    each refused (`StashError::Stale`) unless `stash@{n}` is still the stash listed;
    `stash_store` (a dropped stash's commit put back as the latest, `stash store`).
    `tests/actions.rs` runs all of these, and the pushes, against sandboxes.
  - `rebase.rs` — interactive rebase. `rebase_range`: the commits from one up to `HEAD`
    (`merge-base --is-ancestor`, then `log -z --reverse <from>^..HEAD`, or all from a root),
    oldest first, with authors and whole messages, merges marked (`RangeCommit::merge`,
    `RebaseRange::merges`); refused (`RangeError`) when `HEAD` doesn't reach it, or past
    `MAX_PLANNED` (10,000). `rebase_interactive`: a plan of `Planned` commits (`Step`: pick, reword with a
    message, squash, fixup, drop) checked (`RebaseError`: nothing to squash into, an empty
    message) and written as git's todo in a temporary file (`tempfile`) that the sequence
    editor copies in (`GIT_SEQUENCE_EDITOR=cp -- <file>`; `--no-autosquash
    --no-update-refs`, `--root` from the root); a reword is a pick, then, after the commits
    squashed or fixed up into it, `exec printf … | git commit --amend --file=-` carrying its
    message (each line quoted for the shell), so the todo holds everything and a rebase
    continued after a conflict still rewords. Conflicts are `Integrated::Conflicts`, as
    `integrate.rs` tells. `rebase_keeping_merges`: a range holding merges, in git's own
    `--rebase-merges` todo, read first by a dry run (a sequence editor that copies the todo out
    and fails: git gives up before anything moves, leaving no state, `ORIG_HEAD`, or reflog
    entry; `core.abbrev` at full length, commands unabbreviated), the plan's drops and rewords
    written into it (`with_plan`: `pick` lines dropped, `exec` amends after a reworded `pick`
    or `merge -C`), then run with it; melding (`MeldWithMerges`), a dropped merge
    (`DropMerge`), and a plan git's todo doesn't match (`Mismatch`) refused. `tests/rebase.rs`
    reads ranges and runs plans (reorder, reword, squash, fixup, drop, the root, conflicts and
    continuing, a dirty work tree refused, a range with a merge keeping it, a rebase onto a
    branch keeping merges or flattening them).
  - `bisect.rs` — `bisect_mark` (`git bisect <term> <commit>` in the bisection's terms, read
    by `bisect terms`; `bisect start` first when none is under way; status 2, only skipped
    commits left, isn't a failure). `bisection`: where one stands, from git's records rather
    than its messages (which change between versions): the terms, `refs/bisect/` (the bad
    commit, the good ones, those skipped), and `BisectLeft` worked out from them (needs a bad
    or a good commit; `Testing` with how many candidates, counted by `rev-list --stdin
    --count`, `steps` about their log; `Found` the first bad commit when it alone isn't
    skipped among the candidates (listed when 1,024 or fewer); `Among` those it could be).
    Stopped by `abort_operation` (`bisect reset`). `tests/bisect.rs` finds a first bad
    commit, leaves a choice when skipping, and marks in a bisection's own terms.
  - `blame.rs` — `blame`: `git blame --incremental` (renames followed; lines moved within the
    file only with `Moves::Writing`, `-M`, which is far slower on a large file) at a commit or
    the work tree, streamed through `Lines`, each `BlameRun` (its
    lines here, from 0, and there) handed over as git finds it with the commits so far
    (`BlameCommit`: author, time, summary, `boundary`, `previous` commit and path; a commit's
    details come once, found again by a sorted index; uncommitted lines share a commit without
    a name). `file_log`: `git log --follow -M --name-status -z`, records split on `%x1e` as
    they stream, each `FileCommit` (first parent, author, time, subject, git's status letter,
    its path there and a rename's old one). `tests/blame.rs` blames across a rename and a move
    (either way) and the work tree, and follows a rename's history.
  - `credential.rs` — git's credential helpers: `fill_credential` for a `CredentialPlace`
    (protocol, host; refused with line breaks, which would inject attributes), in a
    repository's configuration or the user's (`/`), `Asking::Never` (non-interactive: no
    credential is `None`, not an error) or labelled prompts; `approve_credential` and
    `reject_credential` hand back every attribute `fill` gave (an expiry, a refresh token).
    A `Credential` is wiped when dropped. `tests/credential.rs` fills, rejects, and approves
    through a sandbox's `store` helper.
  - `reflog.rs` — `reflog`: a ref's moves (`HEAD`, or a full name; never an option), newest
    first, at most a limit (`log --walk-reflogs -z --date=unix`, so `%gd` carries the time):
    each `ReflogEntry`'s place, commit, time, what moved it (`%gs`), and subject; none when
    there's no reflog (no commit yet, no such ref). `ref_target` (`rev-parse --verify`: the
    object a ref points to, an annotated tag's own), `create_ref` and `delete_ref`
    (`update-ref` with the old value: zeros, so it must not exist, or where it must still be).
    `tests/reflog.rs` reads reflogs, `HEAD` and refs, resets keeping changes, stores a dropped
    stash again, and makes and removes refs only where expected.
  - `search.rs` — `find_commits`: the commits a `CommitFilter` finds among every commit a
    history shows (`rev-list --exclude=refs/stash --all`, streamed through `Lines`): a
    message (`--grep`) or author (`--author`) holding some text (`--fixed-strings`,
    `--regexp-ignore-case` when folded), or a path changed, as `git log -- <path>` lists them
    (literal pathspecs, `GIT_NO_LAZY_FETCH`; git's default simplification: `--full-history`
    lists twenty times as many commits for a file of git's own, nearly all merges, five times
    as slowly). `resolve_commit`: `rev-parse --verify --quiet <name>^{commit}` (a name
    starting with `-` names nothing). `tests/search.rs` runs each against a sandbox;
    `tests/branches.rs` fast-forwards (moved, there already, refused when diverged or checked
    out), upstreams, worktrees (each way, a branch in use refused), and a patch `git am`
    applies elsewhere (the first commit's too); `tests/patches.rs` series saved, changes as a
    patch (binary too), summaries and checks (cleanly, merging with conflicts, not without the
    blobs), a series applied (signed off, its author kept; stopped at a conflict with its
    progress and patch, skipped, aborted; stopped where a patch doesn't apply at all), a diff
    applied as changes (staged or not, conflicting), and a stopped cherry-pick skipped.
  - `testing.rs` (feature `testing`) — `Sandbox`; `EGIT_TEST_GIT` picks the git to test;
    `set_mode` (a script's permission bits, where files have them).
- `crates/egit-forge` — forges' web APIs: GitHub's (github.com, Enterprise Server) and GitLab's
  (gitlab.com, self-managed); no egui. egit's only network connections of its own (git makes
  every other).
  - `host.rs` — `ForgeKind` (GitHub, GitLab; `name`). `ForgeHost`: a kind and a host:
    github.com (APIs on `api.github.com`), an Enterprise host (`/api/graphql`, `/api/v3`),
    GitLab's (`/api/graphql`, `/api/v4`); `parse` reads a listed entry (`host[:port]`,
    `https://`; `http://` only for the loopback address); `is_public` (github.com, gitlab.com);
    `new_token_url` (the forge's page making a token with what egit needs). `ForgeHosts`:
    github.com, gitlab.com, and the listed (`with_listed`: each kind's); `for_web` by
    `host[:port]` (no default port), `for_ssh` by host name (`ssh.github.com`,
    `altssh.gitlab.com` are their forges').
  - `repository.rs` — `ForgeRepository` (a host and a path: GitHub's `owner/name`, GitLab's
    `group/…/project`; `owner`, `name`, `is`, web pages of it, a request (`/pull/12`,
    `/-/merge_requests/12`), a commit, a branch, its name escaped). `forge_repository`: a
    remote URL of any form (https, ssh, scp-like, `git://`) to its repository; an ssh host
    alias through `resolve_ssh_alias` (`ssh -G -- <alias>`'s `hostname`, 2 s at most; never an
    option).
  - `http.rs` — `Http`, a ureq agent per host: rustls on aws-lc-rs (`deny.toml` bans `ring`
    and `webpki-roots`), certificates checked by the platform verifier (the system's trust
    store), the environment's proxy (none for the loopback address), no redirects, 10 s to
    connect and 60 s in all, answers of 32 MiB at most; the token a sensitive bearer header,
    built in a zeroizing string, and refused without HTTPS (but to the loopback address);
    GitHub's media type and API version only to GitHub; either forge's rate-limit headers.
  - `account.rs` — `Account`: a host's sign-in under a repository's configuration: the
    credential filled without asking on first use; `sign_in` asks (labelled) and checks it
    (who it is); a 401 rejects and forgets it (`Refused`; finding none after that is still
    `Refused`, until a credential is accepted), the first success approves it.
  - `api.rs` — the calls, each sent to its repository's forge: `pull_requests` (open, most
    recently updated first, 100 a page, each handed over as it lands, up to a limit),
    `pull_request_details`, `repository_info` (default branch, the repository it was forked
    from), `create_pull_request`. `model.rs` — what they give, alike for both forges
    (`PullRequest`, `PullRequestDetails`, `CheckState`, `ReviewDecision`, `NewPullRequest`:
    a fork's `head` repository, …). `answer.rs` — answers read alike: GraphQL data or errors,
    REST JSON, rate limits (by either forge's headers, or `RATE_LIMITED`) saying when they
    reset, `NOT_FOUND`, refusals with either forge's reasons (GitHub's validation messages,
    GitLab's `message` as a sentence, list, or fields); `parse_time`.
  - `github.rs` — GitHub: GraphQL for pull requests, details (checks one by one, latest
    reviews), and the repository; REST for opening one (`owner:branch` from a fork).
    `gitlab.rs` — GitLab: GraphQL (`project(fullPath:)`) for merge requests (`state: opened,
    sort: UPDATED_DESC`; approved by whom, the head pipeline's status) and details (the
    pipeline's jobs, not retried; a failure allowed is neutral; who approved; whether it
    conflicts); REST for the project (`GET /projects/:path`: its id, default branch, the
    project it was forked from) and opening one (in the branch's own project; from a fork,
    aimed at the target's id; a draft by GitLab's own `Draft:` mark).
  - `testing.rs` (feature `testing`) — `FakeForge`: either forge's API on the loopback address
    (pages, details, repository, who is signed in, opening; the bearer token checked; a rate
    limit to switch on; requests recorded). `tests/github.rs` and `tests/gitlab.rs` run each
    call against it, signed in through a sandbox's `store` helper (and an askpass script).
- `crates/egit-gix` — egit's reads through gitoxide (`gix` 0.89 without default features:
  local reads only, no network or TLS; its dynamic pack cache, as the static one needs
  MPL-2.0 `uluru`), each giving what egit's git commands give, and measured faster (phase 12);
  no egui.
  - `lib.rs` — `GixRepository::open`: `None` (git reads it) for references in a reftable or
    replaced objects (`refs/replace/`, which `cat-file` reads in their place); shared behind an
    `Arc`, each read taking a thread's own handle. `references`: `list_references`'s list
    (peeled through every tag: a packed ref's peeled line, else objects' headers, only tags
    decoded). `tips`: what `rev-list --no-walk=unsorted --exclude=refs/stash --all` lists, in
    its order (refs by name, `HEAD`, other worktrees' `HEAD`s; commits only, each once, a set
    saying which were met: 100,000 refs at distinct commits listed in 135 ms, not 5.8 s).
    `read`: objects as `ObjectReader::read` hands them over, saying how many: in a partial
    clone it stops at the first object it hasn't, for git to fetch; `reader`, a `GixReader`
    of a thread's own (its handle and buffer), for reading on several. `tests/parity.rs` checks
    each against git's (every kind of ref, packed and loose, another worktree; 50 random
    histories; a blobless clone read by both; replaced objects and reftable left to git);
    `tests/measure.rs` measures them against a large repository (`EGIT_SCALE_REPO`).
- `crates/egit-graph` — the commit graph's lanes; no dependencies, knows nothing of git
  (commits are any `Copy + Eq + Hash` key).
  - `layout.rs` — `GraphBuilder::push` lays out each commit as `walk_history` yields it, in
    time proportional to its parents and the lanes meeting it (never the graph's width: lanes
    leading to a commit are chained from a hash table, free slots found in a bitmap). Rows
    (12 bytes) and their edges are kept in `Chunk`s of 1024 rows, each with the lanes open at
    its start. Linux: 1.49M rows in 21 MB, laid out as fast as git walks them.
    `GraphBuilder::pinned`: the first lane kept for a branch's tip's first-parent line
    (pinned to the left), empty until the tip comes, which starts its lane there (its
    children's lanes join it); a node taking the leftmost lane leading to it, the line keeps
    the lane, and nothing else takes it.
  - `draw.rs` — `for_each_row` draws any range by replaying at most one chunk;
    `RowView::segments_in` gives a row's lines crossing a window of lanes, so a graph wider
    than its column costs only what shows.
- `crates/egit-text` — text on a grid of cells, as a terminal shows it; no egui.
  - `text.rs` — `Text`: bytes (any; at most 4 GiB) and line starts (`u32` each, counted
    before they're stored); a final `\n` ends the last line.
  - `cells.rs` — `Cells`: each character's `Glyph` and width. Printable ASCII 1, wide and emoji
    2 (East Asian Width, ambiguous narrow), combining marks 0 (drawn over the cell before),
    tabs to the next stop (8 by default), C0 controls `^X`, bad bytes `<FF>`, invisible and
    direction-changing characters (zero-width spaces, bidi controls, BOM) and C1 as `<200B>`;
    only a `\r` ending a line may be hidden. Nothing reorders.
  - `measure.rs` — `Measure`: each line's cells, measured once in the background (printable
    ASCII by its length: 66 MB in 14 ms), and for lines over 4 KiB a checkpoint every 4,096
    cells, so `seek` finds any cell reading at most that much.
  - `rows.rs` — `Wrap::{Off, At(width)}`: line `k`'s row `r` shows cells `r * width` on; a wide
    character in a row's last cell hangs past it, so rows never depend on earlier rows.
    `Position { line, cell }` (a row's start) with `next`/`previous`; `Rows`, the rows before
    each 1,024 lines (rebuilt at a new width in 1.3 ms for 1M lines), only for scroll bars.
  - `words.rs` — `changed_words`: two lines' changed bytes, by the longest common sequence of
    tokens (words, whitespace runs, single punctuation); none past 500 tokens a line, or for
    lines sharing under 40% of their words.
  - `find.rs` — `find_all`: a query's matches as byte offsets (`memmem`, or ASCII case folded
    by a two-byte scan), in 4 MiB windows overlapping by the query's length, asking whether
    to stop between them; `matches_case` is smart case; `Needle`, a query made ready for
    many short texts (a finder built once, or ASCII folded).
- `crates/egit` — the application (`egit_app` library, `egit` executable). eframe/egui 0.36
  (wgpu), `egui_tiles` 0.17, `egui-table-kit` 0.7 (a git revision ahead of crates.io: bump
  `rev` to the owner's pushed commit after each change made there; until it is pushed, cargo
  fetches it from the clone: `GIT_CONFIG_COUNT=1
  GIT_CONFIG_KEY_0=url.$HOME/egui-table-kit.insteadOf
  GIT_CONFIG_VALUE_0=https://github.com/xangelix/egui-table-kit CARGO_NET_GIT_FETCH_WITH_CLI=true
  cargo fetch`), `egui-async` 0.7 (stay on 0.36 until `egui_tiles` and `egui-phosphor` move).
  - `main.rs` — started as `egit-askpass` it is the askpass helper (`run_helper`; on Windows
    in any case, `.exe` or not); otherwise `lib.rs`'s `run`: `-h`/`-V`, paths to open,
    `EGIT_LOG` logging, the runtime, the window (no console window behind it on Windows,
    but in a debug build).
  - `app.rs` — `EgitApp { tabs, state }`. Views never change `AppState` directly: they push
    `Action`s, applied after the frame (`apply_actions`); `AppState::poll` (each frame's
    start) turns finished jobs into state or actions. Git is detected by a job
    (`Bind<Git, Problem>`); paths asked for before it is ready wait in `waiting`. The tabs
    open at the last close (`Settings::open`) come back. Profile edits are `ConfigChange`s
    (set or unset a key, add or remove one of its values) written under the user's
    `WriteLock`, then the user's and every repository's settings are read again. Web pages go
    through `AppState::opener` (`desktop.rs`'s `Opener`: the browser, or for tests a list,
    `recorded`; `Action::OpenUrl`).
  - `jobs.rs` — the runtime (2 workers, at most 16 blocking jobs: git processes queue
    beyond), `blocking` (a closure on the blocking pool as a future for a `Bind`), and
    `WriteLock` (taken inside jobs, never on the interface thread).
  - `tabs.rs` — the window's `egui_tiles` tree: `Pane::{Home, Profile, Preferences,
    Repo(id)}`; `show` focuses or adds a tab; `front` (the tab in front), `close_front`,
    `show_nth` (⌘W, ⌘1 … ⌘9); Home can't be closed; closing a repository's tab pushes `Closed`.
    Tabs underline under the pointer (`widgets::tab_hover`; egui_tiles' don't answer it), and
    their close button is 24 points. The selected tab is the chrome's tone, joining what it
    shows below; the bar and the other tabs are deeper (`Palette::recess`).
  - `preferences.rs` — `Preferences` (kept in the settings, every field defaulted so older
    settings read): the text's size (70–200%, egui's zoom, which ⌘+ ⌘− ⌘0 change too: followed
    back), the background fetch (`FetchEvery`: never, every minute, 5 (the default), 15, or 30
    minutes), the sidebar shown when a repository opens, a file's first look (`FilePreferences`:
    diffs side by side, lines wrapped, a tab's width of 2, 4, or 8 cells, lists of changed
    files as a list or a tree of their folders (`FileLists`, chosen too by each list's header),
    and the merge editor's sides above its merged file: `merge_panes`), and commit dates
    written or as how long ago (`Dates`). `apply` puts them into egui (the zoom, and the context
    the views read them from: `Preferences::of`), and the tab width where every text measured
    from then on takes it (`text_options`).
  - `help/` — Help: topics in six areas' files (`working.rs`, `history.rs`, `branches.rs`,
    `remotes.rs`, `rewriting.rs`, `inside.rs`; `topics()` walks them all), each a `Topic` (its
    `Group`, title, `on`: the words of the controls it explains, whole or with `*` standing
    for any words ("Merge * into *"); what it does at git's level, the git egit runs for it,
    and `cases`: the questions people meet, cancelling a rebase part-way, which side is
    "current" in a rebase, a pushed commit reworded, a submodule that won't sit still, LFS
    pointers, and their answers). Alt held over a control shows its topic beside it: the
    shared controls (`widgets`' buttons, tools, links, checkboxes, radios, segments, facts'
    labels, section titles, headings; menus' items, submenus, and headers) `note` their rect,
    layer, and words as they are drawn, only while Alt is held; custom ones (a commit's row
    and its columns, badges, the work row, sidebar rows and sections, banners, the branch
    label, a heading naming a path: `heading_of`) `note_topic`, which wins over words at the
    same place; at the frame's end (`show`) the smallest noted rect under the pointer on the
    topmost layer picks the topic (`for_label`: the pattern naming most of the words, exact
    words best; a commit's name, 7–64 hex digits, its own topic), drawn in an overlay beside
    it; tooltips wait while Alt is held (`begin_frame`). `HelpWindow` (F1, the status bar's
    Help, the palette): every topic by group, found by what is typed (titles, words, texts,
    cases; the list made again only when the query changes, drawn a row at a time). Its
    tests: ids once, every pattern claimed by one topic, every `note_topic` in the sources
    naming a topic that exists.
  - `commands.rs` — what the keyboard can ask: `Command` (the palette, help (F1), preferences, open,
    clone, home, profile, close the tab, a tab by number, a theme, a recent repository) and
    `RepoCommand` (fetch, pull, push, undo, redo, search, the sidebar shown or filtered, the
    reflog, the work tree, the requests, stash, patches, a branch made or switched to, read
    again), each's words and shortcut; `pressed` takes the application's own shortcuts at a
    frame's start (⌘K and ⇧⌘P, ⌘,, ⌘O, ⌘W, ⌘B, ⌘R, ⌘1 … ⌘9; the views take theirs), the more
    specific first. `keys_text`: a shortcut written as a Mac shows it (`⇧⌘Z`) shown so on
    macOS and as `Ctrl+Shift+Z` elsewhere (egui's command key is Ctrl there). `score`: how
    well typed letters match words (in order; word starts and runs count more). The
    Preferences tab lists every shortcut (`Command::shortcuts`).
  - `repo.rs` — `RepoTab`: `Repository`, `Bind`s for `Status` (head, operations, a stopped
    `git am`'s progress, and a bisection's `Bisection`) and the
    repository's `ConfigSnapshot` (`request` keeps showing the old state until the new one
    lands) and its stash list, worktrees, submodules and where they stand
    (`submodule_states`, read again too whenever a new status of the work tree lands:
    `follow_submodules`), and Git LFS (`lfs`: `refresh_lists`, read at once, when refs change,
    and after each task), its
    `HistoryState` and `HistoryView`, its `WriteLock` (writing a commit-graph), its
    `Journal` (what to undo and redo; `undo` runs the next as a task, and a commit made notes
    how to take it back),
    what it shows (`panel`: `RepoPanel::{History, Overview, Configuration}`, the pages a
    bar over them away, `Action::ShowPanel`), and its `Watcher`
    (`follow_changes`: refs or operation state changed → status read again, refs → history
    read again; config → config; the index, refs, state, or a file → the `WorkTree`'s
    status; a task ended → status, configuration (an upstream set, say), and work tree read
    again, its refs left to the
    watcher, so a history is walked once). `poll` (each frame, every tab) takes in its jobs'
    results.
  - `history.rs` — `History`: a walk (`jobs::detached`) streams `Piece`s through a channel:
    the refs and whether a commit-graph exists, then each completed `egit-graph` chunk with
    its commits' names packed (20 bytes each), then the end. `id(row)`, `row_of(id, near)` (a
    scan from `near`; no id→row index is kept), `Refs::at(id)` (sorted, binary search).
    `Refs::has_branch` (a local branch by name, for switching to a remote's).
    `Objects`: gitoxide's reader where `egit-gix` opens the repository (opened again at each
    walk), else one shared `cat-file` per repository (restarted after a failure), which also
    reads what gix didn't: a partial clone's objects it hasn't (git fetches them), and all
    once gix failed; `EGIT_GIX=0` leaves every read to git. The walk takes its refs and tips
    from gix too, and has git walk from them (`walk_history_from`). `Names`: the rows'
    commits' names, a chunk (`Arc<[u8]>`) at a time, shared with jobs as the history goes on.
    `Scope`: only some refs' history (soloed), or every one's but some (`hidden`, sorted:
    the walk starts from `history_tips`, the table shows no badge for them), a branch pinned
    to the left; `HistoryState`
    keeps the one asked for (`set_scope` reads again), the walk resolves its names against the
    refs it read (walking from those found, `GraphBuilder::pinned`) and says what applied, which
    a line under the top bar shows ("Showing only …", "Show every branch"; "… pinned to the
    left", "Unpin"; "2 branches hidden", "Show them"). `Refs` are shared (`shared`, an
    `Arc<[Reference]>`) and numbered by read (`read`), so the sidebar sorts them in a job and
    knows when they changed.
    `Summaries`: subject, author, preformatted date (the viewer's time zone), co-authors (none allocates nothing), read
    as rows come into view (at most 512 a read, one read at a time), about 4,096 kept, least
    recently drawn forgotten first; each says whether its commit is signed. `Signatures`: the signed commits on screen (and the one
    selected) checked with `check_signatures`, 64 at a time, one batch at a time (each check
    runs a program); a failed batch counts as checked without a verdict, so it isn't retried
    every frame; forgotten when the configuration is read again.
    `read_details` (the commit and its `commit_changes`); `read_change_counts` (its
    `line_counts`, lined up with the changes by path should their orders differ, and their
    total). `HistoryState`: the history shown,
    and a re-read (`reload`) that takes over only once complete (`replaced`), so the table
    never empties or jumps meanwhile.
  - `search.rs` — searching the history: `SearchKind` (message, author, path, commit); a
    `Search` (a detached job, stopped when dropped) marks what it finds in `Found`, a bit a
    row the table reads as it draws (atomic: no lock; `next`, `previous` wrapping, `rank` for
    "3 of 120", progress). Messages and authors: every row's commit read here, through a
    `GixRepository` opened for the search alone (its packs unmapped as it ends), on up to 8
    threads taking 4,096 rows at a time from the top, a `Needle` with smart case against the
    message (after the headers' blank line) or the author's `Name <email>`; where gix doesn't
    open the repository (or fails), `find_commits` instead. Paths: `find_commits` (refused
    unless `has_every_tree`). Names: 4+ hex digits found among the rows, else
    `resolve_commit`. What git finds gets its row from a `RowIndex` (each row's name's first
    four bytes beside the row, sorted: 8 bytes a row), built by the first search that needs
    it and kept, shared, until the history is replaced.
  - `watch.rs` — watches (`notify`: inotify; `FSEvents` on macOS, `ReadDirectoryChangesW` on
    Windows) on git directories, configuration files, and a work tree: on Linux its folders,
    never a work tree recursively (inotify's watches are one a folder, and limited); on macOS
    and Windows, where one watch takes in a whole tree, the work tree at once (`WHOLE_TREE`;
    the folders asked for are in it). Linux's: a repository's git dir and common dir
    (non-recursive) and `refs/` (recursive);
    the user's configuration files through their folders (editors replace files). Paths are
    sorted into `Changes` (refs, state, config, index; `*.lock`, objects, logs ignored), access
    and attribute events ignored, and handed over once quiet for `SETTLE` (120 ms), so one git
    command causes one refresh. The app watches the files its user snapshot names and reads
    every configuration again when one changes. `Watching` keeps a watch that couldn't be set
    up (inotify's per-user limits, often used up by editors) trying again, backing off from
    1 s to 30 s, and hands over `Changes::ALL` once it works, as it does when inotify's queue
    overflowed (a rescan); meanwhile the status bar says outside changes aren't followed.
    `TreeWatcher`: a work tree's folders, each on its own (`Changes::WORK_TREE`); its owner sets
    which (`watch`, from a job: a system call each), and it notes folders created (for the owner
    to consider) and removed (their watches went with them).
  - `worktree.rs` — `WorkTree`: a repository's `Status`, read at once after egit's own writes
    and when the window regains focus, soon after the repository's watcher (index, `HEAD`,
    operations) or the work tree's; one read at a time, at most one a second while files keep
    changing, every 5 s while the work tree can't be watched. The read's job also sets the
    folders watched: the top, `HEAD`'s (`tree_folders`, cached by commit), those of the
    status's entries, and folders created since (with up to 1,024 inside) that `ignored_paths`
    doesn't name, so `mkdir -p` then a new file is seen, and `node_modules` costs no watch.
  - `grid.rs` — the text grid: a `GridSource` (a measured `egit_text::Text`, colours, line
    backgrounds, a gutter) drawn as a terminal draws it. Each visible row is one galley of
    sections of one colour and advance, letter-spaced so every glyph takes exactly its cells
    (egui spaces before each glyph but a row's first, so a section's leading space trades its
    spacing for the previous one's; thin spaces take half a cell, egui's rule). The view keeps
    the `Position` at its top and an offset: wheel, keys (↑ ↓ PgUp PgDn Space, ⌘Home/End;
    ← → Home End sideways), and its own scroll bars (from `Rows`, rebuilt when the width or
    text changes; the thumb follows the part of a row scrolled too). It stops at its end with
    the last row resting on the bottom edge (`end_of`, each frame; a text that fits stays at
    its start): the wheel at the end moves nothing, rather than creeping a part of a row and
    snapping back. Rows' backgrounds and gutters are clipped where the text is (above the
    sideways bar). Advances are cached per character until egui's font count changes
    (`fonts::generation`). `show_panes` draws sources side by side, scrolled as one (one
    position, input, and scroll bar; the first pane's measure moves the rows). Dragging
    selects in one pane between cells (`selected_lines` gives the lines it covers; hit-tested
    against the rows drawn last frame;
    dragging past an edge scrolls), ⌘A selects a pane, Escape clears (focused or not: egui
    drops focus on Escape), and ⌘C (`Event::Copy`) copies the bytes as they are. `find.rs`:
    ⌘F opens a find bar over the top right (Enter, Shift+Enter, Previous, Next; Escape or
    Close); every pane is searched in a job as the query changes (its text shared), at most
    100,000 matches a pane; matches on screen are marked, the current one stronger, and moving
    to one scrolls it a few rows below the top and sideways into view. `Plain` is a text,
    numbered, and highlighted when `named`. A source's `colors` give its text colours,
    `backgrounds` its highlights; `begin_frame` and `needs_repaint` let a source work a little
    each frame (a repaint 30 ms later while it isn't done).
  - `diff.rs` — `DiffDocument`: a `FileDiff` measured as a `GridSource` (lines on their kind's
    background, hunk headers muted, the gutter numbering both files with a `+`/`-` marker), and
    `read_diff`, its job (cancelled when its view closes). Each block's removed lines pair with
    its added ones in order (found in the job); a pair's changed words are compared when first
    drawn and kept (up to 4,096 pairs), and marked with `Styled` backgrounds. `SplitDiff`:
    the old side beside the new (`split_diff`, a job, the first time it is shown and again for
    each diff read after, once it has landed: `is_of`), a block's
    removed lines beside its added ones with fillers making up the difference, both sides'
    measures matched (`Measure::match_rows`) so wrapped lines take the same rows. Both are
    highlighted in the file's language (see `syntax.rs`). `read_uncommitted_diff` reads a
    work tree's file's diff; `changes_in` picks the changes among some lines (or all of their
    hunks), and `file_diff` gives the diff back to stage them. A Git LFS pointer's diff is noticed
    from the diff itself (`lfs_change`: one hunk, every side present a pointer, which
    `history::LfsPointer::parse` reads) and shown as the file's sizes before and after, its
    own changes a button away.
  - `syntax.rs` — `Syntax`: syntect's grammars and themes (through `fancy-regex`; its
    `bincode` advisory is ignored in `deny.toml`: it reads only the dumps compiled in) for the
    language a path's extension or name, or a first line, names. A walk in the background
    parses the whole text once, keeping both parsers' states every 128 lines (one equal to the
    one before is shared: 2.5 MB for 1M lines of C, parsed in 40 s); a line drawn is coloured
    from the nearest state (or where the last colouring stopped), within 3 ms a frame, and
    the latest 4,096 lines' colours are kept. A diff's lines feed an old and a new parser as
    their `Feed` says (context both; a hunk header starts both over). Lines over 4 KiB, and
    lines that aren't UTF-8, are left plain. Dark: `base16-ocean.dark`; light:
    `InspiredGitHub`.
  - `repo.rs`'s `RepoTab::commit` (`Action::Commit`) runs `create_commit` under the
    `WriteLock`; the watcher brings the history and `HEAD` along after.
  - `staging.rs` — `StageRequest`s as jobs under the repository's `WriteLock`: whole files
    (`stage`, `unstage`), or the lines picked in a file's diff (`LinesRequest`: the index's
    blob read through the shared `cat-file`, `apply_lines`, `write_index_content` against the
    entry the diff was read with; a stale diff says so), or a conflict's resolution
    (`ResolveRequest`: each conflict as chosen, built into the file in the job, a whole side,
    or deletion, through `resolve_conflict`; sides changed since say so). `RepoTab::stage` runs
    one (`Action::Stage`), keeps its result for the panel, and reads the work tree again after.
  - `merge.rs` — a file in conflict: `read_conflict` (a job: `conflict_sides`, `merge_sides`,
    `incoming_commit` for the operation in progress) makes a `ConflictFile`; `Choice` (open, a
    side, both in either order, the base, edited text) per conflict; `MergeDocument` (`build`,
    a job, again whenever a choice changes): the merged text with each open conflict's sides
    under headers ("Conflict 2 of 5 · current: HEAD (main)", "incoming: side · 1234abcd …",
    the base's with `show_base`, "(no lines)" for an empty side) and each resolved one's lines,
    numbered as the resolved file will be, `<`/`|`/`>` in the gutter, the current side's lines
    through the old parser and the incoming side's through the new; `start` and `conflict_at`
    map conflicts and lines. `Shown` draws it with its selected conflict's headers stronger.
    `SideDocuments` (`build_sides`, a job, once per conflict read): each side whole, every
    conflict taking that side (`SideDocument`: its lines numbered as that side's file, each
    line's conflict, each conflict's first line); `SideShown` marks a side's conflicts in its
    colour, the selected one stronger, `<` or `>` in the gutter.
  - `blame.rs` — `start_blame`: `blame` in a detached job, its runs handed over in batches
    (every 256 runs or 50 ms) into a `Blaming`'s `BlameState` (each line's commit and whether
    a run starts there, the commits found, their time span) that the view reads each frame,
    so the first lines show before git is done; `read_blamed_file` (`tree_entry`, then
    `read_file`; or the work tree's file). `BlameSource`: the file (highlighted) as a
    `GridSource`, the gutter labelling each run's first line (`1a2b3c4 Ada Lovelace  3y`, 26
    cells) beside its number; the selected line's commit's lines marked, uncommitted ones as
    incoming, the rest shaded by how new their commit is among those found.
  - `tasks.rs` — `Tasks`, what the user asks of a repository: one `Task` the user
    asked for at a time (fetch `--all --prune`, pull as configured or by merge or rebase,
    push a branch or a tag, delete a remote's branch, abort or continue an operation,
    remember `pull.rebase` then pull; switch, stash then switch, create, rename, or delete a
    branch, tag or delete a tag; merge, rebase onto, cherry-pick, revert, reset; rebase
    interactively, drop a commit, squash one into its parent, reword one (`Reword`: `HEAD`'s by
    `reword_head`, which uncommitted changes don't stop, an earlier one's by a rebase) (their
    plans made in the job from
    `rebase_range`; `range_problem` says why a range won't do); stash, apply or pop or drop a
    stash; `Action::Task`), run by `run` through five groups (remotes, history, rewriting,
    references, stashes), under the `WriteLock`, its `Progress` shown and
    `CancelToken` stopped by `Action::CancelTask` (`Outcome::Cancelled`, not a failure).
    Besides git's results, an `Outcome` can be `Done`, `Integrated` (done, conflicts, empty),
    `Stashed` (whether anything was), `WouldOverwrite` (a switch refused, with the paths), or
    `NotMerged` (a delete refused). And a background fetch as often as the preferences say
    (`fetch_every`; every 5 minutes by default, or never; the first 10 s after opening; only
    with a remote): non-interactive, without the lock, stopped when the
    user starts a task, its failure kept apart (`background_problem`; `background_fetches`
    counts them, for the forge); what it brought the current branch's upstream (`Brought`:
    `upstream_tip` before and after, `count_between` them; a later fetch continuing where an
    earlier one ended adds to it) kept in `brought` until the user starts a task, and shown
    under the top bar while the branch is behind (`views/tasks.rs`'s `brought`: "↓ 3 new
    commits on origin/main", fetched in the background; Pull, Show them (the upstream's
    commit selected), Dismiss). `remote_names`, `has_remotes` (a scan, nothing allocated),
    and `upstream_of` read the configuration. Fast-forwards (`FastForward` the current branch,
    `FastForwardBranch` another from a remote or a ref here: `Outcome::Forwarded`),
    `SetUpstream`, `DeleteBranchEverywhere` (here, then on the remote; refused here as not
    merged: `NotMerged`, nothing deleted), `AddWorktree`, `SavePatch` (`format_patch`, the
    file written in the job). Patches: `ApplyPatch` (its bytes shared, `Patch`), `Skip`,
    `CopyPatch` and `CopyChanges` (`Outcome::Copied`, its text, copied as the outcome is first
    shown; refused when empty or not UTF-8), `SavePatches` (`Outcome::SavedPatches`),
    `SaveChanges`. A forge's tasks run with the tab's accounts:
    `CheckOutPullRequest` (`fetch_refspecs` of its refspec, then a switch; changes in the way
    lead to stash-and-switch, as a switch's do) and `CreatePullRequest` (`Outcome::Opened`).
    The repository's plumbing: `AddRemote` (fetched at once if asked), `FetchRemote`,
    `RenameRemote`, `RemoveRemote`, `SetRemoteUrls` (the push address unset when none);
    `RemoveWorktree` (`Outcome::WorktreeChanges` when it has changes: asked again, forced),
    `LockWorktree`, `UnlockWorktree`, `PruneWorktrees`; `UpdateSubmodules`.
    Every task but these reads what its undo needs first (`undo::before`) and notes how to take
    it back once it went through (`undo::noted`, into the tab's journal); `Undo` and `Redo`
    run `undo::take_back` and `do_again` instead (one stopped before changing anything goes
    back on its list).
  - `undo.rs` — taking back what egit did. An `Undo`: what it was in words ("the merge of
    topic", "the commit “Fix it”") and its `Change`s in order: `HEAD` moved (from, to, and
    the `ResetMode` that moves it back and forth: soft for a commit, a soft or mixed reset's
    own, keep otherwise), a switch (`Place`: a branch, or a commit detached), a branch made,
    removed, renamed, or moved (a fast-forward of another), a tag made or removed, changes
    stashed (only a stash the action made) or a stash dropped. Each change is checked before
    it is reversed (`HEAD` where the action left it, a branch or tag where it was, the stash
    still the latest), last first; done again first first, a stash pushed again noted anew.
    `Journal`: up to `KEPT` (100) to undo and those undone, to redo, until a new action; jobs
    report through its inbox (`Landed`), taken in each frame. A stop says why (`Halt`: moved
    on, or git's words, and whether part was done) and whether it can be tried again. Pushes,
    fetches, and what an operation stopped part-way did aren't taken back.
  - `lfs.rs` — a tab's Git LFS: `read_lfs` (`Reading::WhereUsed` for the tab's lists, which
    runs nothing unless the top `.gitattributes` gives a pattern LFS's filter; `All` for its
    page): git-lfs's version, `lfs_set_up` from the configuration, the patterns (from
    `.gitattributes` where git-lfs isn't installed); `LfsState::trouble` (used, but git-lfs
    isn't installed, or isn't set up here); the files and locks read for the page;
    `LfsTask` (set up, track, untrack, fetch, check out, pull, push, prune, lock, unlock),
    `Task::Lfs`, run by `lfs::run`, its words (`doing`, `done`: what to do next, "commit
    .gitattributes to share it"). A banner under the top bar where LFS is used but its files
    can't be whole here (`lfs_banner`: why, and "Set up Git LFS here" or "Open Git LFS").
  - `submodules.rs` — a tab's submodules: `read_states` (nothing run without `.gitmodules`;
    `submodule_status --recursive`, then, for those elsewhere, `submodule_details`: a
    `Standing`'s `behind`, those lacking commits recorded, which a pull or switch here moved
    on, apart from those only past them, someone's work to record); `OutOfStep` (not set up,
    behind, past, in conflict, addresses out of date by `url_is_stale`, a relative one
    resolved against `remote.origin.url` as git resolves it; `any` leaves out those only
    past: no fault) and its words; `read_details` (a selected one's inside, and its commits
    the commit recorded lacks) and `read_move` (a gitlink's change: the commits it brings,
    none known when it isn't here); `SubmoduleTask` (update to the commits recorded or the
    latest, sync, `Mend`: sync then update `--init --recursive`, deinit, add, remove, change
    an address or a branch, `submodule.recurse` on or off), `Task::Submodules`, its words
    (what to do next: "record the new commits here", "commit .gitmodules"). A banner under
    the top bar where any is out of step (`submodules_banner`: what, "Mend everything",
    "Show them", dismissed until what it says changes).
  - `forge.rs` — `RepoForge`, a tab's forge side: `configured` (whenever new configuration
    lands) works out in a job which remotes are GitHub repositories or GitLab projects
    (`remote_urls`, as git rewrites them, else as `remote.<name>.url` writes them; the hosts
    listed as `egit.githubHost` and `egit.gitlabHost`); an `Account` per host (the
    repository's credential configuration); the open requests read in a detached job, each
    repository up to `LIMIT` (1,000),
    each page into a shared `Found` as it lands (the window woken); a read again
    (`refresh`: after any task that exchanged with a remote, each background fetch, a
    sign-in, and Refresh) takes over only once complete, a failed one keeping what is shown
    and saying why (`ForgeProblem`: sign in, refused, rate limited until, other). Badges:
    head commits sorted with their pull request's number and checks, made again when the
    list grows or is replaced (`badges_at`, a binary search). `pull_request_for` (a branch's
    open one), `remote_of`, `repository_of`, `sign_in` (asks through egit's prompts;
    `signing`, `sign_in_problem`); `kind`, `kind_of`, `nouns`. Each forge's words:
    `request_noun` ("pull request", "merge request"), `request_number` (`#12`, `!12`), and
    `fork_head` (`refs/pull/12/head` into `pr/12`, `refs/merge-requests/12/head` into
    `mr/12`).
  - `desktop.rs` — `Opener`, and `is_web_page` (HTTPS, or HTTP to the loopback address;
    nothing that could pass for an option): `xdg-open`, else `gio open` (macOS's `open`;
    Windows' `rundll32 url.dll,FileProtocolHandler`, no shell reading the address), in a job.
    `FileDialogs`: the desktop's file dialogs (`rfd`, one at a time) for a `FileQuestion`:
    where a patch or the changes go (a file), where a series goes (a folder), which patches to
    apply (files, patches first); for tests a folder answering them all (`in_folder`).
    `Action::AskFiles` asks, and `AppState::poll` turns the answer into a task, or shows the
    patches (`Action::OpenPatches`).
  - `fonts.rs` — system fonts for characters egui's lack (CJK, most scripts): views note them
    (`note_missing`, `note_text`, nothing to check for ASCII); `Fallbacks::poll` (each frame)
    looks once per character, in a job, through the system's font directories (XDG's; macOS's
    and Windows' own; CJK families, theirs too, then
    monospace, then regular weights; in a collection the face covering most, monospace and in
    the user's `LANG` first; outlines required, so no colour bitmaps) and adds what it finds as
    lowest-priority fallbacks to every family (monospace, and each weight). Kept fonts are leaked once into `'static`
    slices: egui copies owned font data on every rebuild of its fonts, but borrows static.
  - `views/history.rs` — the history's chrome (the top bar, the sidebar, the details: `chrome`,
    the panel tone) around the commit table on the window's background. The top bar: Undo and
    Redo, then the views and menus as quiet tools
    (Search, Reflog, Stash, Patches, LFS where the repository keeps files in it (its state;
    pull, fetch, check out, push to each remote, prune; its page), Submodules where it has
    some (how they stand; mend everything, update all to the commits recorded or their
    branches' latest, sync addresses; their page), and Repository: its Overview, Settings,
    the submodules' and Git LFS's pages, and reading it again), their words beside their icons where the bar has room for all it shows
    (`Look::Full`), their icons alone elsewhere (`Look::Compact`, their words on hover), the
    commit count (a spinner beside it while the history is read), why the work tree couldn't
    be read (when it couldn't), the requests; at its right `views/tasks.rs`'s bar (drawn first,
    into the row; its words capped at 300 points). The look is chosen from `BarRoom`: the left
    part's width drawn compact last frame (room kept for the spinner, so its coming never
    changes it) plus what the tools' words add (`widgets::tool_width`), against
    the room the task bar leaves (its idle width), and held while a task runs: never flipping
    back and forth; the left part is clipped to its room. Then the
    commit table on egui-table-kit's layout `Table` (its `TableKit` keeps per-row state a
    graph doesn't need): `CommitRows` is the `TableDelegate`; under the column titles, pinned
    as a second header row while there are changes not yet committed, the work in progress
    (`work_cell`: a hollow node on `HEAD`'s lane with a dashed line towards it, "3 uncommitted
    files · 1 staged", counted once a status read (`WorkCounts`); a click shows the working
    tree beside the history in place of a commit's details, as it always is before the first
    commit); `prepare`
    replays the visible rows' lanes once a frame (`GraphRows`, panned to the nodes on screen
    when the column is narrower than the graph: "‹ Graph ›") and asks for summaries (the
    screen, then a screen below and above). Graph cells paint lines (cubic curves between
    lanes) and nodes (hollow for merges, ringed for `HEAD`); rows are buttons named by their
    subject; a bisection's refs as badges in its terms (bad, good, skipped; `bisect_badge`);
    ref badges (each its menu, a branch's double-click switching to it; dragged onto another
    ref's badge or sidebar row (`dragging`: the ref carried as egui's drag payload, its words
    following the pointer, the badge held over ringed in the accent), a menu of what that
    can do opens where it lands (`Dropped`, `drop_menu`; Escape or a click elsewhere closes
    it); in the details too; painted 18 points tall, so badges in rows next to each other never touch; one that
    doesn't fit in its cell is counted, "+2", rather than cut at the column's edge; in the
    details they wrap); a signed commit's seal by its verdict (good, doubtful, bad, or
    unchecked, its meaning on hover); a focusable area takes ↑ ↓ PgUp PgDn Home End. The
    columns' widths are egit's own (`ColumnWidths`, kept with the settings; table-kit's
    columns fixed at them): the message fills what the others leave, and when that's under
    its least (a panel open beside) the others give way as drawn, the graph first, then the
    author, date, and commit, so rows end where the table does. `column_lines` draws a line
    between each two columns over the table, every one dragged to trade width between its
    two columns (the message's by filling what is left), so the line follows the pointer and
    nothing else moves; a double click puts them back. Dates are the viewer's time zone's
    (`format::local_date_time`, jiff's system zone, at the offset each commit's moment had:
    summer time or not), monospace, the column titled with the zone ("Date (CEST)"); the
    details show the same, the commit's own clock on hover. `HistoryView::follow` keeps the
    selection and the top row's commit in place when a re-read takes over. The table's
    column titles are small and muted, and so is all but the subject (author, date, commit),
    which leads its row; the author's column shows the author's avatar and the co-authors'
    (`authors_cell`), and their names ("Ada and Alan", "Ada and 3 others"). Details: the subject as the panel's title, the description in a box
    of its own (`widgets::text_box`), then facts: names,
    parents (buttons: egui announces links as plain text), each person (their avatar, the
    name, leading, the email beneath, muted) and their date as a fact of its own, the
    co-authors (a line each, their avatar and name, the email on hover), the signature (its
    verdict, signer, and key), refs. A commit of the branch checked out (`CommitDetails::in_head`,
    read with them) has its message edited in place: a double click on its subject or
    description (they show it under the pointer), or the pencil at the title's right (named
    "Edit the message", saying why not while a task runs, an operation is in progress, or
    `HEAD` is detached), turns them into a summary field and a description (`MessageEdit`),
    what rewording does (`HEAD`'s: only its message; an earlier one's: a rebase giving it and
    what follows new names), Save message (⌘Enter) or Cancel (Escape); saving runs `Reword`,
    the selection staying on its row (`keep_row`); and the changed files under a section
    title ("Changed files" with their count, the lines gained and lost at its right, and
    `widgets::tree_toggle`, a list or a tree; named "3 files changed") in a box
    (`widgets::list_box`, `ChangesTable`, its tree a `TreeTable`), with the lines each gained
    and lost once counted
    (`Counting`: a job after the list, the previous one stopped; not in a partial clone);
    clicking one opens a `FileView` (`views/file.rs`) in the commit table's place (Escape or
    "History" goes back; its bar: back, how the file changed and its path, Blame and File
    history at the right; then Diff or File, Unified or Side by side, as segmented choices):
    its diff against the first parent (unified or side by side, with
    "Whole file" for all of its context), or the file as the commit left it (as it was, if
    deleted; read the first time it is asked for, `read_file`), or a placeholder for binary
    files, LFS pointers, submodules, and files over 256 MiB; an image (`pictures::is_image`:
    PNG, JPEG, GIF, WebP, BMP, ICO) shows its sides instead: before and after side by side (or
    the image as the change left it), each with its size in pixels and bytes, scaled down to
    fit (never up) over a checkerboard (a 32-texel patch laid as quads: transparency shows),
    read the first time (`read_pictures`, a job: a commit's blobs, or for the work tree
    `HEAD`'s, the index's, and the file) and kept as textures (`Textures`) until the status
    changes. A submodule's change (a gitlink either side) says where it moved from and to
    (its `HEAD` now, for a change not staged) and lists the commits that brings
    (`read_move`), in place of git's "Subproject commit" lines. A `FileView` of a work tree's
    file (`WorkFile`: a path, which `Uncommitted` changes, the index entry its diff was read
    against) reads its diff again whenever the status changes, and has a second bar: stage
    or unstage the file, the lines selected in its unified diff, or their hunks. A file in
    conflict shows `views/merge.rs`'s `MergeView` instead (and "Stage file", resolving it as
    the work tree has it), reading the conflict again when the status changes (its choices kept
    while its sides are the same), then, resolved, its staged changes. `MergeView`: "Conflict 2
    of 5" and its choice, Previous and Next (Alt+↑ and Alt+↓), how many are left, "Show the
    base", "Save resolution" (at the right) once none is open; then "Take" Current, Incoming,
    Both (current first), or Base (moving on to the next open conflict), "Edit…" (its text in
    a code field, Apply or Cancel; a line ending is added unless it ends the file), and More:
    both incoming first, Reopen, All current, All incoming, and "Both sides above" (the
    preference `merge_panes`, on by default); lines selected in a conflict select it. With
    both sides above: the current side and the incoming one whole, side by side over the
    merged file (45% of the height), each under a header ("The current side", where it comes
    from, "Take current"), each scrolled to the selected conflict as it changes, lines
    selected in either selecting theirs. A conflict that isn't two texts says
    what each side did and offers keeping a side or deleting the file. `Shared::operation`
    and `branch` label the sides.
    `views/work.rs` — `WorkPanel`: "Not staged" and "Staged", each a table-kit list (indices
    into the status, made once per read: `ListFiles`, shared with the job arranging its tree)
    with a button for all and one per file (named "Stage <path>"), or as a tree of their
    folders (the first list's header choosing; each folder a button too, "Stage the folder
    src", every file it holds), a file clicked opened in the `FileView`; then the commit form: summary
    (warned past 72 characters), description (⌘Enter commits), co-authors (chips with their
    avatars, each removable; "Co-author" opens a picker: a field typed into at once, the
    recent authors (`recent_authors` of the last 5,000 commits, read when it first opens; the
    user's own `user.email` left out) matching it by name or email, most commits first, eight
    at most, and whoever `Name <email>` typed names; each a `Co-authored-by` trailer), amend
    (a blank form takes the last commit's message, `read_message`, its co-author lines moved
    into the chips: git adds them back), sign off, sign (`commit.gpgSign` by default),
    and the last commit's output (its hooks') or problem (git's whole complaint). It clears
    once the tab's `commits_made` grows. A history of 10,000+ commits
    without a commit-graph is offered one (`Action::WriteCommitGraph`).
    `views/tasks.rs` — the task bar at the top bar's right: Push, Pull, Fetch, and the branch
    beside its upstream with ↑ ahead ↓ behind (or "(not pushed yet)", "(gone)"); while a task
    runs, its progress and Cancel. Under it the last task's outcome until dismissed (a line,
    the remote's notes, or a problem box), a refused push explained by its reason (behind:
    "Force push…" or Pull; a lease that failed: nothing forced, fetch first), and the banner
    of an operation in progress (a stopped `git am`'s "Applying patches stopped at 2 of 5:
    subject" and Show the patch; Show the conflicts, Continue, Skip, Abort…; a bisection's
    state (a commit to mark bad or good; "About 3 more to test: 9 commits could be the first
    bad one" with Good, Bad, Skip for the commit checked out; the first bad commit found, and
    Show the commit; one among several) and Stop bisecting;
    the headline and what to do on lines of their own, the buttons in a row below).
    Each task's end in words (`ended`, `done_text`: "Switched to topic.", "Picked 1234abcd.",
    "… changes nothing here: skipped."); a refused switch and an unmerged delete lead to
    their dialogs instead. Its parts draw in `Ui`s with ids of their own (`own_ids`): what the
    row drew before them comes and goes.
    `views/dialogs.rs` — the `Dialog` asked (`TasksView::dialog`), one at a time in a modal,
    what is typed kept in its variant, ending by starting a task or closing: diverged
    branches (Rebase or Merge, optionally remembered as `pull.rebase`), a force push's
    confirmation (`--force-with-lease --force-if-includes`), a branch's first push (remote and
    name; `--set-upstream`), an abort; a new branch's name (and "Switch to it"), a branch's new
    name, a tag's name and message (annotated with one), each checked by `check_ref_name` as
    it is typed (Enter confirms); and what is lost before deleting a branch (again, saying
    so, when it isn't merged), a remote's branch, a tag, or a stash, before a hard reset, and
    before stashing the changes in a switch's way, and deleting a branch here and on its
    remote at once (again, when it isn't merged). The stash dialog: a message, and untracked
    files too. A branch's upstream: typed, the remote branches holding what is typed offered
    as buttons, refused until it is one; "Forget <upstream>". A worktree: its folder (`~` the
    home folder; relative beside the repository), a new branch's name where one is made (none
    for a commit: detached), "Open it once made" (a tab, once, as the task ends). A remote to
    add (a name git takes and none here has, "origin" offered for the first; its address;
    "Fetch from it now"), its addresses (fetching, and pushing: empty is the same), its new
    name, its removal; a worktree's removal (again, "Remove anyway", when it has changes) and
    lock (why).
    `views/actions.rs` — the menus' pieces, which every menu is built from: `item` (an
    icon in a column of its own, then its words, which name it; disabled, why on hover),
    `danger_item` (what loses work, in the danger colour), `shortcut_item` (its keys at its
    right), `submenu` (an item with an arrow), `menu_header` (a group's name in small
    capitals over its items), and `menu_rows` (rows touching, 28 points each, their words
    8 points in). Long menus are grouped: a commit's (made here; into the current branch;
    rewriting it; bisect; reset; copy; patches) and a branch's (switch; into the current
    branch; its remote; renaming and deleting; copy; in the graph; its reflog). And what the
    commit table offers (`Menus`, from `Shared::menus`: the
    branch, `HEAD`, the configuration (its remotes read only when a menu needs them), the refs, whether a task runs or an operation is in progress,
    which disable what would change the branch, saying why): a commit's context menu (switch
    there detached, create a branch or tag there, merge it in, rebase onto it, cherry-pick,
    revert, reset soft or mixed, hard after a dialog, a worktree there, its Bisect submenu (mark
    it bad or good in the bisection's terms, starting one; skip it), copy a link to it on
    its forge; and in the row's menu, its name, subject, or "Save as a patch…"), a ref badge's
    (switch to a branch, a remote's through its local one tracking it, unless one of that
    name exists; merge it in, rebase onto it (or onto it keeping merges), fast-forward the
    current branch to it, and a
    remote's local branch following it; a local branch's Pull (the current one as configured,
    another fast-forward only), Push (to its upstream, or the first-push dialog), and Set
    upstream; a worktree from it; rename or delete a branch, delete a remote's on its remote,
    or both at once; copy its name, or a link to it on its forge; pin it to the left or unpin
    it, show only it, it too, or hide it (`Scope`, `Action::Scope`), or every branch again;
    push or delete a tag), a branch badge double-clicked (switch), a ref dropped onto another
    (`drop_items`, in a menu where it lands: onto the current branch, merging the dragged one
    in, rebasing onto it, or fast-forwarding to it; the current branch onto another, rebasing
    onto that (keeping merges or not), merging it in, fast-forwarding; else fast-forwarding
    the branch dropped on, or switching to it, a merge into it said to wait for that), and
    the stash menu beside
    the commit count
    ("Stash the changes…", then a row per stash, its message and age, latest first, in a
    list that scrolls past ten (only the rows in view drawn), its submenu holding its whole
    message and Apply, Pop, Drop…). Each asks a task or a
    dialog (`Ask`), or opens the rebase editor, carried out by the history view
    (`carry_out`). The commit menu's "Rewrite" group (on a branch, nothing running): "Rebase
    interactively from here…", "Edit the message…" (the editor, that commit reworded),
    "Squash into its parent", "Drop this commit…" (a dialog first).
    `views/rebase.rs` — `RebaseView`, the interactive rebase editor in the commit table's
    place (Escape or "History" goes back): the range read in a job, the plan's rows newest
    first (each a commit's place in the range, a `Kind`, and a reworded message once there is
    one), drawn with `show_rows` (one height: a row's pitch counts the item spacing), each with
    Move up and Move down, its step, its name and subject (dropped struck through, melded
    marked), the selected one's message below (a code field when reworded); keys while no
    text field has the focus: ↑ ↓, Alt+↑ Alt+↓ to move, P R S F D. `plan` hands git the rows
    oldest first, or says why not (nothing changes, nothing to meld into, an empty message,
    everything dropped); "Start rebase" runs it as a task. A range holding merges (said
    under the bar) keeps them in git's order: rows don't move, nothing melds, a merge (marked)
    is picked or reworded, never dropped (`Kind::allowed`); its task keeps merges.
    `views/search.rs` — `HistorySearch`, the search bar over the commit table (⌘F, or the
    top bar's Search, which shows the table in place of a file or a tool): the kind, the
    query (searched once typing pauses 0.3 s, the history read whole, the previous search
    dropped), "3 of 120" / "Searching… 45% · 120 found" / "No commits match" / the problem,
    Previous and Next (Shift+Enter and Enter, wrapping), Close (or Escape). The search's first
    find, from the selection on, is selected as it comes; each step selects one, centred.
    Found rows take `Palette::found` behind them. A history read again is searched again
    (`follow`), the selection kept.
    `views/patch.rs` — `PatchView`, patches looked at before they are applied, a `Tool` in the
    commit table's place: from files (the Patches menu's dialog, or dropped on the window:
    `looks_like_patch`, a hint while dragging), pasted (read once typing pauses), or the
    patch a stopped `git am` is at (to look at only). What they hold ("3 commits by mail,
    changing 2 files"), whether they apply here, in words and a colour, a mailbox's commits
    and the files with their lines, how (as commits, signed off or not; as changes, staged
    or not), "Apply to <branch>" (disabled, saying why, while a task or an operation runs, or
    when git can't apply it), and the patch itself below, highlighted (`Plain`, a `.patch`).
    The Patches menu (`actions::patches_menu`, beside the stash menu): apply from files,
    paste, and the staged or every change copied or saved as a patch; a commit's row menu:
    Save as a patch, Copy as a patch, Save this and later commits as patches.
    `views/sidebar.rs` — `Sidebar`, the panel left of the commit table (resizable; "Hide the
    sidebar" leaves a strip to show it again): a filter (⌘ Alt F; smart case; Escape clears
    it; it unfolds what it matches and scrolls back to the top), then sections, each a header
    with its count (named "Branches: 5"; a click folds it; its menu; a "+" adding a remote, a
    stash, or a worktree): branches, folded into folders by their `/`s; remotes, each with its
    branches (its menu: fetch, pruned or not, change its address, rename, remove, copy the
    address, open on its forge); pull or merge requests (on a forge; a click shows one); tags
    (folded at first); stashes (a click shows its commit in the details: `HistoryView::peek`,
    which the details show in place of the selection while the history doesn't have it; Apply,
    Pop, Drop); worktrees (here, locked, missing marked; a double click or "Open in a tab";
    Lock, Unlock, Remove); submodules (when there are any: path and recorded commit, or "not
    set up"; Update or Set up, Open in a tab). Each section's header is a band (its caret,
    icon, title in small capitals, and count in a pill: `widgets::section_galleys`,
    `paint_section`); a tree's levels step in by a whole icon (`INDENT`: a folder's or
    remote's children have their icons under its name), a guide running down from each
    level's icon, its caret hanging left of its icon (`paint_row`, `paint_caret`). A branch's
    or remote branch's eye at its row's right (`eye`: open, or struck through while hidden;
    named "Show <name> in the graph", a checkbox) hides it from the graph or shows it
    (`Scope::hidden`; the branch checked out has none: it always shows, marked with a check,
    its name leading); a ref's row selects its commit (or peeks at it), a branch's
    double click switches, its menu is its badge's, and it drags and takes drops as a badge
    does. The refs are sorted (by kind, then their
    `/`-parts naturally: `v2.9` before `v2.10`, either case alike) in a job, once a read of the
    refs or when the remotes change; the rows are made again only when what they show changed
    (the sorted refs, a list read again, the forge's requests, the filter, a fold; from owned
    input, `RowsInput`: typing in the filter over more than 5,000 refs makes them in a job, one
    at a time, the last rows shown until the latest land) and drawn
    with `show_rows`, their text painted.
    `views/lfs.rs` — `LfsView`, Git LFS's page in the commit table's place (the LFS menu's
    "Patterns, files, and locks…", the Repository menu's "Git LFS", the banner; Escape or
    History goes back): what LFS is and whether it is installed and set up here (how to,
    when it isn't), moving its files (Pull, Fetch, Fetch every file, Check out, Push to each
    remote, Prune; each saying what it does), the patterns tracked (each to stop tracking;
    one to track, lockable or not), the files held at `HEAD` (how many, how large, how many
    not downloaded or pointers; each here, downloaded, or a pointer, with its size; drawn as
    far as they show), and the locks (read when asked: they need a server; each to unlock or
    force; a path to lock). It reads the whole state itself, again when the tab's lands anew.
    `views/submodules.rs` — `SubmodulesView`, the submodules' page in the commit table's
    place (their menu, the Repository menu's "Submodules…", the banner, the sidebar's section
    menu; Escape or History goes back): what submodules are and how they stand altogether
    (mend everything, update all either way, sync addresses, "Update them on every switch and
    pull": `submodule.recurse`), each one (nested ones indented under theirs; its state's
    icon, word, and colour, its commit and describe; rows named with their state), the one
    selected (what its state means and what to do about it; name, addresses (the one set up
    here when out of date), the branch it follows, the commits recorded and checked out, on a
    branch or detached "as submodules usually are", ahead and behind, the commits between;
    set it up or update it to the commit recorded, to its branch's latest, record its commit
    here (stage its path), sync its address, open it in a tab, change its address or branch
    in place, forget it here, remove it after a confirmation; in a `Ui` of its own id, as
    what is above comes and goes; a nested one's: open it, its parent changes it), and adding
    one (address, folder named from it as git names it, branch).
    `views/reflog.rs` — `ReflogView`, a ref's reflog in the commit table's place (the top
    bar's Reflog: `HEAD`'s; a branch badge's "Show <name>'s reflog"): read in a job (10,000
    moves at most), read again when the history is (`follow`) or `HEAD` moves, the selection
    kept on its move (by commit and time). Each row (`show_rows`; ↑ ↓; named by what moved
    it): its place, how long ago, what moved it, the commit and its subject. The selected
    move's commit shows in the details beside it (`peeked`, though no branch may reach it),
    its files opened in the reflog's place; "Show the commit" (when the history has it),
    "Create a branch here…" (not switched to: kept), "Reset <branch> to here" (soft, mixed,
    keep, or hard after the dialog), "Switch here, without a branch". The top bar's Undo and
    Redo (first in it, where nothing before them changes width, as the commit and stash
    counts do; icons named in words; their hover says what they'd take back or do again; ⌘Z,
    ⇧⌘Z while no text field or dialog has the keys; disabled while a task runs or a commit is
    made) push `Action::Undo`; their outcome reads "Undid …" or "Redid …".
    `views/blame.rs` — `BlameView`, and `views/file_history.rs` — `FileHistoryView`: a
    `FileView`'s "Blame" and "File history" (`FileWanted`; a deleted file is blamed as its
    parent had it) open them in the commit table's place, each a `Tool` of the history view
    as the rebase editor is (one at a time; Escape or "History" goes back). The blame: "Blame
    <path> at 1234abcd" (or "in the work tree"), "Follow moved lines" (`Moves`, blamed
    again), "Blaming… 1,200 of 5,000 lines" while git works; a line selected shows its
    commit below, with "Show the commit" (selected in the history) and "Blame before this
    commit" (its `previous` commit and path). The history:
    `file_log`'s commits as they are found, newest first (`show_rows`; a rename's row says
    "from <old path>"), ↑ ↓; the selected one's diff of the file below (`read_diff`, a pure
    rename said so), "Show the commit", and "Blame it here".
    `views/clone.rs` — Home's clone card (`CloneView`): the address, the folder it goes into
    (typed, `~` the home folder and a relative path inside it, or picked: `PickCloneFolder`;
    `Settings::clone_folder`, the last clone's, at first), the folder's name (`name_from_url`,
    as git names it, until edited), submodules, partial; `Action::Clone` runs `clone` as a
    job (one at a time; `format::progress` and Cancel while it runs, `CancelClone`), then
    opens the repository; cancelled, it says git removed what it had made.
    `views/pulls.rs` — `PullRequestView`, the pull (or merge) requests in the commit table's
    place, named their forge's way (a `Tool`, from the top bar's "N pull requests" or "N merge
    requests" button, or a badge): a bar (repositories,
    "12 open" or "1,000 of 5,512 open: the most recently updated", Refresh, a problem), the
    rows (`show_rows`: number, checks, title, a draft's chip, author, head → base and a
    fork, review, age; ↑ ↓), and the selected one's details (read when selected:
    `pull_request_details`): "Show the commit" (when its head is in the history: looked for
    once a selection, a scan), "Check out" (a same-repository one fetches its branch into the
    remote-tracking one and switches to the local branch, tracking it; a fork's fetches its
    forge's ref for it into `pr/<n>` or `mr/<n>` (`fork_head`); disabled, saying why, while a
    task runs or an
    operation is in progress), "Open on GitHub", its size and mergeability, labels, checks
    (each with its page), latest reviews, description. Without credentials, a card to sign in
    (git's prompts) or to create a token on the host. `check_look` gives a checks state's
    icon and colour, which the commit table's pull request badges (`#12`, on head commits,
    their title on hover; a click shows it, the menu opens its page) share.
    `views/pull_form.rs` — `PullRequestForm`, the dialog opening a pull (or merge) request,
    from a local branch's menu ("Create pull request…" or "Create merge request…" once it is
    pushed to a remote on GitHub or GitLab; "Show pull request #n" or "Show merge request !n"
    when one is open for it): in a job, the repository's default branch
    and the one it was forked from (`repository_info`; a fork's pull request goes there by
    default), and the title and description of the branch's one commit
    (`commits_between` against the base, then its message), its name otherwise; then into,
    base, title, description, draft. "Open on GitHub" (or GitLab) is on a commit's menu too.
    `views/palette.rs` — `PaletteView`, the command palette (⌘K, ⇧⌘P, or the status bar's
    Commands): a modal with a field ("Command, branch, or repository") and the `Offer`s
    gathered as it opens (`AppState`'s `offers`: the repository in front's commands, each
    allowed or saying why not (`RepoTab::allows`), the application's commands, the recent
    repositories not open, and a switch to each local branch but the current one: `Offers`
    keeps a branch's place among the shared refs, its words made only for the rows drawn),
    narrowed by `score` as letters are typed (a letter typed after the others among what they
    matched), best first, drawn with `show_rows`; ↑ ↓ choose (taken at the
    frame's start, before the views below see them: `take_keys`), Enter or a click runs it,
    Escape closes. A repository's command is queued on its history view (`HistoryView::queue`),
    carried out next frame by the same code as its button.
    `views/preferences.rs` — the Preferences tab (⌘, or the status bar's): cards for how
    egit looks (theme, text size, commit dates), repositories (the background fetch, the
    sidebar), diffs and files (side by side, wrapped, tab width), and every keyboard shortcut;
    each change pushes `Action::SetPreferences`, in effect at once (each tab's fetch interval
    and sidebar too) and kept.
  - `views/` — `home` (who commits and the profile's button first, then open by dialog or
    typed path, the problem and its remedy, recent repositories in egui-table-kit, and
    cloning), `profile` (identity and signing editable;
    credentials, the GitHub Enterprise and self-managed GitLab hosts (added and removed as
    `egit.githubHost` and `egit.gitlabHost`), pull and push, and every setting, each with its
    origin), `repo` (the history, or a page: the overview or the settings table, under a bar
    going back to the history (or Escape) and switching between the two), `status_bar` (git's
    version and path, the
    theme, Commands and Preferences, a warning when
    questions can't be relayed), `prompts` (git's and ssh's questions: the app starts an
    `AskpassServer` at launch and gives git its environment; questions queue and show one
    at a time in a modal (`PromptKind` decides the field and buttons); the abandoned are
    dropped (checked every 250 ms while shown); answers are handed over off the interface
    thread (`jobs::detached`), then wiped, with the text field's own undo copy removed from
    egui's memory).
  - `file_tree.rs` — `FileTree`: paths as a tree of their folders, made in one sort (by
    folders, then names, a folder before a file where they part) and one pass keeping a stack
    of open folders; nodes are ranges of the caller's paths (24 bytes each: depth, entry, the
    name's span, a folder's end and count of files); a folder holding only a folder shows as
    one with it (`src/core`); `visible` lists the rows shown, skipping what folded folders
    hold (folds kept by path); `files_in` a folder's entries. `tree_table.rs` — `TreeTable`:
    a `TreeSource`'s files (a commit's changes, a work list) drawn as that tree by table-kit
    (`tables::tree_list`, one column), each row indented by its depth: a folder's caret, icon,
    name (its path on hover) and count of files, a click folding it; a file's change letter
    and name; at each row's right what its list adds (`TreeRow`: lines counted, a stage
    button); made again only when the list is replaced, in a job past 5,000 files, the last
    tree shown meanwhile, in a scroll area of its own.
  - `pictures.rs` — an image's sides read (blobs through the shared `Objects`, or the work
    tree's file, 64 MiB at most) and decoded by `image` (PNG, JPEG, GIF, WebP, BMP, ICO;
    20,000 pixels a side and 512 MiB of decoding at most), then scaled down to 2,048 pixels a
    side at most (all a screen shows) into an `egui::ColorImage` shared with its texture
    (`Picture`: its size as stored, its bytes, its pixels; a side that couldn't be read says
    why).
  - `tables.rs` — egui-table-kit providers (`ConfigRows`, `RecentRows`, `ChangeRows`) with
    O(1) `cell_at`. Every table asks table-kit to center its text across a row
    (`with_cell_align(Align::Center)`: unasked, it lays text out as `ui.horizontal` does, from
    the top of a cell shorter than the theme's widgets), and the custom cells here lay out
    across the cell's own height likewise; `styled` (every table: selected rows in the text's
    own colour, no table-kit outline) and `list` (file lists: a narrow column of
    `status_cell`s, a coloured letter for how each file changed (`status_mark`: A, M, D, R, C,
    T, U, !), named by the word, the path filling the rest, no header). `SnapshotTable` and
    `ChangesTable`, whose state
    starts over only when their snapshot or commit is replaced. A file's cell is named by its path ("old → new" for
    a rename); `file_cell` draws its counts at its right, so the narrow details panel needs no
    third column, and its (new) path in what is left, losing its start first, from a folder's
    boundary ("…/core/dc_link.c", `elided_start`, summing cached glyph widths), the full path
    and a rename's old one on hover.
  - `theme.rs` (Geist, the interface's font (`fonts/Geist.ttf`, SIL Open Font License 1.1:
    one variable font, borrowed, not copied; it has no glyphs where Phosphor's icons are, as
    Inter has), in three weights (`Weight`: regular, medium for buttons and names that lead
    their line, semibold for titles; `Weighted` gives `RichText` them); the monospace family
    stays egui's; palettes: `background` behind what is read (the commit table, the text
    grid), `panel` for the chrome around it, the selected tab, and pages, `recess` deeper (the
    tab bar), `surface` for cards, dialogs, and the
    sidebar's section bands; corner radii on one scale, each container a step rounder than
    what sits inside it so their corners follow (`RADIUS_INNER` 4: a row's highlight, a
    segment, a badge, a band; `RADIUS_CONTROL` 6: buttons, fields, the focus ring;
    `RADIUS_BOX` 8: boxes, menus, banners; `RADIUS_CARD` 12: cards, dialogs, windows);
    WCAG AA checked by tests, text over every line background too: diff
    lines and words, a conflict's sides and headers, resolved lines, rows a search found;
    a selected or found row under the pointer (`selected_hover`) too, and text on the danger
    colour; Phosphor icons; one scale of text: `SMALL` 11.5, `BODY` 13, `CODE` 12.5, `HEADING`
    14, `TITLE` 18, and `CONTROL` (28), the height of buttons, fields, and menus' buttons;
    every control has an edge: `field` behind what is typed, `border_strong` under the
    pointer), `widgets/` (the pieces every view is built from, so each looks and answers
    alike: titles (`page_title` 22, `panel_title` 16, `heading`), buttons by kind (`primary`,
    `danger` for what loses something, `secondary`, `small`, `link`; a toolbar's quiet `tool`,
    `tool_menu` with its caret, and `icon_tool`, named in words, `Look::Full` or `Compact`;
    words never wrap inside a button; `quiet` draws frameless buttons without an inactive
    edge, which egui leaves out of their size but takes out of their padding, so they don't
    grow under the pointer; `tool_width`, how wide a tool would draw, for choosing a look
    before drawing), `toolbar_separator`, `section_title` (small capitals, muted, its count in
    a pill; named in sentence case) and `section_header` (one, its buttons at the right),
    `text_box` (a long text apart: a commit's description) and `list_box` (a list's rows in a
    border, their corners rounded to follow its), `bar` (a view's bar: the chrome's tone, its
    controls 12 points from its sides and 6 from its top and bottom, a hairline under it,
    nothing spaced from it, so stacked bars make one strip and every view's back button is in
    one place), `panel_head` and `HEAD` (a panel's first row, 40 points: the sidebar's filter,
    the commit table's column titles, the details' title line up across the window; its first
    line centred on the band, by what it holds (`Head`: a control, a title, text; beside a
    control, a label or a smaller button centres on it), and what wraps grows downwards,
    keeping that line's room above it), facts in
    aligned columns (`fact`, `fact_top` (its first line as far down as a one-line fact's, so
    facts keep one rhythm),
    `form_row` naming its field; `FACT_LABEL` on pages), `line_field` one-line fields a
    control's height, their text centred and 8 points from the side, and `text_area` (several
    lines, 8 points from every edge; `bounded_text_area` at most so tall, scrolling inside its
    edge, as every message, description, and pasted patch is), `show_overflow` (scroll bars
    shown while text is cut, thin, in room of their own: egui's floating ones show only under
    the pointer), `link` (hanging into its inset, its words aligned with
    text above and below), `icon_button` exactly its place, `row_fill` a list row's background (a selected
    one lightens under the pointer), cards, origin labels, problem boxes; `avatar.rs`: someone
    as their initials (their name's first and last words', else their email's) on a colour
    of the palette's `avatars` hashed from their email (FNV-1a, any case), so one person looks
    alike everywhere; drawn, never fetched (`avatar`; `avatars` overlapping a little, three
    at most and "+2"); `toggles.rs`: a
    checkbox (a square box, filled and ticked when checked), radio buttons, and a segmented
    control (sized from its words before it is drawn, a control's height, so it centres on its
    row as any widget does; its 24-point segments' corners following its), each a whole row
    to click; `TARGET`, the 24 points every control takes each way;
    `focus_ring`, drawn once a frame around the control with the keyboard focus after a key
    (Tab, an arrow) or an assistive technology moved it, gone at the next press of the
    pointer, as a browser's `:focus-visible`, never around a field (its edge shows it) or an
    area as large as the commit table (its selection shows it)), `format.rs` (`~` paths, "5 minutes ago",
    dates (in the viewer's time zone: `local_zone`, `local_date_time`, `zone_name`), a long
    command's progress), `settings.rs` (theme, recent repositories, open tabs,
    the clone folder; non-UTF-8 paths kept as
    bytes), `problem.rs` (`Problem`, `OpenProblem` with its `safe.directory` remedy).
  - `tests/askpass_helper.rs` — the real `egit` executable (`CARGO_BIN_EXE_egit`) started by
    git as its helper through the server's link: credentials, and an SSH signing key's
    passphrase (asked as "Signing the commit"; the commit's signature then checked).
  - `src/tests/screens.rs` — the whole application driven through the accessibility tree
    (egui_kittest) against a sandboxed git; renders `target/gui-screens/*.png` where a wgpu
    adapter exists (`EGIT_GUI_RENDER=1` requires one, `0` skips). Look at them after any
    interface change. Every rendered screen first passes `src/tests/audit.rs` (after eshine's):
    no egui id clashes, no text or control drawn partly over another on the same layer (a
    floating scroll bar aside: it lies over what it scrolls),
    nothing moving while idle, and every control named in words or numbers (link a field to
    its visible label with `labelled_by`), and every one-line field a control's height, its
    text centred top to bottom and 8 points from its side. `EGIT_GUI_RECTS=1` writes each
    rendered screen's elements (role, rectangle, name) to `<screen>.rects` beside it, to
    measure a screen to the point. `wait_for_label` lets what it found lay out two
    more frames (a window's first frame only sizes it: clicks there are lost); a wait that
    times out saves `timed-out-<test>.png`; `menu_item` right-clicks and picks, and
    `double_click` puts both clicks in one frame (the harness's frames are a quarter second
    apart). After an action, wait for the history's rows to settle before aiming a click at
    one: a re-read moves them. Then, the pointer resting on each control it can reach: it
    visibly responds, moving nothing in its row or column (itself included: a control
    growing under the pointer pushes its row along; elsewhere, a job landing meanwhile may
    move things), its cursor fits (a text cursor over a field, a hand over the rest:
    `pointing_hand` sets it for every enabled control that clicks), it is 24 points each way
    (WCAG 2.5.8), and the keyboard focus shows on it; each looked at with the focus elsewhere
    (given back after), but a field that takes the focus back whenever nothing has it (a
    dialog's), and each found afresh before it is looked at (focusing a commit's row selects
    it, which changes the details beside). A screen is rendered once what it shows has landed
    (a commit's details, their lines counted): the audit's checks take many frames. `render`
    reads each work tree's status afresh first and waits for it, so the work in progress's row
    can't come or go (moving every row) while a screen is checked. `src/tests/polish.rs` drives preferences (changed from the status bar, in effect
    for a repository opened after: dates as how long ago, a diff side by side, no background
    fetch; kept) and the palette (a command found by a few letters, one that can't run saying
    why, a branch switched to) and the shortcuts (⌘B, ⌘,, ⌘W, ⌘1). `src/tests/menus.rs` drives the menus' newer
    actions: pulling, pushing, fast-forwarding branches, upstreams set and forgotten, a name
    copied, a branch deleted here and on its remote, a patch saved, a worktree opened in a
    tab, the graph narrowed and pinned. `src/tests/bisect.rs` drives a bisection from two
    commits' menus and the banner to its first bad commit (shown, then stopped), and a branch
    holding a merge rebased onto another keeping it, then in the editor (a commit dropped,
    nothing moved or melded, the merge kept). `src/tests/sidebar.rs` drives the sidebar: what it lists
    and counts, folders and sections folded, the filter (Escape clears it), a branch's row
    selecting its commit and a stash's showing its changes, a branch hidden and shown again;
    a remote added (its name offered) and fetched, renamed, re-pointed for pushing, fetched
    alone, removed; a worktree locked, unlocked, opened in a tab, removed (asked again with
    changes); a submodule set up. `src/tests/patches.rs` drives patches: a series looked
    at and applied signed off, a conflicting one stopped (its progress, its patch, Skip,
    Abort), the staged changes copied, saved, pasted back and applied, a commit copied, a
    series saved. `src/tests/people.rs` drives co-authors: picked from the recent authors and
    typed, committed as trailers, shown in the details, taken back into the form to amend.
    `src/tests/help.rs` holds Alt over Push and a commit's row (their topics shown, gone
    when let go), and finds a topic with F1 (Escape closes it); its `assert_explained` runs
    on every rendered screen: each control's words drawn so far (kept in tests) must find a
    topic (`EGIT_HELP_GAPS=<file>` lists those that don't instead of failing).
    `src/tests/columns.rs` drags the lines between the commit table's columns (each moving
    alone, holding still once let go, a double click putting them back);
    `src/tests/long_text.rs` edits a 200-line message in place and types one into the commit
    form, each scrolling inside its field, the buttons below still on screen.
    `src/tests/submodules.rs` meets a fresh clone's submodule not set up (the banner mends
    it), updates it to its branch's latest from its page (the commit it brings shown), records
    it, shows its change in the work tree; and adds one from the page.
    `src/tests/lfs.rs` meets Git LFS not set up (the banner sets it up), its menu and page,
    a file listed, a pattern tracked and untracked (skipped without git-lfs unless
    `EGIT_REQUIRE_LFS`).
    `src/tests/dragging.rs` drags branches: onto the current one (dismissed, then merged), the
    current one onto another (rebased), between the sidebar and the graph either way.
    `src/tests/pictures.rs` shows an image's change: a commit's (both sides' sizes) and the
    work tree's.
    `src/tests/tree.rs` drives the folder tree: chosen in a list's header, folded and unfolded,
    a folder staged whole, a commit's files alike, a list again.
    `src/tests/notices.rs` lets background fetches run: one bringing nothing says nothing, two
    bringing a commit each make one notice of both.
    `src/tests/search.rs` drives the search bar
    (by message, author, path, and name; both themes) and holds egit's own reading of messages
    and authors to what `find_commits` finds on 300 assorted commits. `src/tests/scrolling.rs`
    turns the wheel through a commit's 300 files and 300 files to stage, past their ends,
    failing anything that moves back while scrolling down or moves once the wheel stops.
    `src/tests/scale.rs`
    and `src/tests/corpus.rs` are the opt-in measurements (see Scale and Text corpus above).
- `docs/ROADMAP.md` — the MVP phases and their exit criteria.
- `scripts/build-git.sh` — builds a git release into `target/git/<version>` (as C17: older
  releases don't compile as C23). `scripts/check-platforms.sh` — lints for Windows and macOS
  from Linux (see Toolchain & commands).

## Conventions

- **Errors:** typed (`thiserror`) in `egit-git`; the application turns them into sentences a
  person can act on. Never `unwrap`/`expect`; never swallow an error silently.
- **Sync:** `parking_lot` only (`clippy.toml` disallows `std::sync::{Mutex, RwLock, Condvar}`).
- **Unsafe:** denied workspace-wide. The rare exception lives in a small named module that
  allows it, with a `// SAFETY:` comment on every block.
- **Docs:** every public item has rustdoc; every module starts with a doc comment saying what it
  is for.
- **Tests:** unit tests beside the logic (`mod tests`); tests against a real git live in the
  crate's `tests/` directory and run in a sandbox (a temporary `HOME`, no system config).
- **Interface:** never block the interface thread: every git command, dialog, and file read is
  a job. Controls drawn after something that comes and goes (a spinner, a chip, a problem)
  belong in a `Ui` with an explicit id (`UiBuilder::id`): egui counts ids in drawing order
  (a salt alone doesn't change that), and a click pressed on one id and released on another
  is lost. Colours come from `theme::Palette::of(ctx)`, icons from `theme::icon`, shared pieces
  from `widgets`. Text inside an `egui::Grid` is sized explicitly (`add_sized`): a grid sizes
  cells to their content. Names and paths come from people: assume them long, in any script.
  Every `Bind` is made retained (`Bind::new(true)`), never by `Default`: egui-async aborts a
  read under way in a `Bind` not polled the frame before, by a frame count it keeps for the
  whole process (several windows, as the tests run, make that happen at random).
  Help: a new control built from `widgets` (or a menu's item) is explained by its words: claim
  them in a topic's `on` (every rendered test screen fails on a control whose words find no
  topic); a custom one calls `help::note_topic`; a new feature gets topics in its area's file
  under `help/`, at git's level, with the cases that surprise.
- **Performance:** CPU time and peak memory are features. Nothing proportional to a
  repository is loaded or drawn unless it is visible or being prepared off the interface thread;
  parsers work on borrowed bytes and reuse their buffers.

## Commits and review

- The implementer commits each self-contained change as it is finished, in Conventional Commit
  style (`feat(git): …`, `fix(app): …`, `ci: …`, `chore(meta/deps): …`), ending the message with
  the `Co-Authored-By` trailer the session asks for.
- Never sign: commit with `--no-gpg-sign` (the owner's config signs by default). The owner reviews
  by rebasing, signs, and pushes. Never push, never create signed tags, never rewrite the owner's
  history.
- Changes to `egui-table-kit` are made in its own clone (`~/egui-table-kit`), directly on
  `main` (no branches), each an independent commit appended to the previous ones, so they can
  be reviewed and released on their own.

## CI

GitHub Actions (`.github/workflows/ci.yml`): formatting, lints, and docs; the tests on the
latest Ubuntu (screens rendered, every signer required) and in `ubuntu:24.04` with its git
2.43; macOS and Windows each linted and tested (the platforms job); and the supply chain.
Every action is used at its latest major version, and Dependabot keeps them there
(`.github/dependabot.yml`).
