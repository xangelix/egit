# AGENTS.md

## What this is

**egit** — a native git client in egui, built to stay fast on the largest repositories
(GitKraken's ground, without Electron). The system's `git` binary does every repository
operation; egit runs it, reads what it prints, and draws only what is on screen.
`docs/ROADMAP.md` is the plan (five MVP phases) and the decisions behind it.

## Status

| Phase | Scope | State |
|---|---|---|
| 1 | Foundation: shell, running git, askpass, profile, opening repositories | done |
| 2 | Commit graph: lanes, the commit table, details and changed files | done |
| 3 | Text grid and read-only diff viewer | not started |
| 4 | Working tree, staging, signed commits | not started |
| 5 | Clone, pull, push | not started |

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
- Text corpus: `cargo test -p egit --release --lib -- --ignored --nocapture corpus` draws the
  text grid through the roadmap's worst cases (1M lines of code, a 10 MB line wrapped and not,
  CJK and emoji, resizing wrapped at line 900k) and prints frame times, laid out and
  tessellated (it fails past 8 ms at p99); renders `corpus-*.png` where it can. Every case
  stays under 2 ms.
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
    64 KiB tail (`output.rs`); stdin is fed from another.
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
    exactly). `set_config` (`--replace-all`) and `unset_config` (`--unset-all`; exit 5 is fine)
    write `--global` from `/` or `--local` in a work tree.
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
    --name-status -M` against the first parent, or `--root`), `write_commit_graph`;
    `Repository::has_commit_graph` (without one, a topological walk outputs nothing until
    git has read all history: 6 s for Linux's 1.5M commits).
  - `diff.rs` — `file_diff`: one file's patch from `diff-tree -p` (plumbing, so no colour,
    prefix, external-diff, or textconv setting applies; `--no-textconv --full-index
    --src-prefix=a/ --dst-prefix=b/`; `GIT_LITERAL_PATHSPECS`; a rename's both paths;
    `DiffOptions`: context (`WHOLE_FILE` for all of it) and the user's `diff.algorithm`, which
    plumbing doesn't read). Parsed as it streams into `FileDiff`: each line's text without
    its prefix (hunk headers as lines), its `DiffLineKind`, old and new numbers, and a
    missing final newline; a binary file is flagged and has no lines. `lines.rs` — `Lines`,
    the streamed-output splitter `walk_history` and `file_diff` share.
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
    lands), its `HistoryState` and `HistoryView`, its `WriteLock` (writing a commit-graph),
    its own tiles (`RepoPanel::{History, Overview, Configuration}`), and its `Watcher`
    (`follow_changes`: refs or operation state changed → status read again, refs → history
    read again; config → config). `poll` (each frame, every tab) takes in its jobs' results.
  - `history.rs` — `History`: a walk (`jobs::detached`) streams `Piece`s through a channel:
    the refs and whether a commit-graph exists, then each completed `egit-graph` chunk with
    its commits' names packed (20 bytes each), then the end. `id(row)`, `row_of(id, near)` (a
    scan from `near`; no id→row index is kept), `Refs::at(id)` (sorted, binary search).
    `Objects`: one shared `cat-file` per repository, restarted after a failure.
    `Summaries`: subject, author, preformatted date, read as rows come into view (at most 512
    a read, one read at a time), about 4,096 kept, least recently drawn forgotten first.
    `read_details` (the commit and its `commit_changes`). `HistoryState`: the history shown,
    and a re-read (`reload`) that takes over only once complete (`replaced`), so the table
    never empties or jumps meanwhile.
  - `watch.rs` — inotify (`notify`) on git directories and configuration files only, never a
    work tree: a repository's git dir and common dir (non-recursive) and `refs/` (recursive);
    the user's configuration files through their folders (editors replace files). Paths are
    sorted into `Changes` (refs, state, config, index; `*.lock`, objects, logs ignored), access
    and attribute events ignored, and handed over once quiet for `SETTLE` (120 ms), so one git
    command causes one refresh. The app watches the files its user snapshot names and reads
    every configuration again when one changes. `Watching` keeps a watch that couldn't be set
    up (inotify's per-user limits, often used up by editors) trying again, backing off from
    1 s to 30 s, and hands over `Changes::ALL` once it works, as it does when inotify's queue
    overflowed (a rescan); meanwhile the status bar says outside changes aren't followed.
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
    selects in one pane between cells (hit-tested against the rows drawn last frame;
    dragging past an edge scrolls), ⌘A selects a pane, Escape clears (focused or not: egui
    drops focus on Escape), and ⌘C (`Event::Copy`) copies the bytes as they are. `Plain` is a
    text, numbered.
  - `diff.rs` — `DiffDocument`: a `FileDiff` measured as a `GridSource` (lines on their kind's
    background, hunk headers muted, the gutter numbering both files with a `+`/`-` marker), and
    `read_diff`, its job (cancelled when its view closes). Each block's removed lines pair with
    its added ones in order (found in the job); a pair's changed words are compared when first
    drawn and kept (up to 4,096 pairs), and marked with `Styled` backgrounds. `SplitDiff`:
    the old side beside the new (`split_diff`, a job, the first time it is shown), a block's
    removed lines beside its added ones with fillers making up the difference, both sides'
    measures matched (`Measure::match_rows`) so wrapped lines take the same rows.
  - `fonts.rs` — system fonts for characters egui's lack (CJK, most scripts): views note them
    (`note_missing`, `note_text`, nothing to check for ASCII); `Fallbacks::poll` (each frame)
    looks once per character, in a job, through the XDG font directories (CJK families, then
    monospace, then regular weights; in a collection the face covering most, monospace and in
    the user's `LANG` first; outlines required, so no colour bitmaps) and adds what it finds as
    lowest-priority fallbacks to both families. Kept fonts are leaked once into `'static`
    slices: egui copies owned font data on every rebuild of its fonts, but borrows static.
  - `views/history.rs` — the commit table on egui-table-kit's layout `Table` (its `TableKit`
    keeps per-row state a graph doesn't need): `CommitRows` is the `TableDelegate`; `prepare`
    replays the visible rows' lanes once a frame (`GraphRows`, panned to the nodes on screen
    when the column is narrower than the graph: "‹ Graph ›") and asks for summaries (the
    screen, then a screen below and above). Graph cells paint lines (cubic curves between
    lanes) and nodes (hollow for merges, ringed for `HEAD`); rows are buttons named by their
    subject; ref badges; a focusable area takes ↑ ↓ PgUp PgDn Home End. The message column
    is sized each frame from the others' remembered widths. `HistoryView::follow` keeps the
    selection and the top row's commit in place when a re-read takes over. Details: message,
    names, parents (buttons: egui announces links as plain text), people, signature, refs,
    and the changed files (`ChangesTable`); clicking one opens a `FileView` in the commit
    table's place (Escape or "History" goes back): its diff against the first parent (unified
    or side by side, with "Whole file" for all of its context), or the file as the commit left
    it (as it was, if deleted; read the first time it is asked for, `read_file`), or a
    placeholder for binary files, LFS pointers, submodules, and files over 256 MiB. A history of 10,000+ commits without a
    commit-graph is offered one (`Action::WriteCommitGraph`).
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
    their snapshot or commit is replaced.
  - `theme.rs` (palettes, WCAG AA checked by a test; Phosphor icons), `widgets.rs` (cards,
    buttons, origin labels, problem boxes), `format.rs` (`~` paths, "5 minutes ago",
    dates), `settings.rs` (theme, recent repositories, open tabs; non-UTF-8 paths kept as
    bytes), `problem.rs` (`Problem`, `OpenProblem` with its `safe.directory` remedy).
  - `tests/askpass_helper.rs` — the real `egit` executable (`CARGO_BIN_EXE_egit`) started by
    git as its helper through the server's link.
  - `src/tests/screens.rs` — the whole application driven through the accessibility tree
    (egui_kittest) against a sandboxed git; renders `target/gui-screens/*.png` where a wgpu
    adapter exists (`EGIT_GUI_RENDER=1` requires one, `0` skips). Look at them after any
    interface change. Every rendered screen first passes `src/tests/audit.rs` (after eshine's):
    no egui id clashes, no text or control drawn partly over another on the same layer,
    nothing moving while idle, and every control named in words or numbers (link a field to
    its visible label with `labelled_by`). Not yet audited, as eshine does: hover response,
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
  a job. Colours come from `theme::Palette::of(ctx)`, icons from `theme::icon`, shared pieces
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
