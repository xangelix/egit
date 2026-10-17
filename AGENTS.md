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
| 7 | The graph's actions: branches, commits brought in, tags, stashes | in progress |

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
- Text corpus: `cargo test -p egit --release --lib -- --ignored --nocapture --test-threads=1
  corpus` draws the text grid through the roadmap's worst cases (1M lines of code, plain and
  highlighted, a 10 MB line wrapped and not, CJK and emoji, resizing wrapped at line 900k),
  then the whole application on a commit moving 100,000 files (listed in 105 ms, counted in
  430 ms), and prints frame times, laid out and tessellated (it fails past 8 ms at p99);
  renders `corpus-*.png` where it can. Every case stays under 2 ms but the highlighted ones
  (3.7 ms jumping, up to 2.6 ms scrolling: colouring takes up to 3 ms a frame).
  `corpus_merge`: the merge editor on 100,000 lines of C with 1,000 conflicts: merged by git
  in 220 ms, the document made in 9 ms (open, and again after a pick), stepping from conflict
  to conflict highlighted at 3.4 ms p99.
- Scale: `EGIT_SCALE_REPO=<a large repository> cargo test -p egit --release --lib -- --ignored
  --nocapture scale` opens it headless and prints time to first rows, to the whole history,
  frame times jumping and wheel-scrolling through it, and peak memory (it fails past 8 ms a
  frame). Linux (1.49M commits): first rows 135 ms, all 1.45–1.6 s, slowest frame 2.1 ms,
  peak 69 MB (with rendering: 190 MB, and `scale-*.png` screens).

## Supported platforms

Linux only for now. The floor is Ubuntu 24.04 LTS: glibc 2.39 and git 2.43.0. Never use a git
feature newer than 2.43.0 without a fallback, and check its release notes when unsure.

## Rules for running git

- Every repository operation runs the system `git`; never read or write files under `.git`
  directly, except to watch them and to read the few state files git documents (`MERGE_HEAD`,
  `rebase-merge/`, …). No in-process git (`deny.toml` bans libgit2).
- Machine-readable output only: `-z`, `--porcelain`, explicit `--format`s, and `-c` overrides
  for any setting that changes a format the parser reads.
- Reads pass `--no-optional-locks`, so a background refresh never takes `index.lock` from the
  user's own git. Commands that write go through the repository's single write queue.
- Never SIGKILL git: cancellation is SIGTERM to its process group, which lets git remove its
  lock files.
- A command about one repository runs in its work tree (`current_dir`); one that must not see
  any repository (the user's global configuration) runs in `/`, never in egit's own working
  directory, which may be inside some repository.

## Repository layout

- `crates/egit-git` — the git binary: running it, and parsing what it prints. No egui.
  - `git.rs` — `Git`: the executable (found on `PATH` or given), its version checked against
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
    stdin is fed from another.
  - `progress.rs` — `Progress`: a long command's `--progress` lines read as they come, each
    replacing the last (git rewrites them with `\r`): the phase, done and total, the transfer
    (`1.20 MiB | 2.30 MiB/s`), whether the remote is doing it (`remote: `), and the other
    lines as notes (the latest fifty).
  - `process.rs` — the crate's only `unsafe`: `setsid` in `pre_exec` (no controlling
    terminal; git leads its process group). The leader is waited for with `waitid(WNOWAIT)`
    so a cancel can't signal a reused id; `cancel.rs` holds the running groups under one lock.
  - `error.rs` — `GitError`, with `Failure` (exit status + stderr tail, `message()`).
  - `object_id.rs` — `ObjectId` (SHA-1 or SHA-256, by length; never assume 40 digits) and
    `ObjectFormat`.
  - `repo.rs` — `Repository::discover` (two `rev-parse`s; `DiscoverError` names not a
    repository, `safe.directory` refusals (`trust_directory` adds one), bare repositories, and
    paths inside `.git`), `head` (`symbolic-ref`, then `for-each-ref` on that exact ref for the
    commit and `%(upstream:track)`, or `log -1` when detached), and `operations` (merge,
    rebase, am, cherry-pick, revert, bisect from their state files in the git dir).
  - `config.rs` — `ConfigSnapshot::read(git, ConfigPlace::{User, Repository})`: one
    `git config --list --show-origin --show-scope --null`, kept as one buffer the entries point
    into (u32 spans); entries in the command scope (egit's own `-c`) are dropped. `get` is the
    last value (git's rule), keys compare like git's (section and name in any case, subsection
    exactly); `is_partial_clone` (a promisor remote or a clone's filter). `set_config`
    (`--replace-all`) and `unset_config` (`--unset-all`; exit 5 is fine) write `--global` from
    `/` or `--local` in a work tree.
  - `profile.rs` — `Profile` over a snapshot: identity, `Signing` (format, key, defaults,
    program — `gpg.program` and `gpg.openpgp.program` are one setting, the later wins —,
    allowed signers), credential helpers (an empty value resets the list), pull and push
    settings; each as a `Setting` or `ConfigEntry` that knows its scope and file.
  - `askpass/` — prompts from git's and ssh's helpers relayed to the window. `AskpassServer`
    makes `egit-<uid>/<pid>-<nonce>/` (0700, ownership checked) under `$XDG_RUNTIME_DIR` or
    the temporary directory, holding a Unix socket and `egit-askpass`, a link to egit's
    executable; `environment()` sets `GIT_ASKPASS`, `SSH_ASKPASS`, `SSH_ASKPASS_REQUIRE=force`,
    the socket, and a random token. Started under that name, egit is the helper
    (`run_helper`). Connections must come from the same uid (`SO_PEERCRED`) and carry the
    token; fields are bounded (`protocol.rs`); answers are zeroized. Its accept loop polls the
    listener and an eventfd, so stopping never depends on the socket's path. `Prompt`s
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
    is `%s`).
  - `history.rs` — `walk_history` (`rev-list --topo-order --parents`, streamed, lines split
    across chunks without copying; with a commit-graph it starts from `merge-base
    --independent` of `--all`'s tips, since git's topological walk first reads down to the
    oldest tip: Linux's first commit in 60 ms instead of 700), `list_references`
    (`for-each-ref`, symbolic refs skipped, tags peeled), `commit_changes` (`diff-tree -r -z
    --raw --no-abbrev -M` against the first parent, or `--root`: each side's mode and object),
    `line_counts` (the same with `--numstat`, in the same order: every file diffed, so it
    follows the list; `GIT_NO_LAZY_FETCH` where git honours it, never prompting),
    `write_commit_graph`;
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
    is; folders end with `/` so folder-only patterns match them).
  - `committing.rs` — `create_commit`: `git commit --file=- --cleanup=whitespace` (a line
    starting with `#` is kept), `NewCommit`'s amend, sign-off, and signing (`-S`,
    `--no-gpg-sign`, or the configuration's choice); a passphrase is asked through askpass,
    labelled "Signing the commit"; `Committed::output` is what git and its hooks printed (a
    refusal's whole standard error is on the failure).
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
    notes; a refusal isn't an error), `remotes`, and `abort_operation` /
    `continue_operation` for a merge, rebase, am, cherry-pick, revert (bisect: `reset`).
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
    `tag.gpgSign` would ask for a message), `delete_tag`.
  - `integrate.rs` — commits brought into the current branch: `merge` (`--no-edit`),
    `rebase`, `cherry_pick`, `revert` (`--no-edit`; a merge's against its first parent, by
    `rev-list --parents`), each `Integrated::Done`, `Conflicts` (failed with the operation in
    progress and `ls-files -u` not empty), or `Empty` (a pick that is "now empty", skipped);
    `reset` (`ResetMode`: soft, mixed, hard).
  - `stash.rs` — `stashes` (`stash list -z`, `%gd %H %ct %gs`: index, commit, time, git's
    message), `stash_push` (a message, untracked files with `--include-untracked`; whether
    anything was stashed, from `refs/stash` before and after, as `--quiet` says nothing),
    `stash_apply` (and pop; `Integrated::Conflicts` keeps a popped stash) and `stash_drop`,
    each refused (`StashError::Stale`) unless `stash@{n}` is still the stash listed.
    `tests/actions.rs` runs all of these, and the pushes, against sandboxes.
  - `testing.rs` (feature `testing`) — `Sandbox`; `EGIT_TEST_GIT` picks the git to test.
- `crates/egit-graph` — the commit graph's lanes; no dependencies, knows nothing of git
  (commits are any `Copy + Eq + Hash` key).
  - `layout.rs` — `GraphBuilder::push` lays out each commit as `walk_history` yields it, in
    time proportional to its parents and the lanes meeting it (never the graph's width: lanes
    leading to a commit are chained from a hash table, free slots found in a bitmap). Rows
    (12 bytes) and their edges are kept in `Chunk`s of 1024 rows, each with the lanes open at
    its start. Linux: 1.49M rows in 21 MB, laid out as fast as git walks them.
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
    to stop between them; `matches_case` is smart case.
- `crates/egit` — the application (`egit_app` library, `egit` executable). eframe/egui 0.36
  (wgpu), `egui_tiles` 0.17, `egui-table-kit` 0.7 (a git revision ahead of crates.io: bump
  `rev` to the owner's pushed commit after each change made there), `egui-async` 0.7 (stay on
  0.36 until `egui_tiles` and `egui-phosphor` move).
  - `main.rs` — started as `egit-askpass` it is the askpass helper (`run_helper`); otherwise
    `lib.rs`'s `run`: `-h`/`-V`, paths to open, `EGIT_LOG` logging, the runtime, the window.
  - `app.rs` — `EgitApp { tabs, state }`. Views never change `AppState` directly: they push
    `Action`s, applied after the frame (`apply_actions`); `AppState::poll` (each frame's
    start) turns finished jobs into state or actions. Git is detected by a job
    (`Bind<Git, Problem>`); paths asked for before it is ready wait in `waiting`. The tabs
    open at the last close (`Settings::open`) come back. Profile edits are `ConfigChange`s
    written under the user's `WriteLock`, then the user's and every repository's settings
    are read again.
  - `jobs.rs` — the runtime (2 workers, at most 16 blocking jobs: git processes queue
    beyond), `blocking` (a closure on the blocking pool as a future for a `Bind`), and
    `WriteLock` (taken inside jobs, never on the interface thread).
  - `tabs.rs` — the window's `egui_tiles` tree: `Pane::{Home, Profile, Repo(id)}`; `show`
    focuses or adds a tab; Home can't be closed; closing a repository's tab pushes `Closed`.
  - `repo.rs` — `RepoTab`: `Repository`, `Bind`s for `Status` (head + operations) and the
    repository's `ConfigSnapshot` (`request` keeps showing the old state until the new one
    lands) and its stash list (read at once, when refs change, and after each task), its
    `HistoryState` and `HistoryView`, its `WriteLock` (writing a commit-graph),
    its own tiles (`RepoPanel::{History, Overview, Configuration}`), and its `Watcher`
    (`follow_changes`: refs or operation state changed → status read again, refs → history
    read again; config → config; the index, refs, state, or a file → the `WorkTree`'s
    status; a task ended → status and work tree read again, its refs left to the
    watcher, so a history is walked once). `poll` (each frame, every tab) takes in its jobs'
    results.
  - `history.rs` — `History`: a walk (`jobs::detached`) streams `Piece`s through a channel:
    the refs and whether a commit-graph exists, then each completed `egit-graph` chunk with
    its commits' names packed (20 bytes each), then the end. `id(row)`, `row_of(id, near)` (a
    scan from `near`; no id→row index is kept), `Refs::at(id)` (sorted, binary search).
    `Refs::has_branch` (a local branch by name, for switching to a remote's).
    `Objects`: one shared `cat-file` per repository, restarted after a failure.
    `Summaries`: subject, author, preformatted date, read as rows come into view (at most 512
    a read, one read at a time), about 4,096 kept, least recently drawn forgotten first; each
    says whether its commit is signed. `Signatures`: the signed commits on screen (and the one
    selected) checked with `check_signatures`, 64 at a time, one batch at a time (each check
    runs a program); a failed batch counts as checked without a verdict, so it isn't retried
    every frame; forgotten when the configuration is read again.
    `read_details` (the commit and its `commit_changes`); `read_change_counts` (its
    `line_counts`, lined up with the changes by path should their orders differ, and their
    total). `HistoryState`: the history shown,
    and a re-read (`reload`) that takes over only once complete (`replaced`), so the table
    never empties or jumps meanwhile.
  - `watch.rs` — inotify (`notify`) on git directories, configuration files, and a work tree's
    folders, never a work tree recursively: a repository's git dir and common dir
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
    text changes). Advances are cached per character until egui's font count changes
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
  - `tasks.rs` — `Tasks`, what the user asks of a repository: one `Task` the user
    asked for at a time (fetch `--all --prune`, pull as configured or by merge or rebase,
    push a branch or a tag, delete a remote's branch, abort or continue an operation,
    remember `pull.rebase` then pull; switch, stash then switch, create, rename, or delete a
    branch, tag or delete a tag; merge, rebase onto, cherry-pick, revert, reset; stash, apply
    or pop or drop a stash; `Action::Task`), run by `run` through four groups (remotes,
    history, references, stashes), under the `WriteLock`, its `Progress` shown and
    `CancelToken` stopped by `Action::CancelTask` (`Outcome::Cancelled`, not a failure).
    Besides git's results, an `Outcome` can be `Done`, `Integrated` (done, conflicts, empty),
    `Stashed` (whether anything was), `WouldOverwrite` (a switch refused, with the paths), or
    `NotMerged` (a delete refused). And a background fetch every 5 minutes (the first 10 s
    after opening; only with a remote): non-interactive, without the lock, stopped when the
    user starts a task, its failure kept apart (`background_problem`). `remote_names`,
    `has_remotes` (a scan, nothing allocated), and `upstream_of` read the configuration.
  - `fonts.rs` — system fonts for characters egui's lack (CJK, most scripts): views note them
    (`note_missing`, `note_text`, nothing to check for ASCII); `Fallbacks::poll` (each frame)
    looks once per character, in a job, through the XDG font directories (CJK families, then
    monospace, then regular weights; in a collection the face covering most, monospace and in
    the user's `LANG` first; outlines required, so no colour bitmaps) and adds what it finds as
    lowest-priority fallbacks to both families. Kept fonts are leaked once into `'static`
    slices: egui copies owned font data on every rebuild of its fonts, but borrows static.
  - `views/history.rs` — the top bar (the commit count, how many files have uncommitted
    changes: a button showing the working tree beside the history in place of a commit's
    details, as it always is before the first commit; and `views/tasks.rs`'s bar), and the
    commit table on egui-table-kit's layout `Table` (its `TableKit` keeps per-row state a
    graph doesn't need): `CommitRows` is the `TableDelegate`; `prepare`
    replays the visible rows' lanes once a frame (`GraphRows`, panned to the nodes on screen
    when the column is narrower than the graph: "‹ Graph ›") and asks for summaries (the
    screen, then a screen below and above). Graph cells paint lines (cubic curves between
    lanes) and nodes (hollow for merges, ringed for `HEAD`); rows are buttons named by their
    subject; ref badges (each its menu, a branch's double-click switching to it; in the details
    too); a signed commit's seal by its verdict (good, doubtful, bad, or
    unchecked, its meaning on hover); a focusable area takes ↑ ↓ PgUp PgDn Home End. The message column
    is sized each frame from the others' remembered widths. `HistoryView::follow` keeps the
    selection and the top row's commit in place when a re-read takes over. Details: message,
    names, parents (buttons: egui announces links as plain text), people, the signature (its
    verdict, signer, and key), refs,
    and the changed files (`ChangesTable`), with the lines each gained and lost once counted
    (`Counting`: a job after the list, the previous one stopped; not in a partial clone);
    clicking one opens a `FileView` (`views/file.rs`) in the commit table's place (Escape or
    "History" goes back): its diff against the first parent (unified or side by side, with
    "Whole file" for all of its context), or the file as the commit left it (as it was, if
    deleted; read the first time it is asked for, `read_file`), or a placeholder for binary
    files, LFS pointers, submodules, and files over 256 MiB. A `FileView` of a work tree's
    file (`WorkFile`: a path, which `Uncommitted` changes, the index entry its diff was read
    against) reads its diff again whenever the status changes, and has a second bar: stage
    or unstage the file, the lines selected in its unified diff, or their hunks. A file in
    conflict shows `views/merge.rs`'s `MergeView` instead (and "Stage file", resolving it as
    the work tree has it), reading the conflict again when the status changes (its choices kept
    while its sides are the same), then, resolved, its staged changes. `MergeView`: "Conflict 2
    of 5: <its choice>", Previous and Next (Alt+↑ and Alt+↓), how many are left, "Show the
    base", "Save resolution" once none is open; "Take" Current, Incoming, both either way, or
    Base (moving on to the next open conflict), "Edit…" (its text in a code field, Apply or
    Cancel; a line ending is added unless it ends the file), "Reopen", "All current", "All
    incoming"; lines selected in a conflict select it. A conflict that isn't two texts says
    what each side did and offers keeping a side or deleting the file. `Shared::operation`
    and `branch` label the sides.
    `views/work.rs` — `WorkPanel`: "Not staged" and "Staged", each a table-kit list (indices
    into the status, made once per read) with a button for all and one per file (named
    "Stage <path>"), a file clicked opened in the `FileView`; then the commit form: summary
    (warned past 72 characters), description (⌘Enter commits), amend (a blank form takes the
    last commit's message, `read_message`), sign off, sign (`commit.gpgSign` by default),
    and the last commit's output (its hooks') or problem (git's whole complaint). It clears
    once the tab's `commits_made` grows. A history of 10,000+ commits
    without a commit-graph is offered one (`Action::WriteCommitGraph`).
    `views/tasks.rs` — the task bar at the top bar's right: Push, Pull, Fetch, and the branch
    beside its upstream with ↑ ahead ↓ behind (or "(not pushed yet)", "(gone)"); while a task
    runs, its progress and Cancel. Under it the last task's outcome until dismissed (a line,
    the remote's notes, or a problem box), a refused push explained by its reason (behind:
    "Force push…" or Pull; a lease that failed: nothing forced, fetch first), and the banner
    of an operation in progress (Show the conflicts, Continue, Abort…; a bisect's Stop).
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
    before stashing the changes in a switch's way. The stash dialog: a message, and
    untracked files too.
    `views/actions.rs` — what the commit table offers (`Menus`, from `Shared::menus`: the
    branch, `HEAD`, the remotes, the refs, whether a task runs or an operation is in progress,
    which disable what would change the branch, saying why): a commit's context menu (switch
    there detached, create a branch or tag there, merge it in, rebase onto it, cherry-pick,
    revert, reset soft or mixed, hard after a dialog), a ref badge's (switch to a branch, a
    remote's through its local one tracking it, unless one of that name exists; merge it in,
    rebase onto it; rename or delete a branch, delete a remote's on its remote; push or delete
    a tag), a branch badge double-clicked (switch), and the stash menu beside the commit count
    ("Stash the changes…", and each stash with Apply, Pop, Drop…). Each asks a task or a
    dialog (`Ask`), carried out by the history view (`carry_out`).
    `views/clone.rs` — Home's clone card (`CloneView`): the address, the folder it goes into
    (typed, `~` the home folder and a relative path inside it, or picked: `PickCloneFolder`;
    `Settings::clone_folder`, the last clone's, at first), the folder's name (`name_from_url`,
    as git names it, until edited), submodules, partial; `Action::Clone` runs `clone` as a
    job (one at a time; `format::progress` and Cancel while it runs, `CancelClone`), then
    opens the repository; cancelled, it says git removed what it had made.
  - `views/` — `home` (open by dialog or typed path, the problem and its remedy, recent
    repositories in egui-table-kit, who you are), `profile` (identity and signing editable;
    credentials, pull and push, and every setting, each with its origin), `repo` (overview
    and settings table), `status_bar` (git's version and path, the theme, a warning when
    questions can't be relayed), `prompts` (git's and ssh's questions: the app starts an
    `AskpassServer` at launch and gives git its environment; questions queue and show one
    at a time in a modal (`PromptKind` decides the field and buttons); the abandoned are
    dropped (checked every 250 ms while shown); answers are handed over off the interface
    thread (`jobs::detached`), then wiped, with the text field's own undo copy removed from
    egui's memory).
  - `tables.rs` — egui-table-kit providers (`ConfigRows`, `RecentRows`, `ChangeRows`) with
    O(1) `cell_at`; `SnapshotTable` and `ChangesTable`, whose state starts over only when
    their snapshot or commit is replaced. A file's cell is named by its path ("old → new" for
    a rename); `file_cell` draws its counts at its right, so the narrow details panel needs no
    third column, and its (new) path in what is left, losing its start first, from a folder's
    boundary ("…/core/dc_link.c", `elided_start`, summing cached glyph widths), the full path
    and a rename's old one on hover.
  - `theme.rs` (palettes, WCAG AA checked by tests, text over every line background too: diff
    lines and words, a conflict's sides and headers, resolved lines; Phosphor icons), `widgets.rs` (cards,
    buttons, origin labels, problem boxes), `format.rs` (`~` paths, "5 minutes ago",
    dates, a long command's progress), `settings.rs` (theme, recent repositories, open tabs,
    the clone folder; non-UTF-8 paths kept as
    bytes), `problem.rs` (`Problem`, `OpenProblem` with its `safe.directory` remedy).
  - `tests/askpass_helper.rs` — the real `egit` executable (`CARGO_BIN_EXE_egit`) started by
    git as its helper through the server's link: credentials, and an SSH signing key's
    passphrase (asked as "Signing the commit"; the commit's signature then checked).
  - `src/tests/screens.rs` — the whole application driven through the accessibility tree
    (egui_kittest) against a sandboxed git; renders `target/gui-screens/*.png` where a wgpu
    adapter exists (`EGIT_GUI_RENDER=1` requires one, `0` skips). Look at them after any
    interface change. Every rendered screen first passes `src/tests/audit.rs` (after eshine's):
    no egui id clashes, no text or control drawn partly over another on the same layer,
    nothing moving while idle, and every control named in words or numbers (link a field to
    its visible label with `labelled_by`). `wait_for_label` lets what it found lay out two
    more frames (a window's first frame only sizes it: clicks there are lost); a wait that
    times out saves `timed-out-<test>.png`; `menu_item` right-clicks and picks, and
    `double_click` puts both clicks in one frame (the harness's frames are a quarter second
    apart). After an action, wait for the history's rows to settle before aiming a click at
    one: a re-read moves them. Not yet audited, as eshine does: hover response,
    cursors, focus rings, and 24-point targets. `src/tests/scale.rs` and `src/tests/corpus.rs`
    are the opt-in measurements (see Scale and Text corpus above).
- `docs/ROADMAP.md` — the MVP phases and their exit criteria.
- `scripts/build-git.sh` — builds a git release into `target/git/<version>` (as C17: older
  releases don't compile as C23).

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

GitHub Actions (`.github/workflows/ci.yml`). Every action is used at its latest major version,
and Dependabot keeps them there (`.github/dependabot.yml`).
