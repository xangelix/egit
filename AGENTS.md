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
| 26 | Follow-ups: columns, long text, dates, Help for every word, signing commits picked together | done; awaiting the owner's review |
| 27 | People's pictures, and hooks' output | done; awaiting the owner's review |
| 28 | Agents (Claude), on this computer | done (a second UI pass, files, commands, images, code); awaiting the owner's review |
| 29 | Repositories on other computers, over SSH | done; awaiting the owner's review |
| 30 | GitLab's CI/CD pipelines: a mark on each commit, a pipeline's stages and jobs | done; awaiting the owner's review |
| 31 | Signing in to GitHub and GitLab, offered wherever it counts, with a token | done; awaiting the owner's review |

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
- Git on another computer: `EGIT_TEST_SSH=1 cargo test -p egit-git` runs every egit-git test
  with each git command going through an sshd of the tests' own on the loopback address (Linux,
  sshd installed): this computer playing the other one. Do this after touching how commands
  are built or run, or anything reading files directly (CI does it). Following changes there
  at once needs inotify-tools (`inotifywait`): its tests skip without it unless
  `EGIT_REQUIRE_INOTIFYWAIT` is set (CI sets it).
- Supply chain: `cargo deny check` (`deny.toml`) and `cargo shear` (unused dependencies).
- Agents: the bridge's tests need Node.js (`EGIT_REQUIRE_NODE=1` makes their absence fail);
  `cargo test -p egit-agents --test install -- --ignored` installs the Claude Agent SDK for real
  (about 250 MB from npm's registry) and loads it. Moving to a newer SDK: change its version
  in `bridge/claude/package.json` and `install::SDK_VERSION`, then `npm install
  --package-lock-only --legacy-peer-deps --ignore-scripts` there. Nothing in a test signs in
  or sends a prompt to Anthropic.
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
  of 2.8 ms, each letter typed 4.1 ms at most; peak 179 MB. `corpus_markdown`: a Markdown
  document of 1,020 KiB changed (each of its 2,600 headings), previewed before and after: read,
  compared, and first drawn in 13 ms, wheel-scrolled at 5.0 ms p99 (drawn whole, as
  `egui_commonmark` draws a document, 780 ms a frame and 943 MB), peak 214 MB.
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
  then whole again: each read again and shown once whole (Linux: 1.4–1.5 s). And the Stats
  page: its figures land in 0.7 s for Linux (65 ms for git's own 86k commits), peaking at
  1.2 GB while the packs are read (mapped, released as it ends; 44 MB more allocated after,
  most of it the page's own drawing); the newest 10,000 commits' lines counted in 2.1 s (git's;
  none in Linux's partial clone); the page scrolled at 0.36–0.51 ms p99.
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
  files under `.git` directly, except to watch them, to read the few state files git
  documents (`MERGE_HEAD`, `rebase-merge/`, …), and to add ignore patterns to `info/exclude`
  (found by `rev-parse --git-path`), which gitignore(5) documents for people to edit. No
  libgit2 (`deny.toml` bans it).
- What egit reads or writes itself (state files, `.gitignore`, a rebase's todo, a file put
  back) goes through `Git::files`, which works where git runs: on another computer, by a
  command there. Never `std::fs` on a path git gave, nor a temporary file here that git there
  must read (`Files::temp_dir`).
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
    with the character set kept, no pager, editor `:`, `GIT_TERMINAL_PROMPT=0`, no `GPG_TTY`);
    its `Host` (`GitOptions::host`): here, or another computer, where git's environment is
    `Environment::remote` (egit's own, never this computer's helpers: the askpass, the ssh
    wrapper, a display, which the ssh reaching it gets instead, `is_local_only`), and the
    program `git` there unless named, and its home folder read once reached (`home_there`:
    `$HOME` there, `GitError::NoHome` when it isn't a folder). `Git::files`: that computer's
    files (`files.rs`).
  - `host.rs` — `Host` (`Local`, `Ssh(SshHost)`: a destination as ssh takes it, more of ssh's
    options (before egit's own: ssh keeps the first value of each), and the folder its
    connection is shared through, an `SshSharing`'s: `ControlMaster=auto`, `ControlPersist=1h`).
    A command there is a session (`ssh -T -e none`, its key held and connection shared as
    git's ssh: `AddKeysToAgent`, `ControlMaster`) whose remote line is fixed, `exec /bin/sh
    -c 'eval "$(printf %s <base64> | base64 -d)"'`, so any login shell passes it on; the script
    (`script`, every word in single quotes for sh) says `egit-pid <pid>` on stderr, goes into
    its directory (exit 126 when it can't: "isn't a directory", as here), marks the start of
    its output (`\x01EGIT-GO\x02`), and `exec env -u … K=V … git …`. `GitStdout` skips what
    comes before the marker (a login script's words), a byte at a time, then passes through;
    `PidReader` takes the id from stderr, holding back what came before it (kept only when it
    never came: ssh's words on a login that failed). `SshHost::terminate`: `kill -TERM --
    -<pid>` there (sshd made the session's shell lead its group; git `exec`s in its place).
  - `files.rs` — `Files`, a `Git`'s computer's files as egit reads and writes them directly:
    `read`, `read_all` (several in one command there: each a size line, then its bytes),
    `exist`, `exists`, `is_file`, `runnable` (hooks), `look` (`Entry`: nothing, a link and its
    target, a file's size and executable bit, a folder), `write`, `append`, `put` (`Put`: a
    file written beside and renamed over, its permissions the replaced one's with the
    executable bit as asked; or a link), `remove_file`, `remove_if_there`, `create_dir_all`,
    `temp_dir` (`TempDir`, removed when dropped, there too), `folders_within` (folders and those
    inside them, `.git` left out, at most so many: breadth first here, `find` there). Here
    `std::fs`; there `sh -c` with
    the paths as arguments, one session for several paths where it can (a missing path exits 3:
    `NotFound`).
  - `looking.rs` — following a repository on another computer: `look_there` (a `Follow`: how
    often to look, and the `inotifywait` to be told by) runs a session there that, where
    `inotifywait` is installed, runs it on the git directories, `refs` (recursive), and the
    user's configuration's folders, each path it names sorted by `classify` (a `Looked` set:
    refs, state, config, index; lock files, objects, logs nothing; shared with the app's
    watches); live once both of its watchers say their watches are set up; one stopping
    (inotify's limits) falls back to the `sh` loop looking every few seconds (refs through
    their folders' times, `packed-refs`, `HEAD`'s text; an operation's files; the
    configuration's and index's times), as where it isn't installed. `look_at_folders_there`:
    a work tree's folders (each on its own, NUL-separated on its input, those gone left out by
    `xargs`, listed in a temporary file for `inotifywait --fromfile`), each change `Seen`
    (ready, changed, a folder created, missing, failed with its words). Every session says `.`
    every 5 s (a reader gone without a word ends it) and stops its watchers, and itself, as soon
    as its input ends (`Looking` holds it: dropped, the session closes). `Liveness`, how one
    stands, shared. `tests/looking.rs` (through the tests' sshd; the live ones skipped without
    inotify-tools unless `EGIT_REQUIRE_INOTIFYWAIT`, which CI sets): told at once of a commit,
    a file staged, a bisection, both configurations, branches in a new folder; looking every
    second without it, or with one that stops; folders followed and no others, a folder made
    seen; every watcher there stopped at once when dropped.
  - `command.rs` — `GitCommand` from `Git::read` / `Git::write`: `-c` settings, arguments,
    directory, stdin, and a `CancelToken`; `output` (collected up to `stdout_limit`, failing
    unless git succeeds), `output_unchecked` (any exit), `stream` (chunks through one reused
    buffer; the consumer may stop it). Reads add `--no-optional-locks`; writes are retried
    (25 ms … 800 ms) while `Stderr::held_lock` finds another git's lock. Every command gets
    `-c color.ui=never -c core.quotePath=false`. Stderr is drained on a helper thread into a
    64 KiB tail (`output.rs`), and into a `Progress` when the command has one (`progress`);
    stdin is fed from another. Secrets: `secret_stdin` and `secret_output` keep them in
    zeroizing buffers (growing never leaves a copy), never shown in debug output. A writing
    command run through a `Git` given a transcript (`Git::with_transcript`) writes into it: its
    command line (`git commit --file=-`, egit's own `-c` left out), standard output and error
    as they come (teed from the threads reading them), and how it failed or was cancelled; with
    `PRE_COMMIT_COLOR=always`. A secret's command (`secret_stdin`, `secret_output`) never.
    On another computer (`build_elsewhere`): the ssh running git's script there, git's
    environment there plus the command's but this computer's helpers, which the ssh gets (a
    prompt's context reaches the askpass); its stdout a `GitStdout`, its stderr through a
    `PidReader`; a `CancelToken` cancelling it asks its group there to stop (`Elsewhere`, from
    a thread, once its id came) as well as the session here.
  - `transcript.rs` — `Transcript`: lines (`LineKind`: command, out, err (hooks' output: git
    sends it there), ended), a `\r` rewriting the line under way as a terminal does (shown live),
    SGR colours turned into `Span`s (`Style`: 16, 256, or exact colours, bold, faint; a colour
    lasting across lines until reset), other escapes dropped; at most 20,000 lines (the oldest
    dropped) of 16 KiB; `read` lends the `Log` (only the lines shown are touched).
  - `hooks.rs` — `installed_hooks`: the folder git runs hooks from (`rev-parse --git-path
    hooks`, which follows `core.hooksPath`) and those of `KNOWN` there that it would run
    (executable on Unix; never a `.sample`); `Hooks::of` picks a task's. `tests/hooks.rs`
    lists them (an unrunnable one and a sample left out; `core.hooksPath`), and transcribes a
    commit's hook: its colours and `PRE_COMMIT_COLOR`, a refusal and its end, `--no-verify`
    skipping it, and one cancelled part-way (no commit, quickly).
  - `progress.rs` — `Progress`: a long command's `--progress` lines read as they come, each
    replacing the last (git rewrites them with `\r`): the phase, done and total, the transfer
    (`1.20 MiB | 2.30 MiB/s`), whether the remote is doing it (`remote: `), and the other
    lines as notes (the latest fifty). `step` reports work egit does itself one item at a
    time (commits signed) the same way.
  - `process.rs` — `Detached`: a program other than git (an agent's bridge, npm) run as git is:
    leading a group of its own without a terminal, `stop` waiting, then asking the group to
    stop (SIGTERM; Windows' gentle way), then killing; never signalled once reaped.
    Git leading a group of its own, stopped and waited on safely, per platform
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
  - `ssh.rs` — `SshSharing`: a key's passphrase held and ssh's connections shared while egit
    runs, so it is asked once (Unix: OpenSSH for Windows can't). A directory of its own
    (`egit-<uid>/ssh/<pid>-<nonce>`, 0700, under `$XDG_RUNTIME_DIR`, `/tmp`, or the temporary
    directory: the first where a socket's path, `%C` and ssh's 17-character suffix, fits
    `sun_path`) holds a wrapper, `ssh`, given to git as `GIT_SSH`: `exec ssh -o
    AddKeysToAgent=yes -o ControlMaster=auto -o ControlPersist=1h -o ServerAliveInterval=15 -o
    ControlPath=<dir>/%C "$@"`. A key whose passphrase is typed goes to an agent (`Holding`): the
    one egit's environment names (`SSH_AUTH_SOCK`) where it answers, as `ssh-add` would; else
    one of egit's own (`ssh-agent -a <dir>/agent cat`, its `SSH_AUTH_SOCK` given to git: `cat`
    reads egit's pipe, so the agent ends however egit does; dropped, `ssh-add -D` first), so a
    connection the server drops or won't share (a second session refused) is made again
    without asking, a background fetch's too; without `ssh-agent`, connections are only
    shared. git runs the wrapper only where neither `GIT_SSH_COMMAND` nor `core.sshCommand`
    names an ssh (and none is started when egit's environment sets `GIT_SSH_COMMAND` or
    `GIT_SSH`); its basename keeps git's OpenSSH variant. Dropped (the app's `on_exit`), each
    master is closed (`ssh -F /dev/null -S <socket> -O exit`) and the directory removed;
    those of egit processes gone are swept the same way. `tests/ssh.rs` (Linux; skipped
    without sshd unless `EGIT_REQUIRE_SSHD`, which CI sets) runs an sshd of its own and a key
    with a passphrase (never the tester's agent: `SSH_AUTH_SOCK` emptied): a clone, two
    fetches, and a push ask once; the master closed (a server dropping it), a fetch and a
    background fetch ask nothing (egit's agent holds the key), nor a repository's own
    `core.sshCommand`; without sharing each fetch asks; dropped, the master exits and the
    agent goes.
  - `batch.rs` — `GitProcess` from `GitCommand::spawn`: a git kept running for a batch
    protocol (stdin requests, stdout answers); dropping it closes stdin, SIGTERMs its group,
    reaps it.
  - `objects.rs` — `ObjectReader`: one `cat-file --batch-command --buffer` per reader;
    requests go 128 at a time then `flush` (both pipes stay under their buffers), answers are
    read into one reused buffer. `commit.rs` — `Commit::parse` (raw commit objects; `subject`
    is `%s`; `co_authors`, each `Person` its `Co-authored-by` trailers name, in any case,
    once). `people.rs` — `recent_authors`: who wrote the last commits of every branch (`log
    --all --no-merges`, the stash left out), through the mailmap (`%aN`, `%aE`), most commits
    first, each email once; `map_contacts`, any number of `Name <email>` through the mailmap
    (`check-mailmap --stdin`; one it can't read passed through as it is).
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
    `list_commits` (any commits, in the order given: `log --no-walk=unsorted --stdin -z`, each
    `ListedCommit`'s name, parents, and subject); `commit_lines` (each commit's lines added and
    removed, `CommitLines` with its time and mailmapped email, streamed: `log --no-walk=unsorted
    --stdin --shortstat`, never fetching in a partial clone);
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
    missing final newline; a binary file is flagged and has no lines; `blobs`, its `index`
    line's (the work tree's side as git would store it). `lines.rs` — `Lines`,
    the streamed-output splitter `walk_history` and `file_diff` share (`ended_by(0)` for `-z`
    records).
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
  - `ignore.rs` — ignoring files git doesn't track: `path_pattern` (anchored at the top, a
    folder's ending with `/`) and `extension_pattern` (`*.log`), each escaping what git would
    read as a glob (`*`, `?`, `[`, `\`) and trailing spaces, a line break refused; `ignore`
    appends those not held already to the top `.gitignore` (`IgnoreIn::Shared`) or
    `info/exclude` (`Private`: `rev-parse --path-format=absolute --git-path`), in the file's
    own line endings, after a newline the file lacked; `Ignored` (the file, what was added,
    whether it was made) for `unignore` (its end cut off, the file removed if made for it;
    `IgnoreError::Changed` when it no longer ends so) and `ignore_again`. `tests/ignore.rs`:
    names with glob characters, `#`, `!`, trailing spaces, a backslash, each ignored alone
    while a look-alike stays listed (by `git status`); an extension and a folder in a CRLF
    `.gitignore`; taken back, refused once changed, a file made for them removed.
  - `discard.rs` — discarding changes not staged. `discard_files`: tracked files back to what
    the index holds (`restore --worktree`, literal paths on standard input), untracked ones
    removed (`clean --force`, 16 KiB of paths a command); `discard_lines`: the lines a diff's
    `picked` chooses put back, `apply_lines` of every change but those onto the index's blob,
    written through the file's filters (`cat-file --filters --path`), refused (`Stale`) unless
    the index holds the entry the diff was read with and the file is still the diff's side
    (`hash-object` through its clean filters against `blobs`); a link's or a submodule's
    lines aren't (`NotLines`). Each file is kept before and after (`KeptFile`: a blob of its
    bytes as they are, `hash-object -w --no-filters`, `--stdin-paths` together, a link's
    target apart; its mode), none past `KEEP_FILES` (1,000) or a file over `KEEP_BYTES`
    (64 MiB); `put_back` writes either side again (`DiscardSide`), once every file is as the
    other left it (by its bytes and whether it is a link), read through one `cat-file`, each
    written beside its place and renamed over it (permissions kept, the executable bit as its
    mode says; links made where they can be). `tests/discard.rs` discards, puts back, and does
    again whole files (a change with some staged, a deletion, an untracked executable, a
    link) and lines (a hunk of two; of a file whose line endings git converts; of an untracked
    file, of a deleted one), refuses a stale diff and putting back over a change, and keeps
    nothing of a file too large.
  - `tree.rs` — for watching a work tree's folders: `tree_folders` (a commit's folders,
    `ls-tree -r -d`) and `ignored_paths` (`check-ignore -z --stdin`, which exits 1 when none
    is; folders end with `/` so folder-only patterns match them); and `tree_entry`, one path's
    entry at a commit (`ls-tree -z --full-tree`, a literal pathspec), or none; `tree_files`,
    every file at a commit streamed with its size (`ls-tree -r -z -l`; sizes left out in a
    partial clone, where they'd be fetched; submodules left out; `tests/stats.rs` lists them
    (a name holding a newline), counts each commit's lines (a merge's and an empty one's none,
    an email through the mailmap), and maps contacts). `git_path`: a
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
    labelled "Signing the commit"; `no_verify` skips `pre-commit` and `commit-msg`;
    `Committed::output` is what git and its hooks printed (a
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
  - `signing.rs` — signing commits already made, as `rebase --force-rebase --gpg-sign` would,
    without a rebase. `plan_signing`: the branch checked out, every commit asked for in its
    history (`rev-list --count --stdin` of them `^tip`: none outside), its commits newest
    first (`--topo-order --parents`, children before parents) until every one asked for has
    come, so all after them have too, then oldest first each made again when asked for or a
    parent is (`SigningPlan`: those, how many asked, merges, and whether the upstream holds
    one: `merge-base --is-ancestor` of the lowest asked for); at most `MAX_SIGNED` (10,000).
    `sign_commits`: each read through one `cat-file`, made again by `commit-tree -S` with its
    tree, author (`GIT_AUTHOR_DATE=@<seconds> <offset>`), message byte for byte, encoding, and
    parents as made again (the committer is whoever signs, now), labelled "Signing the
    commits"; then one `update-ref -m "egit: signed N commits"` checking the branch's old
    value (`SignError::Moved` otherwise). Nothing is checked out or merged again: the work tree
    and index stay, merges keep their resolutions, no hook runs; `mergetag` headers are left
    behind. `tests/signing.rs` signs one commit of a history holding a hand-resolved merge
    (trees, author and date, the merge's resolution and second parent, the side branch, an
    uncommitted change all kept; signatures good), and refuses nothing asked for, a commit off
    the branch, a branch that moved, a detached `HEAD`; a pushed commit is said to be; and a
    revert is signed as commits are (`commit.gpgSign`, git's message).
  - `remote.rs` — moving commits, each with `--progress` into a `Progress`, a `CancelToken`,
    and askpass prompts labelled by what is done: `clone` (from `/`; `CloneOptions`:
    submodules, partial `--filter=blob:none`, both with `--also-filter-submodules`),
    `fetch` (`FetchOptions`: `--all`, `--prune`, and in the background `non_interactive` and
    `--no-auto-maintenance`), `pull` (`PullMode`: as configured, merge, rebase, fast-forward
    only; `Pulled::Diverged` when git says the branches diverged, `Pulled::Conflicts` when a
    merge or rebase stopped (from the repository's state); with `autostash`, `--autostash`:
    changes put back after (staged ones not staged), or once a stopped merge or rebase is
    continued or aborted; `Pulled::ChangesInTheWay` when they are, as git names them (none
    when a rebase wants the work tree clean; untracked ones, which `--autostash` leaves, said
    so: `branch::overwritten`, shared with the switch), `ChangesConflicted` when they came back
    with conflicts (seen as a new `refs/stash`: git's words for it differ between versions)),
    `push` (`--porcelain`, one branch
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
    git removes its folder; a pull with changes in its way: named, put back, put back with
    conflicts, untracked).
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
    `--binary`, prefixes, no external diff or textconv, from the top). Any commits (the newest
    first): `format_series` (one patch each, the oldest first, in one mailbox: `format-patch
    --stdout --no-walk=unsorted --stdin`, git listing them the other way round; `Series`: the
    text and how many, merges left out) and `format_series_into` (`-o` a folder);
    `combined_patch`, their changes as one diff under a list of them: from the oldest's parent
    (the empty tree for a root) to the newest when each is the next's parent, else each merged
    in turn onto those before it (`merge-tree --write-tree --merge-base=<its parent>`, onto
    unreferenced commits `commit-tree` makes; `CombineError::Conflict` names the commit that
    needs one left out). `tests/exporting.rs` lists commits, makes a series that `git am`
    applies, and combines commits that follow each other, commits apart, a root's, and refuses
    a conflict.
  - `foresee.rs` — what bringing commits in would do, worked out without touching the work
    tree, the index, or a ref (`Foreseen`: up to date, a fast-forward, clean, empty, the files
    in conflict and the commit a rebase stops at, or too long): `foresee_merge` (`merge-base
    --is-ancestor` both ways, then `merge-tree --write-tree -z --name-only --no-messages`),
    `foresee_pick` (a cherry-pick, or a revert: the commit's first parent or the commit as
    `--merge-base`; empty when the tree is `HEAD`'s; none for a root commit), and
    `foresee_rebase` (the commits of `onto..head`, merges left out, at most `MAX_REPLAYED`
    (100), replayed in turn onto unreferenced commits `commit_of` makes, as the combined patch
    does: git 2.43's `merge-tree` takes commits, not trees). Trees, blobs, and those commits are
    written, referred to by nothing; `GIT_NO_LAZY_FETCH`, never asking. `tests/foresee.rs`
    holds each to what git's merge, revert, and rebase then do (the files, the commit a rebase
    stops at).
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
  - `integrate.rs` — commits brought into the current branch: `merge` (`--no-edit`; `FastForward`: as `merge.ff`
    says, or never, `--no-ff`: a merge commit even where the branch could just move forward; a
    ref's full name shortened as far as it stays unambiguous, `rev-parse --abbrev-ref=strict`,
    since git's message names what it is given: "Merge branch 'topic'", not
    "'refs/heads/topic'"),
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
    (protocol, host, and the user when a host has several; refused with line breaks, which
    would inject attributes), in a
    repository's configuration or the user's (`/`), `Asking::Never` (non-interactive: no
    credential is `None`, not an error) or labelled prompts; `approve_credential` and
    `reject_credential` hand back every attribute `fill` gave (an expiry, a refresh token).
    A `Credential` is wiped when dropped; `Credential::given` makes one of a secret the user
    gave (a token pasted), as `fill` would. `store_credential` hands a secret of egit's (an
    agent's API key, under its account's name) to the helpers as approved, `erase_credential`
    has them forget a place's. `tests/credential.rs` fills, rejects, and approves through a
    sandbox's `store` helper, and stores two users' secrets for one host, erasing one.
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
    `EGIT_TEST_SSH` runs it as on another computer (`on_another_computer`): an sshd started once
    a process (`/tmp/egit-ts-<pid>`: keys of its own, `-F /dev/null`, `BatchMode`,
    `ControlPersist=5`), kept by a shell that stops it and removes its folder when the test
    process's end closes its input; tests of what only this computer does (the askpass, git's
    own ssh) skip then. `set_mode` (a script's permission bits, where files have them).
- `crates/egit-forge` — forges' web APIs: GitHub's (github.com, Enterprise Server) and GitLab's
  (gitlab.com, self-managed), and people's pictures; no egui. egit's only network connections
  of its own (git makes every other).
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
    option), or `resolve_ssh_alias_on` a git's computer (a repository there names its remotes
    by that computer's ssh configuration).
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
    `sign_in_with_token`: a token the user gave, checked first by an account of its own that
    never tells the helpers anything (a mistyped one makes them forget nothing), then kept
    under the login the host says it is (git finds it for the host's HTTPS remotes too);
    `SignedIn` (the login, whether the helpers keep it: `fill` gives it back, and the
    credential). `adopt` (another account's sign-in), `who` (the login, or asked with what the
    helpers keep; none without), `sign_out` (the helpers forget it), `signed_out` (forgotten
    here alone). `tests/github.rs` refuses a mistyped token (the helper's entry kept), keeps
    the right one under its login, has another account adopt it, signs out, and lasts a run
    where no helper keeps it.
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
  - `pictures.rs` — `PictureSources` (`public`: GitHub's avatars by account number, its
    profiles by login, its lookup by email, Gravatar's): `urls` for an email, in order: a
    GitHub no-reply address's account alone (`12345+login@users.noreply.github.com`; an older
    `login@…` by its profile); else GitHub's lookup by email (`avatars.githubusercontent.com/
    u/e?email=`: the account that verified the address, as GitHub shows commits' authors,
    public or not), then Gravatar's by the trimmed lower-case address's SHA-256 (`sha2`),
    `d=404` so someone without one says so. GitHub answers an address of no account with a
    placeholder (`is_github_placeholder`: its known SHA-256, or a PNG wider than asked for),
    which counts as none. `Pictures::find` asks them in order; `fetch`: no credential, so
    redirects are followed, HTTPS only (but every source on the loopback address, for tests),
    1 MiB at most; `Fetched::Found` or `Missing` (404).
  - `pipelines.rs` — GitLab's CI/CD pipelines through its GraphQL API (`has_pipelines`: GitLab
    alone; GitHub's Actions are a pull request's checks): `pipelines` (a project's newest, 100 a
    page, each page handed over as it lands, up to a limit; none for a project whose pipelines
    can't be seen), `pipeline_details` (one by its `iid`: its stages in order, each one's jobs
    (their latest runs), each job's page, a trigger job's its downstream pipeline's).
    `Pipeline` (GitLab's number from its global id, `iid`, commit, `CiStatus`, warnings, ref,
    made, page; `is_newer_than`), `PipelineDetails` (started, finished, duration, who, how,
    `Stage`s of `Job`s: status, may fail, manual, duration, page). `CiStatus` (GitLab's every
    status, read however it writes them: `RUNNING`, `running`, `waiting-for-resource`; `word`,
    `is_moving`: under way or waiting for its time) and its `CiGroup`, the five egit colours:
    passed, failed, under way, paused (waiting for someone or its time), cancelled or skipped.
  - `testing.rs` (feature `testing`) — `FakeForge`: either forge's API on the loopback address
    (pages, details, repository, who is signed in, opening; GitLab's pipelines, `FakePipeline`
    and `FakeJob`, a page or one whole; the bearer token checked; a rate limit to switch on;
    requests recorded). `tests/github.rs` and `tests/gitlab.rs` run each call against it,
    signed in through a sandbox's `store` helper (and an askpass script): pipelines a page at a
    time and one whole (stages in order, jobs, pages), and none asked of GitHub.
- `crates/egit-agents` — agents working in a repository beside egit: Claude Code, driven
  through Anthropic's Claude Agent SDK (TypeScript) by a small bridge under Node; no egui.
  - `bridge/claude/` — `bridge.mjs` (plain JavaScript: the SDK's `query` with streaming input,
    Claude Code's own system prompt (`claude_code` preset, recorded), the user's, project's, and
    local settings, partial messages; `canUseTool` turned into permission events answered by
    egit; a prompt's images as base64 blocks before its text; interrupt, mode, model, info
    (said once started, unasked: who is signed in, the models, the slash commands; Claude Code
    asks nothing of Anthropic until a prompt); `sessions` and `history` run once;
    `EGIT_CLAUDE_SDK` names a stand-in SDK for tests), `package.json`, and `package-lock.json`
    (the SDK pinned, `--legacy-peer-deps`: only Anthropic's packages, each with its integrity).
  - `protocol.rs` — JSON lines: `Request` (start (and its account's variables where the bridge
    runs on another computer: `Secrets`, sent rather than set, never shown in debug output,
    wiped when dropped), send (its `Image`s: a media type and base64,
    `Image::of`), answer, interrupt, mode, model, info), `Event` (ready, message (an SDK
    message as it is), permission, withdrawn, info, failed, ended); `Mode` (Claude Code's permission modes and their names), `Answer` (allow, always:
    the suggestions sent back as rules; input replaced: a question's answers; refusal's
    message, interrupt), `PermissionRequest`, `Info` (account, models, commands).
  - `transcript.rs` — `Transcript`: the SDK's messages (live, or `history`'s) read into `Item`s
    (prompt (`Prompt`: its text and the names of the images attached; a past one's image blocks
    as "an image"), text, thinking, tool call with its result, a turn's end with its cost, tokens,
    and error, notices: compaction, retries, limits, refusals); streamed text and thinking
    written into a live item made whole by its message; subagents' items under their call
    (`parent`); each item's `changed` against `version`, so views measure again only what
    changed; unknown messages ignored. `ToolCall::summary`: ("Read", path), ("Run", the
    command's first line), …
  - `install.rs` — `find_node` (Node 18+ and npm on `PATH`); `Install::at(cache)`: a folder per
    SDK version and hash of what egit carries; `install` writes the package beside it, runs
    `npm ci --ignore-scripts --legacy-peer-deps` (`Detached`, cancellable), checks Claude
    Code's binary for this platform is there (`claude()`), marks it ready, renames it into
    place, and removes older ones. On another computer (`Install::there(git)`: under
    `${XDG_CACHE_HOME:-$HOME/.cache}/egit` there; `find_node_there`, its `PATH`'s): the same done
    there through its git (`Git::command_there`, `Git::files`; `sh_there` for the small steps;
    Claude Code's binary looked for among Linux's and macOS's packages; `is_ready` and `claude`
    then ask the other computer: in a job).
  - `bridge.rs` — `Launch` (node, the script, environment set and unset, and `there`: the
    other computer's git it runs through, a session of the shared connection, its output read
    from the script's marker (`GitStdout::of`), its process id's line kept out of its words
    (`PidReader`); never a secret in its environment there, a command line there would show
    it; what it doesn't get, there through `env -u`); `Bridge::start`: a
    conversation's process, its lines read on a thread into `Event`s (a last `Ended` always,
    with the stderr's end when it died), `send`; dropped, its input closes and it is waited
    for on a thread (stopped after 3 s). `run_once`: one JSON answer.
  - `accounts.rs` — `Signer` (Claude Code as the terminal runs it, a configuration folder of
    its own (`CLAUDE_CONFIG_DIR`, other credentials left out), an API key), its environment;
    `auth_status` (`claude auth status --json` with the bundled binary, there through its git,
    other credentials left out by `env -u`; never an API key there:
    `AgentError::SecretOnCommandLine`);
    `login_command` (the command signing into a folder with Claude Code itself, there through
    `ssh -t <host>`: egit has no Claude login).
  - `sessions.rs` — `list_sessions` (newest first, with branch and title), `session_history`.
  - `tests/bridge.rs` drives the real bridge under Node against `tests/stand-in/sdk.mjs`
    (streaming, allowed once and always, refused, a question answered, interrupted, mode and
    model, info, said too once started; images sent with a prompt; the bridge run on another
    computer through the tests' sshd, its key sent in the start request; sessions listed and read;
    skipped without Node unless `EGIT_REQUIRE_NODE`,
    which CI sets). `tests/install.rs` (ignored: `-- --ignored`) runs the real `npm ci` and
    loads the real SDK (an empty folder's sessions, its login status).
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
  - `sequences.rs` — `kept`: which elements of two versions of a list they share (any `Eq +
    Hash`): the common start and end set aside, then those found once on each side anchor the
    rest, as git's patience diff does (the longest run of them in order on both, by patience
    sorting), and each stretch between anchors is aligned by Myers' O(ND) algorithm (its own
    common start and end aside); past `MOST_EDITS` (1,000) edits a stretch counts as changed.
    Tests: what is kept reads the same on both sides, Myers' pairs are a longest common
    subsequence's (500 random cases, against the slow way), and 2,000 sections with every
    heading renamed mark the headings alone.
  - `find.rs` — `find_all`: a query's matches as byte offsets (`memmem`, or ASCII case folded
    by a two-byte scan), in 4 MiB windows overlapping by the query's length, asking whether
    to stop between them; `matches_case` is smart case; `Needle`, a query made ready for
    many short texts (a finder built once, or ASCII folded).
- `crates/egit` — the application (`egit_app` library, `egit` executable). eframe/egui 0.36
  (wgpu), `egui_tiles` 0.17, `egui_plot` 0.37 (egui's charts: the Stats page),
  `egui_commonmark` 0.25 (Markdown previewed; no default features, so it loads no image),
  `egui-table-kit`
  0.7 (a git revision ahead of crates.io: bump
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
    (`Bind<Git, Problem>`); repositories asked for before it is ready wait in `waiting`. A
    repository opens at a `Location` (`settings.rs`: a path here, or a host as ssh reaches it
    and a path there; `Action::Open`, `OpenBeside` for a worktree's or submodule's on the
    computer its tab's is on): there, a `Git` on `Host::Ssh` is reached first in the job (as
    git here is run, `options_there`, through `AppState::ssh`'s shared folder; `reach`, a
    failure saying ssh's words, `reach_detail`), then the folder typed read as a shell there
    reads it (`views::home::folder_there`: `~` and a relative path the home folder there's),
    then discovery; `Action::Opened` carries the git with the
    repository, kept as `RepoTab::git` (every repository action runs with its tab's git:
    `with_repo`), and trusting (`safe.directory`) runs there too. The tabs open at the last
    close (`Settings::open`, `OpenTab`: a path, or a host and a path) come back. Profile edits are `ConfigChange`s
    (set or unset a key, add or remove one of its values) written under the user's
    `WriteLock`, then the user's and every repository's settings are read again. Web pages go
    through `AppState::opener` (`desktop.rs`'s `Opener`: the browser, or for tests a list,
    `recorded`; `Action::OpenUrl`).
  - `jobs.rs` — the runtime (2 workers, at most 16 blocking jobs: git processes queue
    beyond), `blocking` (a closure on the blocking pool as a future for a `Bind`), and
    `WriteLock` (taken inside jobs, never on the interface thread).
  - `tabs.rs` — the window's `egui_tiles` tree: `Pane::{Home, Profile, Preferences,
    Repo(id)}` (a repository's on another computer titled with it, "repo · devbox", under a
    globe); `show` focuses or adds a tab; `front` (the tab in front), `close_front`,
    `show_nth` (⌘W, ⌘1 … ⌘9); Home can't be closed; closing a repository's tab pushes `Closed`.
    Tabs underline under the pointer (`widgets::tab_hover`; egui_tiles' don't answer it), and
    their close button is 24 points. The selected tab is the chrome's tone, joining what it
    shows below; the bar and the other tabs are deeper (`Palette::recess`).
  - `preferences.rs` — `Preferences` (kept in the settings, every field defaulted so older
    settings read): the text's size (70–200%, egui's zoom, which ⌘+ ⌘− ⌘0 change too: followed
    back), the background fetch (`FetchEvery`: never, every minute, 5 (the default), 15, or 30
    minutes), the sidebar shown when a repository opens, people's pictures or their initials
    (`pictures`), whether the output opens by itself (`output`), a file's first look (`FilePreferences`:
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
    output shown or hidden (⌘J), the agents shown or hidden (⌘L), the
    reflog, the work tree, the requests, stash, patches, a branch made or switched to, read
    again), each's words and shortcut; `pressed` takes the application's own shortcuts at a
    frame's start (⌘K and ⇧⌘P, ⌘,, ⌘O, ⌘W, ⌘B, ⌘R, ⌘J, ⌘L, ⌘1 … ⌘9; the views take theirs), the more
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
    once gix failed; `EGIT_GIX=0` leaves every read to git, and so does a repository on
    another computer (`Objects::of`, `gix_allowed_for`: gix reads files here; the Stats page
    and the search alike). The walk takes its refs and tips
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
    `read_details` (the commit and its `commit_changes`); `read_comparison` (two commits: the
    newer's details with the older as its only parent, so the details' files, counts, and
    diffs run from one to the other); `read_change_counts` (its
    `line_counts`, lined up with the changes by path should their orders differ, and their
    total). `HistoryState`: the history shown,
    and a re-read (`reload`) that takes over only once complete (`replaced`), so the table
    never empties or jumps meanwhile.
  - `sign_ins.rs` — signing in to GitHub and GitLab hosts (`AppState::sign_ins`): the dialog
    (`SignInDialog`: its host, the account it signs in through, a repository's (its own
    credential helpers) or the user's, the token typed, `with_token`, `with_helper`, how it
    went), who is signed in to each host (`who`, asked once each in a job), signing out; `poll`
    hands what finished (`Landed`) to the window, which passes a sign-in to every open
    repository's account for the host (`RepoForge::signed_in`), or a sign-out
    (`signed_out`). `Action::SignIn` (from a repository's banner, top bar, sidebar, or
    requests' card: its account; from Profile: the user's) and `Action::SignOut`. The banner
    under a repository's top bar (`views/history.rs`'s `sign_in_banner`: what signing in shows,
    Sign in…, and Not now, which hides it for that host from then on, kept in egui's memory).
  - `ci.rs` — a repository's GitLab pipelines, for the commit table's marks (`Pipelines`, its
    forge's: `RepoForge::pipeline_of`, `has_pipelines`): each GitLab project's newest `LIMIT`
    (500) read in a job when the remotes are known, and again whenever the requests are
    (`RepoForge::refresh`: after a task reaching the remotes, each background fetch, a sign-in,
    Refresh); while any shown may still change, the newest page read again every `EVERY`
    (20 s) and merged in. `Marks`: each commit's latest (`CiMark`: its project's place and the
    `Pipeline`), of any project, sorted by commit (a binary search a row); a read takes over
    only once complete, one that fails keeping what is shown (a host needing a sign-in or rate
    limited skipped for its other projects).
  - `stats.rs` — a history's statistics, for its Stats page: `read_stats` reads every row's
    commit once, through gitoxide on up to 8 threads a block of rows at a time (else one
    `cat-file`), each thread parsing headers into compact `Read`s (an author line's FNV-1a
    hash, time, offset, merge) and the contacts it hadn't met, handed to one `Tally` over a
    bounded channel: commits each day and month and each weekday's hour by their authors'
    clocks, merges, each contact's commits, first and last, and months active; then the
    contacts through the mailmap as people by email (`AuthorStats`, the most commits first),
    authors active each month, and the files at `HEAD` by language (`language_of`, from names)
    and the largest (`FileStats`). `read_lines`: the newest `LINES_COUNTED` (10,000) commits'
    lines (`commit_lines`) each month, by author, and by size (`LineStats`). `Stats::streak`,
    `busiest`; `day_of`, `month_of`, `date_of`.
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
    to consider) and removed (their watches went with them). For a repository on another
    computer, `Watcher::elsewhere`: a session there (`egit_git::look_there`,
    `Follow::at_once`), told at once by `inotifywait` where it is installed there, else looking
    every 2 s (`LOOKING_THERE`), what it says noted as changes are (`Changes::from_looked`;
    `is_live`); the session ending (the connection lost) is a watch failing, tried again with
    its backoff. `classify_repository` is `egit_git::classify`'s, so both sort paths alike.
    `TreeWatcher::elsewhere`: the folders followed by a session there
    (`look_at_folders_there`), started again whenever its owner names other folders, a folder
    made there noted as here; none where `inotifywait` isn't installed; one that failed or lost
    its connection tried again after `RETRY_THERE` (30 s), its words the owner's problem;
    `is_live` (here always; there once its watches are set up, read from the session's
    `Liveness` without the lock a job may hold).
  - `worktree.rs` — `WorkTree`: a repository's `Status`, read at once after egit's own writes
    and when the window regains focus, soon after the repository's watcher (index, `HEAD`,
    operations) or the work tree's; one read at a time, at most one a second while files keep
    changing, every 5 s while the work tree can't be watched. The read's job also sets the
    folders watched: the top, `HEAD`'s (`tree_folders`, cached by commit), those of the
    status's entries, and folders created since (with up to 1,024 inside) that `ignored_paths`
    doesn't name, so `mkdir -p` then a new file is seen, and `node_modules` costs no watch
    (those inside a new folder listed by `Files::folders_within`, on either computer). On
    another computer (`here` false) the watcher is made with the first read's git
    (`TreeWatcher::elsewhere`); while it isn't told (setting up, down, no `inotifywait` there:
    `is_live`), the status is read every 5 s. `RepoTab::following_there` (`FollowingThere`: at
    once, the files looked for, or all looked for) is said on the Overview's "Where it lives"
    card, under the computer.
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
    `InspiredGitHub`. `highlight_snippet`: a short text (an agent's code, 64 KiB at most)
    coloured whole in the language its fence word or extension names, in a job.
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
    stash; `PullStashing`, a pull with changes in its way: `--autostash`, or for files git
    doesn't track a stash first (untracked too), left in the list and undone with the pull;
    `Action::Task`), run by `run` through five groups (remotes, history, rewriting,
    references, stashes), under the `WriteLock`, its `Progress` shown and
    `CancelToken` stopped by `Action::CancelTask` (`Outcome::Cancelled`, not a failure).
    Besides git's results, an `Outcome` can be `Done`, `Integrated` (done, conflicts, empty),
    `Stashed` (whether anything was), `WouldOverwrite` (a switch refused, with the paths), or
    `NotMerged` (a delete refused). And a background fetch as often as the preferences say
    (`fetch_every`; every 5 minutes by default, or never; the first 10 s after opening; only
    with a remote): non-interactive, without the lock, stopped when the
    user starts a task (`Fetched::Stopped`: nothing learned, nothing said), its failure kept
    apart (`background_problem`, until one goes through, or a fetch of the user's does; another
    task reaching the remotes, `Task::reaches_remotes`, that goes through brings the next one
    forward; `background_fetches` counts them, for the forge); what it brought the current branch's upstream (`Brought`:
    `upstream_tip` before and after, `count_between` them; a later fetch continuing where an
    earlier one ended adds to it) kept in `brought` until the user starts a task, and shown
    under the commit table while the branch is behind (`views/tasks.rs`'s `brought`: "↓ 3 new
    commits on origin/main", fetched in the background; Pull, Show them (the upstream's
    commit selected), Dismiss). `remote_names`, `has_remotes` (a scan, nothing allocated),
    and `upstream_of` read the configuration. Fast-forwards (`FastForward` the current branch,
    `FastForwardBranch` another from a remote or a ref here: `Outcome::Forwarded`),
    `SetUpstream`, `DeleteBranchEverywhere` (here, then on the remote; refused here as not
    merged: `NotMerged`, nothing deleted), `AddWorktree`, `SavePatch` (`format_patch`, the
    file written in the job). Patches: `ApplyPatch` (its bytes shared, `Patch`), `Skip`,
    `CopyPatch` and `CopyChanges` (`Outcome::Copied`, its text, copied as the outcome is first
    shown; refused when empty or not UTF-8), `SavePatches` (`Outcome::SavedPatches`),
    `SaveChanges`. `Export`: commits picked together (`exporting.rs`; `Outcome::Exported`, how
    many patches or lines were made and the text copied, if it was; copied texts are shared,
    `Arc<str>`, as an outcome is cloned each frame it shows). A forge's tasks run with the
    tab's accounts:
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
    `SignCommits` signs commits picked together (`plan_signing`, then `sign_commits`, how many
    made in the `Progress`; `Outcome::Signed`: made, asked for, whether any was pushed),
    undone as any task moving `HEAD` is. `Discard` throws changes not staged away
    (`discarding.rs`; `Outcome::Discarded`, the files kept for Undo, or none). `Revert`
    works the revert out first (`revert_conflicts`: `foresee_pick` from `HEAD`) and, where
    it would conflict, changes nothing (`Outcome::WouldConflict`: the files, said in a problem
    box with "Revert and resolve the conflicts", which runs it `anyway`, stopping at them).
  - `foresight.rs` — `Foresight`, conflicts foreseen for the menus: a `Bringing` (a merge,
    a rebase, a pick, a revert) from the commit `HEAD` is at, worked out in a detached job
    the first time an item asks (`look`: `Working`, `Seen`, `Unknown`), at most four at once,
    the window woken as each lands; the latest 64 kept, by `HEAD` and what is brought, so
    `HEAD` moving asks again. Owned by the `HistoryView`; `Shared::menus` lends it (with git
    and `HEAD`) only where the repository isn't a partial clone.
  - `ignoring.rs` — `Ignore`: a path and `IgnoreWhat` (the file, every file of its
    `extension`, or a folder: `folder_of`) and where (`IgnoreIn`), its words ("every .log
    file"), and `run`: the pattern added (`Outcome::Ignored`), then `ignored_paths` asking git
    whether the path is ignored now (a later `!` pattern may bring it back: said so). Undone
    as `Change::Ignored` (`unignore`, `ignore_again`).
  - `discarding.rs` — `Discard`: whole files (tracked back to what the index holds, untracked
    deleted) or `DiscardLines` (a path, its diff shared, the lines picked, the index's entry,
    whether whole hunks), its words ("a hunk of src/main.rs", "the changes to 3 files"), and
    `run` (`discard_files` or `discard_lines`); `refusal` (a conflict, a submodule, an
    untracked repository of its own aren't discarded here) and `of_entries` (every entry that
    can be, from a list).
  - `exporting.rs` — `Export`: commits picked together as patches (one each, into a folder or
    copied), as one patch (into a file or copied), or a changelog copied (`changelog`: a
    Markdown list of their subjects, the oldest first, each with its short name linked to its
    page on the repository's forge; merges left out unless they are all there is); `run`, and
    each's words (`done`: "Saved 2 patches to ~/out.", merges left out said).
  - `output.rs` — a tab's output: `Run`s (a task or commit each: its title, `Transcript`, the
    hooks it was expected to run (`Task::hooks`, `commit_hooks`, among those installed), its
    start and end (`Ended`: done, stopped (refused or stopped part-way, `Outcome::stopped`:
    asked or shown elsewhere, so the panel doesn't open for it), failed, cancelled), its
    `CancelToken` while it runs), the last
    16 kept. `RepoTab::run_task` and `commit` begin one and give git its transcript;
    `follow` (each frame) opens the panel while the latest's hooks have run 0.4 s, or when it
    failed saying something (once a run: closed, it stays so), and closes it 1.5 s after a run
    it opened for went well (`Preferences::output`; the user's own opening never closes).
    `RepoTab::hooks`, read with the configuration and when the window comes back.
  - `undo.rs` — taking back what egit did. An `Undo`: what it was in words ("the merge of
    topic", "the commit “Fix it”") and its `Change`s in order: `HEAD` moved (from, to, and
    the `ResetMode` that moves it back and forth: soft for a commit, a soft or mixed reset's
    own, keep otherwise), a switch (`Place`: a branch, or a commit detached), a branch made,
    removed, renamed, or moved (a fast-forward of another), a tag made or removed, changes
    stashed (only a stash the action made) or a stash dropped, or changes not staged discarded
    (`Discarded`: each file as kept before and after, put back by `put_back`). Each change is
    checked before
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
    listed as `egit.githubHost` and `egit.gitlabHost`; an ssh alias as the repository's
    computer resolves it); an `Account` per host (the repository's credential configuration;
    for a repository on another computer this computer's git and the user's configuration
    here, `here`: this computer reaches the forge, and its prompts are relayed); the open requests read in a detached job, each
    repository up to `LIMIT` (1,000),
    each page into a shared `Found` as it lands (the window woken); a read again
    (`refresh`: after any task that exchanged with a remote, each background fetch, a
    sign-in, and Refresh) takes over only once complete, a failed one keeping what is shown
    and saying why (`ForgeProblem`: sign in, refused, rate limited until, other). Badges:
    head commits sorted with their pull request's number and checks, made again when the
    list grows or is replaced (`badges_at`, a binary search). `pull_request_for` (a branch's
    open one), `remote_of`, `repository_of`, `needs_sign_in` (the host whose requests want a
    sign-in, and whether it refused), `signed_in` and `signed_out` (a sign-in made or undone
    elsewhere taken by the host's account, and everything read again); `kind`, `kind_of`,
    `nouns`. Each forge's words:
    `request_noun` ("pull request", "merge request"), `request_number` (`#12`, `!12`), and
    `fork_head` (`refs/pull/12/head` into `pr/12`, `refs/merge-requests/12/head` into
    `mr/12`).
  - `desktop.rs` — `Opener`, and `is_web_page` (HTTPS, or HTTP to the loopback address;
    nothing that could pass for an option): `xdg-open`, else `gio open` (macOS's `open`;
    Windows' `rundll32 url.dll,FileProtocolHandler`, no shell reading the address), in a job.
    `Opener::reveal` (`Action::Reveal`): an absolute path opened in its app (`Reveal::Open`:
    the same openers; Explorer on Windows) or shown picked out in its folder (`InFolder`: the
    file manager's `org.freedesktop.FileManager1.ShowItems` through `gdbus`, else
    `dbus-send`, its `file://` address escaped byte by byte, else the folder opened; `open
    -R`; `explorer /select,"…"` as a raw argument), in a job; what is gone opens its nearest
    folder still there; recorded in tests (`revealed`). `in_work_tree`: a git path in the
    system's separators.
    `FileDialogs`: the desktop's file dialogs (`rfd`, one at a time) for a `FileQuestion`:
    where a patch or the changes go (a file), where a series goes (a folder), where commits
    picked go (`SavePicked`: a file for one patch, a folder for one each), which patches to
    apply (files, patches first), which images to attach to an agent's prompt
    (`AttachImages`); for tests a folder answering them all (`in_folder`). `Action::AskFiles`
    asks, and `AppState::poll` turns the answer into a task, shows the patches
    (`Action::OpenPatches`), or attaches the images to the tab's conversation shown.
  - `avatars.rs` — people's pictures (`Preferences::pictures`, on by default): `install` (the
    real application only: tests fetch nothing) starts two worker threads taking the newest
    request first (the rows on screen; past 128 waiting, the oldest dropped), each picture read
    from the cache folder (`cache_folder`: XDG's, macOS's `Library/Caches`, `LOCALAPPDATA`;
    `egit/pictures/2/<key>.png`, or `.none` for someone without one, believed a week; an
    earlier version's left in `pictures/` are removed) or fetched (`egit_forge::Pictures::find`:
    GitHub's, then Gravatar's), cropped square, scaled to 128 × 128 (Lanczos) and kept, then
    scaled to 64, 32, and 16 too (egui's renderer makes no mipmaps), each masked to a circle a
    pixel inside its edge, in one tile (192 × 128) of an atlas of 80 (2048 × 1024, one texture:
    `set_partial`, up to 8 a frame in `poll`), the one drawn least recently making room.
    `picture`: an email's atlas part, the smallest level at least its size on screen, asking
    for it otherwise (keyed by FNV-1a of the address in lower case: nothing allocated to look
    it up); a failed fetch is tried again after ten minutes.
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
    (Search, Reflog, Agents (the agents' panel at the tab's right), Stash, Patches, LFS where the repository keeps files in it (its state;
    pull, fetch, check out, push to each remote, prune; its page), Submodules where it has
    some (how they stand; mend everything, update all to the commits recorded or their
    branches' latest, sync addresses; their page), and Repository: its Overview, Settings,
    the submodules' and Git LFS's pages, opening its folder, and reading it again), their words beside their icons where the bar has room for all it shows
    (`Look::Full`), their icons alone elsewhere (`Look::Compact`, their words on hover), the
    commit count as a button, "Stats: 222 commits", opening the Stats page in the commit
    table's place (`Tool::Stats`; closed, it is kept, `stats_kept`, and shows again at once
    while the history is the same: `Names::identity`) (a spinner beside it while the history is
    read), why the work tree couldn't
    be read (when it couldn't), the requests (or, while they want a sign-in, "Sign in to
    GitLab", opening the dialog); at its right `views/tasks.rs`'s bar (drawn first,
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
    unchecked, its meaning on hover); in the commit's column, while any commit ran a GitLab
    pipeline, room on every row for one's mark (`pipeline::mark`, 24 points, the column drawn
    that much wider than kept: `column_widths`): the latest's status, by colour and icon alike
    (`ci_look`), its number and status on hover and as its name ("Pipeline #9003: running"), a
    click showing it (`Ask::Pipeline`), its menu opening it on GitLab; a focusable area takes ↑ ↓ PgUp PgDn Home End. The
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
    (`authors_cell`), and their names ("Ada and Alan", "Ada and 3 others"). Details (never so wide the commit table keeps under
    360 points, `CENTRAL_LEAST`, where the window has room): the subject as the panel's title, the description in a box
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
    the selection staying on its row (`keep_row`); a line under the message and facts
    (`split_line`), dragged to give them or the files below more room, kept with the settings
    as the message's share of the panel (`split_room`; a double click puts back the message's
    own height, up to half); and the changed files under a section
    title ("Changed files" with their count, the lines gained and lost at its right, and
    `widgets::tree_toggle`, a list or a tree; named "3 files changed") in a box
    (`widgets::list_box`, `ChangesTable`, its tree a `TreeTable`), with the lines each gained
    and lost once counted
    (`Counting`: a job after the list, the previous one stopped; not in a partial clone);
    clicking one opens a `FileView` (`views/file.rs`) in the commit table's place (Escape or
    "History" goes back; its bar: back, how the file changed and its path, at the right Open
    file and Show in folder (the work tree's file, as it is now), Blame, and File history
    (their icons alone where the bar is narrow, on a row of their own, wrapping, narrower
    still: panels open beside it);
    then Diff or File (or for Markdown Preview: before and after rendered side by side, the
    blocks that changed marked, under a bar stepping through them ("Change 2 of 5", Previous
    change and Next change, Alt+↑ Alt+↓; or "Nothing changed in how it reads"), or the side
    there is; `markdown.rs`), Unified or Side by side, as segmented choices):
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
    or unstage the file, the lines selected in its unified diff, or their hunks; for changes
    not staged, after a line, discard them the same ways (lines and hunks at once, the file
    after `Dialog::Discard` asks; disabled, saying why, while a task runs or for a
    submodule). A right click in that diff opens the same as a menu at the pointer (`lines_menu`:
    stage or discard lines or hunks, or unstage them on the staged side, and Copy), for the
    lines selected, or the line under the pointer when the click isn't on a selection
    (`TextGrid::select_line_at`, `line_at`, `selected_text`). A file in
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
    src", every file it holds), a file clicked opened in the `FileView`, a right click opening
    its row's menu (an `egui::Popup` at the pointer, over the list's files as they were then:
    stage or unstage it or the folder, "Discard changes…" or "Discard the folder's changes…",
    for what git doesn't track an Ignore submenu (`ignore_menu`: this file, every file of its
    extension, its folder, or this folder; under a header each for .gitignore and
    info/exclude; `RowMenu::untracked` worked out once as it opens), Show changes, Open file
    and Show in folder (a folder's: Open the folder; `WorkEvent::Reveal`), Copy path;
    "Discard all…"; each item's paths gathered only when chosen); then
    the commit form: summary
    (warned past 72 characters), description (⌘Enter commits from either, or from anywhere
    while no other field or dialog has the keys; the keys shown beside the commit button, not
    in its name), co-authors (chips with their
    avatars, each removable; "Co-author" opens a picker: a field typed into at once, the
    recent authors (`recent_authors` of the last 5,000 commits, read when it first opens; the
    user's own `user.email` left out) matching it by name or email, most commits first, eight
    at most, and whoever `Name <email>` typed names; each a `Co-authored-by` trailer), amend
    (a blank form takes the last commit's message, `read_message`, its co-author lines moved
    into the chips: git adds them back), sign off, sign (`commit.gpgSign` by default),
    and the last commit's output (its hooks') or problem (git's whole complaint, and "Show the
    output"); where the repository has `pre-commit` or `commit-msg`, which run first and "Skip
    hooks" (`--no-verify`, this commit only); while a commit runs, the hooks running and for how
    long, "Show the output", and Cancel (its token: SIGTERM to git's group, its hooks with it). It clears
    once the tab's `commits_made` grows. A history of 10,000+ commits
    without a commit-graph is offered one (`Action::WriteCommitGraph`).
    `views/tasks.rs` — the task bar at the top bar's right: Push, Pull, Fetch, and the branch
    beside its upstream with ↑ ahead ↓ behind (or "(not pushed yet)", "(gone)"); while a task
    runs, its progress and Cancel. Under the commit table (a bottom panel between the sidebar
    and the details, `history::notices_panel`, there only while it has something to say, so
    nothing above the table moves) the last task's outcome (a line, the remote's notes, or a
    problem box; one that went well goes by itself after `OUTCOME_LASTS`, 5 s of egui's clock,
    put forward while the pointer rests on it; a problem, or what stopped short, stays until
    dismissed; `set_outcome_lasts`: tests keep them all), a refused push explained by its
    reason (behind: "Force push…" or Pull; a lease that failed: nothing forced, fetch first),
    and what background fetches brought; under the top bar the banner
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
    confirmation (`--force-with-lease --force-if-includes`), changes in a pull's way ("Stash,
    pull, and put back", or for files git doesn't track "Stash and pull"; `asks_next` picks
    the dialog an outcome leads to), a branch's first push (remote and
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
    lock (why). Discarding whole files: what each becomes (back to what is staged, or deleted
    when untracked: "Delete todo.md?") and how long Undo can bring them back.
    `views/actions.rs` — the menus' pieces, which every menu is built from: `item` (an
    icon in a column of its own, then its words, which name it; disabled, why on hover),
    `danger_item` (what loses work, in the danger colour), `shortcut_item` (its keys at its
    right), `submenu` (an item with an arrow), `menu_header` (a group's name in small
    capitals over its items), and `menu_rows` (rows touching, 28 points each, their words
    8 points in). Long menus are grouped: a commit's (made here; into the current branch;
    rewriting it; bisect; reset; copy; patches) and a branch's (switch; into the current
    branch; its remote; renaming and deleting; copy; in the graph; its reflog). And what the
    commit table offers (`Menus`, from `Shared::menus`: what bringing commits in would do,
    `foreseen_item`'s hint at an item's right ("✓ clean", "⚠ 2 conflicts", "up to date",
    "fast-forward", "no changes", "checking…"; the files, and where a rebase would stop, on
    hover; the item's name its words alone; nothing worked out for an item disabled), on
    merging, rebasing onto (not keeping merges), cherry-picking, and reverting a commit or a
    ref, and in a drop's menu; the
    branch, `HEAD`, the configuration (its remotes read only when a menu needs them), the refs, whether a task runs or an operation is in progress,
    which disable what would change the branch, saying why): a commit's context menu (switch
    there detached, create a branch or tag there, merge it in (or always with a merge commit,
    "(no fast-forward)"; a branch's tip merged as the branch, a local one first, so git's message
    names it), rebase onto it, cherry-pick,
    revert, reset soft or mixed, hard after a dialog, a worktree there, its Bisect submenu (mark
    it bad or good in the bisection's terms, starting one; skip it), copy a link to it on
    its forge; and in the row's menu, its name, subject, or "Save as a patch…"), a ref badge's
    (switch to a branch, a remote's through its local one tracking it, unless one of that
    name exists; merge it in (or without fast-forward), rebase onto it (or onto it keeping
    merges), fast-forward the
    current branch to it, and a
    remote's local branch following it; a local branch's Pull (the current one as configured,
    another fast-forward only), Push (to its upstream, or the first-push dialog), and Set
    upstream; a worktree from it; rename or delete a branch, delete a remote's on its remote,
    or both at once; copy its name, or a link to it on its forge; pin it to the left or unpin
    it, show only it, it too, or hide it (`Scope`, `Action::Scope`), or every branch again;
    push or delete a tag), a branch badge double-clicked (switch), a ref dropped onto another
    (`drop_items`, in a menu where it lands: onto the current branch, merging the dragged one
    in (or without fast-forward), rebasing onto it, or fast-forwarding to it; the current branch onto another, rebasing
    onto that (keeping merges or not), merging it in (or without fast-forward), fast-forwarding; else fast-forwarding
    the branch dropped on, or switching to it, a merge into it said to wait for that), and
    the stash menu beside
    the commit count
    ("Stash the changes…", then a row per stash, its message and age, latest first, in a
    list that scrolls past ten (only the rows in view drawn), its submenu holding its whole
    message and Apply, Pop, Drop…). Each asks a task or a
    dialog (`Ask`), or opens the rebase editor, carried out by the history view
    (`carry_out`). The commit menu's "Rewrite" group (on a branch, nothing running): "Rebase
    interactively from here…", "Edit the message…" (the editor, that commit reworded),
    "Squash into its parent", "Drop this commit…" (a dialog first). Its "Bulk actions"
    submenu: what picking several offers (`views/picked.rs`), for this one ("Sign this
    commit…"), and how to pick more.
    `views/picked.rs` — commits picked together in the commit table (`Picked`: their rows and
    names, the newest first, and where a range runs from): ⇧-click a range from the last row
    clicked alone (at most 10,000), ⌘-click (Ctrl) adds or takes away one (the keys read from
    the click's own event, as a test's are let go in the same frame), ⇧↑ ⇧↓ stretch, Escape or
    a plain click lets them go; kept by name across a history read again (`follow`). While
    several are picked they alone show as selected, the details show them (`panel`: how many,
    Sign these commits…, Compare these two (exactly two), Export (a menu), Copy their names,
    Clear the selection, wrapped in the panel's width; the first 200 listed), and a right click
    on one opens their menu ("Bulk actions on 3 commits": the same). Signing asks first
    (`Dialog::SignCommits`: what is made again, what a push takes, what stays). Export
    (`export_items`, the menu's too): "As a single patch" and "As separate patches", each
    "Save to a file…" (or "…a folder…", asked by `FileQuestion::SavePicked`) and "Copy to the
    clipboard", and "Copy as a changelog" (`Task::Export`). Compared, two show in the details'
    place (`views/history.rs`'s `Comparison`: From and To, Swap them, Close the comparison; the
    files changed from one to the other, their lines counted, each opening its diff in a
    `FileView`), as long as they are the two picked.
    `views/output.rs` — the output panel at a repository tab's bottom (once anything ran): a
    bar (show or hide it, ⌘J; the run shown and how it stands: its hooks running and for how
    long, done, failed, cancelled; the runs kept; Cancel while one runs; Copy; Clear) over the
    lines (`show_collapsible`, dragged shut it closes), only those in view laid out, following
    new ones while at the bottom: a command after `$ `, the end in its verdict's colour, the
    rest in the colours its tools printed (the sixteen as the theme draws them, the 256's cube
    and exact colours as they are).
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
    address, open on its forge); pull or merge requests (on a forge; a click shows one; while
    they want a sign-in, a row in the accent offering it, `sign_in_row`); tags
    (folded at first); stashes (a click shows its commit in the details: `HistoryView::peek`,
    which the details show in place of the selection while the history doesn't have it; Apply,
    Pop, Drop); worktrees (here, locked, missing marked; a double click or "Open in a tab";
    Lock, Unlock, Remove; Open its folder); submodules (when there are any: path and recorded
    commit, or "not set up"; Update or Set up, Open in a tab, Open its folder). Each section's header is a band (its caret,
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
    `views/stats.rs` — `StatsView`, the Stats page in the commit table's place (Escape or
    History goes back): read when shown and again once the history is (what was read shows
    meanwhile; "Reading 1,200 of 5,000 commits…" at first), its lines after; tiles (commits and
    merges, authors, days with commits, first and latest commit, the longest streak, the busiest
    day, files at `HEAD`), as many to a row as fit; cards in pairs where there is room (860
    points), each pair as tall as each other (the taller's height kept, both reserving it before
    their content: egui's `set_min_height` measures from the cursor): commits each month and
    authors active each month (egui_plot: no zooming, the page scrolls; solid bars; years along
    the bottom; egui_plot's own hover off, as it draws its labels inside the chart, clipped:
    `Hover` lights the bar (or marks the month) under the pointer and says its figures in a
    tooltip at the pointer), the year of
    contributions to the latest commit (a calendar painted a square a day), when commits are
    made (a punch card, circles by weekday and hour), the authors with the most commits (their
    avatars, years, lines, a bar), languages at `HEAD` (a bar of shares and a legend), the lines
    changed each month (added above, removed below) and how large commits are, and the largest
    files. Each chart is named for those who can't see it.
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
    `views/clone.rs` — Home's clone card (`CloneView`): the address, the computer it goes onto
    ("Onto": this one, or another, as ssh reaches it, offered as the last a repository was
    opened on; not on Windows), the folder it goes into (here typed, `~` the home folder and a
    relative path inside it, or picked: `PickCloneFolder`; `Settings::clone_folder`, the last
    clone's, at first; there typed, read there once it is reached: `home::folder_there`), the
    folder's name (`name_from_url`, as git names it, until edited), submodules, partial;
    `Action::Clone` runs `clone` as a job (one at a time; there, with git there reached first,
    `app::reach`; `format::progress` and Cancel while it runs, `CancelClone`), then opens the
    repository where it was made (a `Location`); cancelled, it says git removed what it had
    made.
    `views/pipeline.rs` — `PipelineView`, a GitLab pipeline in the commit table's place (from
    its mark; Escape or History goes back): a bar (its number and status in words, Refresh,
    Open on GitLab, a spinner while read), what it ran for, its commit (Show the commit), who
    started it and how, how long it ran (or has so far); then its stages in order, each a header
    (status, name, how many jobs) over its jobs (status, name, "may fail", "manual", status in
    words, duration, the arrow of a page outside), drawn as far as they show (`show_rows`); a
    job's click opens its page in the browser, egit's last step. Read when shown and again
    every 5 s while it, or a job of it, may still change (`READ_AGAIN`), what was read shown
    meanwhile. `ci_look` (each status's icon and colour: the theme's `ok`, `danger`, `running`,
    `paused`, `muted`), `ci_words` ("passed with warnings", "waiting for someone to start a
    job"), `duration` ("1 min 35 s").
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
    (each with its page), latest reviews, description. Without credentials, a card saying why,
    its Sign in… opening the dialog. `check_look` gives a checks state's
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
    sidebar), diffs and files (side by side, wrapped, tab width), agents (the accounts
    besides Claude Code's own: each forgotten (an API key erased from the helper; a folder
    kept), one added: its name, "A Claude subscription" (a Claude Code folder of its own,
    `~/.claude-<name>` unless typed) or an API key (a password field, stored and found again in a job: no helper keeping it is a
    problem said so)), and every keyboard shortcut;
    each change pushes `Action::SetPreferences`, in effect at once (each tab's fetch interval
    and sidebar too) and kept.
  - `views/agents/` — the Agents panel at a repository tab's right (`views/repo.rs`'s
    `agents_panel`, drawn first: beside everything, the output under the rest; shown and
    hidden by the top bar's Agents, `Action::ToggleAgents`, so hidden it takes no room; its
    parts keep their own margins). `mod.rs`: the head on the window's head band, a hairline
    under it (the title; the conversations' menu, named by the one shown, its words cut to
    the room left (`fitted`): New conversation, those open (Close the conversation), Earlier
    ones with their age; a new conversation's +; Hide the agents); `ConversationView` (the
    draft and the images attached, read in a job (`attach`); the stack, what is open, the
    rows made, Markdown's and code's caches, an answer being made, the slash commands
    offered); images dropped on the window attached while it shows; the conversation shown
    started ahead of its first prompt (`RepoAgents::warm`). `setup.rs`: until Claude is set
    up, a card of steps, each done, under way, to do now (its button) or later, or failed:
    Node.js and npm (Look again, nodejs.org), the SDK and Claude Code (its version and size;
    Set up Claude, its time and Cancel, a failure's words), signing in. `transcript.rs`: the
    conversation on the reading background (a `Stack`, its margins inside what scrolls), its
    `Rows` made again only when the transcript, a fold, or whether Claude works changes: each
    top-level item, an open call's subagent items stepped in under it (a guide down each step)
    and its report last (`Row::Report`), and while a turn runs a working row (its seconds;
    starting; "Waiting for your answer, below"); prompts in a box (their images' names), Claude's
    words as Markdown, thinking a line of its first words opening onto all of it, each turn's
    end (time, tokens, estimated cost, a hairline after), notices (a warning's or failure's on
    a tint); before the first prompt, what Claude does here and tips. `tools.rs`: a tool's
    line (`line`: caret, icon, verb, detail cut as suits it, a path losing its start
    (`tables::elided_start`), a note (an edit's "+2 −1", a plan's "1 of 3 done", an agent's
    kind, an MCP tool's server) and how it stands (running, "Waiting for you", "Failed");
    clickable only where something opens; a file it read or changed shown from the arrow at
    its right, `Action::ShowWorkFile`), opening to what suits it (`details`: a command's
    description, the command when cut on its line, its output; an edit as a diff, several, or a
    file written whole; a read's contents; a fetch's answer as Markdown; a subagent's
    instructions; a question's words and answer; a plan as Markdown; other input as JSON),
    a failure's first line while folded, a plan's steps always (done struck through, the one
    under way in its own words). `code.rs`: boxes of code (a header saying what they hold, Copy
    saying it copied; the panel's tone, or the danger's for what failed; at most 200 lines,
    how many more said; selectable labels, so read and copied as text), an edit's diff (its
    lines from `egit_text::kept`, removed then added around those kept, `−`/`+` on their
    colours across the box), coloured by `syntax::highlight_snippet` in jobs (`Highlights`:
    plain until they land, the latest 256 kept), and Markdown split at its top-level code
    blocks (`parts`, pulldown-cmark's offsets; one still streaming runs to the end, coloured
    once it has). `waiting.rs`: the oldest request over the prompt (how many more): a tool's
    permission in its own terms ("Claude wants to edit src/parser.rs", the command, the diff,
    the file's content), why Claude Code asks, "Allow once", "Allow for this conversation" (its
    suggestions as rules), "Refuse…" (asking why: Enter or Refuse, Back); Claude's questions
    (choices described under their words, several where it takes several, one's own words;
    Answer, Don't answer); a plan ("Approve and accept edits", "Approve": the mode set first,
    then the answer; "Keep planning…" asking what to change). `composer.rs`: the images (chips:
    a thumbnail, name, size, scaled said; × takes one off; a problem box), the slash commands
    offered while the draft is a `/` and a word (those starting with it, then holding it, eight
    at most; ↑ ↓, Tab or Enter completes, Escape dismisses, a click), "Ask Claude" and what
    Enter does as fits, the prompt (Enter sends, Shift+Enter a new line; queued while a turn
    runs), attaching images (`FileQuestion::AttachImages`), the mode's menu (each mode's words
    on hover) and the model's, compact under 420 points, Send or Stop; who it runs as (the
    account's menu, chosen before it begins), who is signed in or an API key, what it has cost;
    nobody signed in, Claude Code's own login command for that folder, copied, and "Look
    again". Help: `help/agents.rs`.
  - `views/` — `home` (who commits and the profile's button first, then open by dialog or
    typed path, or on another computer over SSH (its computer and the folder there, `~` and a
    relative path its home folder's (`folder_there`); "Open there"; not on Windows, whose
    OpenSSH shares no connection and whose paths join with `\`), the problem and its remedy, recent repositories in egui-table-kit (another
    computer's said before their folder), and cloning), `profile` (identity and signing editable;
    GitHub and GitLab: its accounts (github.com, gitlab.com, and the hosts listed: who is
    signed in to each, asked once with what git's helpers keep, `SignIns::who`; Sign in… or
    Sign out), and the GitHub Enterprise and self-managed GitLab hosts (added and removed as
    `egit.githubHost` and `egit.gitlabHost`); credentials, pull and push, and every setting,
    each with its origin), `sign_in` (the sign-in dialog, the window's: for a host, what egit
    reads from it, then two steps: Create a token… (its page, the scope egit needs ticked:
    `scope_of`), or Copy the link beside it (for whichever browser one is signed in with;
    "Copied" for a moment, as wide either way: `copy_link`), and the token pasted (a password field typed into at once; Enter signs in);
    Sign in, Cancel, or "Sign in through git's credential helper instead" (Git Credential
    Manager's browser sign-in, or git's questions); asking the host, a refusal said as one to
    act on; signed in, as whom, a warning where no helper keeps it, and Done), `repo` (the history, or a page: the overview (with a Hooks card: where git looks for them
    (`core.hooksPath` said), those installed, where their output shows) or the settings table, under a bar
    going back to the history (or Escape) and switching between the two), `status_bar` (git's
    version and path, the
    theme, Commands and Preferences, a warning when
    questions can't be relayed), `prompts` (git's and ssh's questions: the app starts an
    `AskpassServer` and an `SshSharing` at launch and gives git their environment; questions queue and show one
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
  - `agents.rs` — agents (`egit-agents`): `AgentsSetup`, the window's (`AppState::agents`), and
    one for each other computer a repository is on (`AgentsSetup::there(git, home)`,
    `AppState::agents_there` by its name, made as its tab's panel first shows; its installation
    found with its Node, `installation`, `claude`, `elsewhere`):
    Node found and the SDK's readiness (`found`, a job), installing it (`installing`,
    cancellable), who is signed in to Claude Code's own folder (`signed_in`), `launch` (how to
    run the bridge as a `Signer`; the tests' `stand_in`); its cache under egit's
    (`cache_folder`); who is signed in, by configuration folder (`who`, asked once each,
    `ask_who_again`); adding an account (`add_account`, `AccountForm`). An `AgentAccount`
    (kept in the settings, `agent_accounts`): a name and its `AccountKind`, a `Folder` of its
    own or an `ApiKey` that git's helpers keep for `https://api.anthropic.com` with the name as
    user (never in the settings); its signer filled in the conversation's start job
    (`Starter`: the bridge as Claude Code's own runs it, git (here: its credential helpers
    keep the keys), and `Homes`, this computer's home folder and the other's: a subscription's
    folder under this one's is the same folder under the home folder there, `Homes::place`,
    where Claude Code there signs into it, as `who` asks and the login command, through
    `ssh -t`, offers; there, an API key goes in the start request (`Started::secrets`, put
    into the queued start as the bridge lands), a folder's path on the command line, and the
    other credentials are left out by `env -u`). `RepoAgents`, a tab's
    (`RepoTab::agents`): shown or not, its `Conversation`s and the one shown, the account new
    ones run as (`choose_account`: the one shown too, before it begins, its bridge started
    ahead stopped to start again as it; earlier ones listed and read from its folder), the
    earlier ones Claude Code keeps for the work tree (`past`, read when the menu opens),
    `begin`, `resume` (its messages read in a job into the transcript; `resume` set), `close`,
    `warm` (the one shown started ahead of its first prompt, once: its `Info` said unasked),
    `send` (a `Prompt` and its images). A `Conversation`: its transcript, permission requests
    waiting, `Info`, mode (as asked, or as Claude Code last said it changed) and model;
    `has_begun` (a prompt sent, or resumed: its account settled), `turn_time`; its bridge
    started in a job (`start`), requests queued until it says it is ready (`deliver`), its
    events taken in each frame (`poll`, its view's images too); ended, a later prompt starts it
    again going on with its session.
  - `attachments.rs` — images attached to a prompt: `read` (a job: each file 32 MiB at most,
    PNG, JPEG, GIF, or WebP by its bytes, decoded within bounds; one the API wouldn't take, over
    5 MiB as base64 or 8,000 pixels a side, scaled to 1,568 a side and sent as a PNG, else a
    JPEG; a thumbnail of 64 pixels a side), `Attachment` (its name, the `Image` sent, its size
    as sent, whether scaled, the thumbnail until its texture is made), `is_image` (dropped
    files, by extension), `MOST` (20 a prompt).
  - `stack.rs` — `Stack`: a tall list of items of different heights in a scroll area of its
    own, drawn only where in view (each `Entry`'s version and a guess), `margin` in from what
    scrolls (its sides, top, and bottom), measured when drawn or, out of view and never
    measured or changed since, a little each frame (2 ms) unseen; those above the view growing
    keep it where it is; at the end it follows what is added.
  - `markdown.rs` — Markdown previewed (`is_markdown`: `.md`, `.markdown`, …):
    `read_documents` reads a change's sides as `pictures.rs` does (1 MiB at most), each a
    `Document`: its text `prepared` (an image a link to it, as nothing is fetched; a task's box
    an icon, as `egui_commonmark` draws a checkbox nothing names) and split there into blocks
    (top-level elements, a top-level list's items one by one, an ordered one's numbered as it
    was; the room before each), with the link reference definitions written out and each
    heading's anchor as GitHub names it. `show` draws only the blocks in view (each through
    `egui_commonmark`, the definitions after it when it may use them; `ScrollArea::
    show_viewport`), a block's height guessed from its lines until drawn or measured ahead
    (unseen, 2 ms a frame), the view kept where it is as heights above it change; links' pages
    taken from egui's output for egit's opener (web pages only), a heading's link scrolling to
    it. Both sides read, `compare` matches their blocks by their own text (`egit_text::kept`)
    into `Change`s (removed, added: ranges of blocks, either may be empty), each changed
    block marked with its change and each side's `gaps` noted (where the other side has
    blocks it hasn't). A compared document's blocks are drawn 10 points in, a changed one's
    band behind it (the diff's removed or added colour; the change in view the words'
    stronger one; joined with the next of its change) with a bar at its left, a gap a line in
    the room above. `Previews` (both sides', and the change in view): `follow` goes to the
    first change as documents land, `go` scrolls each side to a change's first block (a line
    below the top) once its heights are there (`target`); a block measured above the view, or
    a new width (the block at the top kept, as far in), keeps the view where it was.
  - `pictures.rs` — an image's sides read (blobs through the shared `Objects`, or the work
    tree's file, 64 MiB at most: `read_side`, its limit given) and decoded by `image` (PNG, JPEG, GIF, WebP, BMP, ICO;
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
    colour, and pipelines' `running` (blue) and `paused` (orange); Phosphor icons; one scale of
    text: `SMALL` 11.5, `BODY` 13, `CODE` 12.5, `HEADING`
    14, `TITLE` 18, and `CONTROL` (28), the height of buttons, fields, and menus' buttons;
    every control has an edge: `field` behind what is typed, `border_strong` under the
    pointer), `widgets/` (the pieces every view is built from, so each looks and answers
    alike: titles (`page_title` 22, `panel_title` 16, `heading`), buttons by kind (`primary`,
    `danger` for what loses something, `secondary`, `small`, `link`; a toolbar's quiet `tool`,
    `tool_menu` with its caret (`tool_menu_of` where its words are someone's: a help topic explains
    it), and `icon_tool`, named in words, `Look::Full` or `Compact`;
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
    alike everywhere, or their picture once fetched (`avatars.rs`: a quad of its atlas on their
    colour, ringed alike) (`avatar`; `avatars` overlapping a little, three at most and "+2"); `toggles.rs`: a
    checkbox (a square box, filled and ticked when checked), radio buttons (`checkbox_of`,
    `radio_of`: labelled with someone's words, explained by a help topic), and a segmented
    control (sized from its words before it is drawn, a control's height, so it centres on its
    row as any widget does; its 24-point segments' corners following its), each a whole row
    to click; `TARGET`, the 24 points every control takes each way;
    `focus_ring`, drawn once a frame around the control with the keyboard focus after a key
    (Tab, an arrow) or an assistive technology moved it, gone at the next press of the
    pointer, as a browser's `:focus-visible`, never around a field (its edge shows it) or an
    area as large as the commit table (its selection shows it)), `format.rs` (`~` paths, "5 minutes ago",
    dates (in the viewer's time zone: `local_zone`, `local_date_time`, `zone_name`), a long
    command's progress), `settings.rs` (theme, recent repositories (each a path and, on another
    computer, its host), open tabs (`OpenTab`: a path here, or a host and a path; earlier
    settings' paths read as here),
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
    floating scroll bar aside: it lies over what it scrolls; and slivers under a point, text
    runs flowing one after another),
    nothing moving while idle, and every control named in words or numbers (link a field to
    its visible label with `labelled_by`), and every one-line field a control's height, its
    text centred top to bottom and 8 points from its side. `EGIT_GUI_RECTS=1` writes each
    rendered screen's elements (role, rectangle, name) to `<screen>.rects` beside it, to
    measure a screen to the point. `wait_for_label` lets what it found lay out two
    more frames (a window's first frame only sizes it: clicks there are lost); a wait that
    times out saves `timed-out-<test>.png`; `click_when_enabled` waits for a button and scrolls it into the window first
    (`into_view`: below a page's fold a click lands where it was last drawn); `menu_item`
    right-clicks and picks, and
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
    alone, holding still once let go, a double click putting them back), and the details'
    line between the message and the files alike;
    `src/tests/long_text.rs` edits a 200-line message in place and types one into the commit
    form, each scrolling inside its field, the buttons below still on screen.
    `src/tests/submodules.rs` meets a fresh clone's submodule not set up (the banner mends
    it), updates it to its branch's latest from its page (the commit it brings shown), records
    it, shows its change in the work tree; and adds one from the page.
    `src/tests/lfs.rs` meets Git LFS not set up (the banner sets it up), its menu and page,
    a file listed, a pattern tracked and untracked (skipped without git-lfs unless
    `EGIT_REQUIRE_LFS`).
    `src/tests/picked.rs` picks two commits by ⇧-click, adds and takes away a third by ⌘-click,
    signs the two from their menu (the one after them made again too, the first left alone;
    their signatures good in `git log`), and undoes it; and signs one commit from its menu's
    Bulk actions. `src/tests/avatars.rs` serves pictures from stand-ins for
    GitHub and Gravatar on the loopback address: one GitHub knows drawn from it (Gravatar not
    asked), one it answers its placeholder for drawn from Gravatar, one neither has kept to
    initials; all kept on disk, a window opened after asking nothing; and keeps to initials
    when the preferences say so.
    `src/tests/output.rs` commits through a pre-commit hook: the form says it runs first; its
    lines show as they come, the output closing after it went well; refused (the output kept
    open with its words); skipped; cancelled part-way from the output's bar (no commit); and
    the Overview's Hooks card lists it.
    `src/tests/foresight.rs` opens a side branch's commit's menu (merging, cherry-picking, and
    rebasing onto it foreseen to conflict in g, said on hover), the commit before's (clean),
    and the latest's (reverting it clean), nothing moved; and reverts a commit changed again
    since (refused, nothing changed; then anyway, stopped at the conflict, aborted), and the
    latest (git's message).
    `src/tests/stats.rs` opens the Stats page from the top bar on dated commits by two authors
    (one under two names, joined by the mailmap) and renders it, at its window's size and
    whole; closed and opened again, it shows at once.
    `src/tests/exporting.rs` picks two commits apart, copies them as separate patches and a
    changelog, saves them as one patch and as separate patches (a folder answering), then
    compares them (a file's diff opened, swapped, closed).
    `src/tests/discard.rs` discards a hunk from a diff (nothing asked) and undoes it, stages a
    line's hunk from a right click in the diff and unstages it from the staged one's, a file
    from its row's menu (asked first), an untracked file (deleted, then brought back by Undo),
    and everything not staged from a row's menu.
    `src/tests/ignoring.rs` ignores a file from its row's menu in a `.gitignore` made for it
    (Undo removes it), then every `.log` file in `info/exclude`; a tracked file's menu offers
    none. `src/tests/reveal.rs` opens a file and shows it in its folder from its row's menu,
    opens it from its view's bar, and the repository's folder from its menu (recorded).
    `src/tests/dragging.rs` drags branches: onto the current one (dismissed, then merged), the
    current one onto another (rebased), between the sidebar and the graph either way.
    `src/tests/markdown.rs` previews a README a commit changed (before and after: its headings,
    code, a table, an image made a link, tasks' boxes), a link going to the opener and a
    heading's link opening nothing; a document added and one deleted, each its one side; a
    changelog of 80 sections changed halfway down and further on, opened at the first change
    (in view on both sides, the top out of it), stepped to the next, and back by Alt+↑.
    `src/tests/remote.rs` opens a repository on "another computer" (this one, through the
    tests' sshd: `testing::ssh_host`, the sandbox's `options`, `AppState::ssh`) from Home's
    form (`~/repo`, the home folder there), reads its history and work tree there, shows a
    change's diff, stages and commits there, follows a commit made there behind its back, is
    told at once of a file changed there (inotify-tools installed; the Overview says so,
    rendered), and cancels a fetch whose remote hasn't answered; and runs the agents there (the
    stand-in bridge through the session): Claude Code's own account taking the API key the
    login there exports, a subscription's account (a folder under the home folder here) running
    from the same folder under the home folder there, that key left out; skipped without sshd
    unless `EGIT_REQUIRE_SSHD`. And clones onto it from Home (into `~/clones` there): a clone
    from a remote that hasn't answered cancelled (nothing left there), then one opened in a tab
    there.
    `src/tests/agents.rs` drives the Agents panel against `egit-agents`' stand-in SDK (skipped
    without Node unless `EGIT_REQUIRE_NODE`): shown from the top bar, a prompt sent with Enter
    and its answer streamed, a command allowed for the conversation (its line opened), a
    question answered, the mode changed, an earlier conversation gone on with; then two
    accounts added in Preferences (a folder, and an API key a sandbox's `store` helper keeps)
    and a conversation run as each (the stand-in says the key, or the folder, it was given);
    every state drawn in both themes (setup, empty, a long turn with every kind of line, an
    edit, a plan, a question, at work, the panel at its narrowest); a slash command completed
    by Tab, an image attached from the paperclip and sent (the stand-in says its type), and a
    file Claude read opened from its line (unchanged: in its app; changed: its diff).
    `src/tests/sign_in.rs` meets a fake GitLab never signed in to: offered in the banner, the
    top bar, and the sidebar (rendered); the dialog opening the token's page (or copying its
    link, the button keeping its place), refusing a
    mistyped token (the helper's file untouched), taking the right one (kept under its login),
    the merge requests read at once; Profile saying who (rendered) and signing out (forgotten,
    offered again); and Not now hiding the banner, the top bar's way staying.
    `src/tests/pipelines.rs` serves a fake GitLab's pipelines: each commit's latest marked (an
    older one of the same commit not), the mark following its pipeline as it finishes (read
    again at once: `read_pipelines_now`), its page (stages in order, jobs and their words, "may
    fail"; rendered), a job opening its page (recorded), Show the commit selecting it, and
    Escape closing the page.
    `src/tests/pictures.rs` shows an image's change: a commit's (both sides' sizes) and the
    work tree's.
    `src/tests/tree.rs` drives the folder tree: chosen in a list's header, folded and unfolded,
    a folder staged whole, a commit's files alike, a list again.
    `src/tests/notices.rs` lets background fetches run: one bringing nothing says nothing, two
    bringing a commit each make one notice of both; one failing (its remote out of reach), its
    warning gone once the user's own Fetch goes through, and none for one a task stopped.
    It also shows an outcome under the commit table (the table's head where it was), gone by
    itself five seconds later, and a refused revert staying.
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
