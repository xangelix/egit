# AGENTS.md

## What this is

**egit** — a native git client in egui, built to stay fast on the largest repositories
(GitKraken's ground, without Electron). The system's `git` binary does every repository
operation; egit runs it, reads what it prints, and draws only what is on screen.
`docs/ROADMAP.md` is the plan (five MVP phases) and the decisions behind it.

## Status

| Phase | Scope | State |
|---|---|---|
| 1 | Foundation: shell, running git, askpass, profile, opening repositories | in progress |
| 2 | Commit graph | not started |
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
- Tests: `cargo test --workspace`
- Supply chain: `cargo deny check` (`deny.toml`) and `cargo shear` (unused dependencies).

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

## Repository layout

- `crates/egit-git` — the git binary: running it, and parsing what it prints. No egui.
- `crates/egit` — the application (`egit_app` library, `egit` executable).
- `docs/ROADMAP.md` — the MVP phases and their exit criteria.

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
- Changes to `egui-table-kit` are made in its own clone (`~/egui-table-kit`), each an
  independent commit appended to the previous ones, so they can be reviewed and released on
  their own.

## CI

GitHub Actions (`.github/workflows/ci.yml`). Every action is used at its latest major version,
and Dependabot keeps them there (`.github/dependabot.yml`).
