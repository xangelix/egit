//! Commits egit makes, signed each way, then checked as `git log --show-signature` checks them:
//! SSH (a key file, and an agent holding the key), OpenPGP, X.509, and a signing program of the
//! user's own. And commits already made, signed after the fact.
//!
//! Each needs its tools (ssh-keygen and ssh-agent, gpg, gpgsm). A test whose tools aren't
//! installed is skipped, unless `EGIT_REQUIRE_SIGNERS` is set (as CI sets it).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers;
// a skipped test says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::print_stderr
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, Git, NewCommit, ObjectId, Repository, SignError, SignatureCheck, Verdict,
    Wait as _, check_signatures, create_commit, plan_signing, sign_commits,
};

/// Whether `program` can be run; a test without it is skipped, unless every signer is required.
fn available(program: &str) -> bool {
    let found = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();
    if !found {
        assert!(
            std::env::var_os("EGIT_REQUIRE_SIGNERS").is_none(),
            "{program} is needed to test signing"
        );
        eprintln!("skipped: {program} isn't installed");
    }
    found
}

/// A repository with a change staged, configured with `config` (keys and values).
fn staged(sandbox: &Sandbox, config: &[[&str; 2]]) -> PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    std::fs::write(repo.join("file"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["file"]).unwrap();
    for setting in config {
        sandbox.run(&repo, "config", setting).unwrap();
    }
    repo
}

/// Commits what is staged, signed, with `git`; returns the commit and its signature checked.
fn commit_signed(git: &Git, repo: &Path) -> SignatureCheck {
    let repository = Repository::discover(git, repo).wait().unwrap();
    let cancel = CancelToken::new();
    let commit = NewCommit {
        message: "Signed".to_owned(),
        sign: Some(true),
        ..NewCommit::default()
    };
    create_commit(git, &repository, &commit, &cancel)
        .wait()
        .unwrap();
    let head = git
        .read("rev-parse")
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
    let head = ObjectId::from_hex(head.stdout.trim_ascii()).unwrap();
    let parent = git
        .read("rev-parse")
        .arg("HEAD^")
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
    let parent = ObjectId::from_hex(parent.stdout.trim_ascii()).unwrap();
    let mut checks = check_signatures(git, &repository, &[head, parent], &cancel)
        .wait()
        .unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(
        checks[1].verdict,
        Verdict::Unsigned,
        "the first commit isn't signed"
    );
    checks.swap_remove(0)
}

/// What `git log --show-signature -1` says.
fn shown(git: &Git, repo: &Path) -> String {
    let out = git
        .read("log")
        .args(["--show-signature", "-1"])
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(out.stderr.as_bytes()),
        String::from_utf8_lossy(&out.stdout)
    )
}

/// Makes an SSH key without a passphrase at `path`; returns its public half.
fn ssh_key(path: &Path) -> String {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "tests", "-f"])
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
    std::fs::read_to_string(path.with_extension("pub"))
        .unwrap()
        .trim()
        .to_owned()
}

/// An allowed signers file trusting `public` for the tests' email.
fn allowed_signers(sandbox: &Sandbox, public: &str) -> PathBuf {
    let path = sandbox.home().join("allowed_signers");
    std::fs::write(
        &path,
        format!("tests@egit.invalid namespaces=\"git\" {public}\n"),
    )
    .unwrap();
    path
}

#[test]
fn signs_with_an_ssh_key_file() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let public = ssh_key(&key);
    let allowed = allowed_signers(&sandbox, &public);
    let repo = staged(
        &sandbox,
        &[
            ["gpg.format", "ssh"],
            ["user.signingKey", key.to_str().unwrap()],
            ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
        ],
    );
    let check = commit_signed(sandbox.git(), &repo);
    assert_eq!(check.verdict, Verdict::Good);
    assert_eq!(check.signer, "tests@egit.invalid");
    let shown = shown(sandbox.git(), &repo);
    assert!(
        shown.contains("Good \"git\" signature for tests@egit.invalid"),
        "{shown}"
    );
}

#[test]
fn a_revert_is_signed_as_commits_are() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let public = ssh_key(&key);
    let allowed = allowed_signers(&sandbox, &public);
    let repo = staged(
        &sandbox,
        &[
            ["gpg.format", "ssh"],
            ["user.signingKey", key.to_str().unwrap()],
            ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
            ["commit.gpgSign", "true"],
        ],
    );
    sandbox
        .run(&repo, "commit", &["--quiet", "-m", "Second"])
        .unwrap();
    let git = sandbox.git();
    let repository = Repository::discover(git, &repo).wait().unwrap();
    let cancel = CancelToken::new();
    let second = rev(git, &repo, "HEAD");
    assert_eq!(
        egit_git::revert(git, &repository, second, &cancel)
            .wait()
            .unwrap(),
        egit_git::Integrated::Done
    );
    let reverted = rev(git, &repo, "HEAD");
    let checks = check_signatures(git, &repository, &[reverted], &cancel)
        .wait()
        .unwrap();
    assert_eq!(checks[0].verdict, Verdict::Good);
    assert_eq!(
        said(git, &repo, reverted, "%s").trim_end(),
        "Revert \"Second\""
    );
    assert_eq!(std::fs::read_to_string(repo.join("file")).unwrap(), "one\n");
}

/// An ssh-agent of its own, stopped when dropped.
struct Agent {
    child: Child,
    socket: PathBuf,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn signs_with_a_key_an_ssh_agent_holds() {
    if !available("ssh-keygen") || !available("ssh-agent") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let public = ssh_key(&key);
    let allowed = allowed_signers(&sandbox, &public);
    let socket = sandbox.home().join("agent.sock");
    let agent = Agent {
        child: Command::new("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
        socket,
    };
    let started = Instant::now();
    while !agent.socket.exists() {
        assert!(started.elapsed() < Duration::from_secs(10), "no agent");
        std::thread::sleep(Duration::from_millis(10));
    }
    let added = Command::new("ssh-add")
        .arg("-q")
        .arg(&key)
        .env("SSH_AUTH_SOCK", &agent.socket)
        .status()
        .unwrap();
    assert!(added.success());
    // Only the agent has it now.
    std::fs::remove_file(&key).unwrap();
    let repo = staged(
        &sandbox,
        &[
            ["gpg.format", "ssh"],
            ["user.signingKey", &format!("key::{public}")],
            ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
        ],
    );
    let git = sandbox
        .git_with([("SSH_AUTH_SOCK", agent.socket.as_os_str())])
        .unwrap();
    let check = commit_signed(&git, &repo);
    assert_eq!(check.verdict, Verdict::Good);
    assert!(shown(&git, &repo).contains("Good \"git\" signature"));
}

#[test]
fn signs_with_a_program_of_the_users_own() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let key = sandbox.home().join("id_ed25519");
    let public = ssh_key(&key);
    let allowed = allowed_signers(&sandbox, &public);
    // A signer that notes it ran, then signs as ssh-keygen does.
    let log = sandbox.home().join("signer.log");
    let signer = sandbox.home().join("my-signer");
    std::fs::write(
        &signer,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec ssh-keygen \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    egit_git::testing::set_mode(&signer, 0o755).unwrap();
    let repo = staged(
        &sandbox,
        &[
            ["gpg.format", "ssh"],
            ["gpg.ssh.program", signer.to_str().unwrap()],
            ["user.signingKey", key.to_str().unwrap()],
            ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
        ],
    );
    let check = commit_signed(sandbox.git(), &repo);
    assert_eq!(check.verdict, Verdict::Good);
    let ran = std::fs::read_to_string(&log).unwrap();
    assert!(ran.contains("-Y sign"), "{ran}");
}

/// GnuPG's home in the sandbox (its default: the sandbox's `HOME`), its agent stopped when
/// dropped.
struct GnuPg(PathBuf);

impl GnuPg {
    fn new(sandbox: &Sandbox) -> Self {
        let home = sandbox.home().join(".gnupg");
        std::fs::create_dir(&home).unwrap();
        egit_git::testing::set_mode(&home, 0o700).unwrap();
        Self(home)
    }

    fn run(&self, program: &str, args: &[&str]) -> String {
        let out = Command::new(program)
            .env("GNUPGHOME", &self.0)
            .args(["--batch"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Makes an OpenPGP key of `algorithm` without a passphrase; returns its fingerprint and
    /// keygrip.
    fn key(&self, algorithm: &str) -> (String, String) {
        self.run(
            "gpg",
            &[
                "--passphrase",
                "",
                "--quick-generate-key",
                "egit tests <tests@egit.invalid>",
                algorithm,
                "sign",
                "never",
            ],
        );
        let listed = self.run(
            "gpg",
            &["--with-colons", "--with-keygrip", "--list-secret-keys"],
        );
        let field = |kind: &str| {
            listed
                .lines()
                .find(|line| line.starts_with(kind))
                .and_then(|line| line.split(':').nth(9))
                .unwrap()
                .to_owned()
        };
        (field("fpr:"), field("grp:"))
    }
}

impl Drop for GnuPg {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .env("GNUPGHOME", &self.0)
            .args(["--kill", "all"])
            .status();
    }
}

#[test]
fn signs_with_an_openpgp_key() {
    if !available("gpg") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let gnupg = GnuPg::new(&sandbox);
    let (fingerprint, _) = gnupg.key("ed25519");
    let repo = staged(&sandbox, &[["user.signingKey", &fingerprint]]);
    let check = commit_signed(sandbox.git(), &repo);
    assert_eq!(check.verdict, Verdict::Good);
    assert_eq!(check.signer, "egit tests <tests@egit.invalid>");
    let shown = shown(sandbox.git(), &repo);
    assert!(
        shown.contains("Good signature from \"egit tests <tests@egit.invalid>\""),
        "{shown}"
    );
}

#[test]
fn signs_with_an_x509_certificate() {
    if !available("gpg") || !available("gpgsm") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let gnupg = GnuPg::new(&sandbox);
    // gpgsm can't make a key without a passphrase; it certifies one gpg made.
    let (_, keygrip) = gnupg.key("rsa2048");
    let params = gnupg.0.join("certificate");
    std::fs::write(
        &params,
        format!(
            "Key-Type: RSA\nKey-Grip: {keygrip}\nKey-Usage: sign\nSerial: random\n\
             Name-DN: CN=egit tests\nName-Email: tests@egit.invalid\n"
        ),
    )
    .unwrap();
    let certificate = gnupg.run(
        "gpgsm",
        &["--armor", "--generate-key", params.to_str().unwrap()],
    );
    let pem = gnupg.0.join("certificate.pem");
    std::fs::write(&pem, certificate).unwrap();
    gnupg.run("gpgsm", &["--import", pem.to_str().unwrap()]);
    let listed = gnupg.run("gpgsm", &["--with-colons", "--list-keys"]);
    let fingerprint = listed
        .lines()
        .find(|line| line.starts_with("fpr:"))
        .and_then(|line| line.split(':').nth(9))
        .unwrap()
        .to_owned();
    // Self-signed, so trusted by hand, with no revocation list to fetch.
    std::fs::write(
        gnupg.0.join("trustlist.txt"),
        format!("{fingerprint} S relax\n"),
    )
    .unwrap();
    std::fs::write(gnupg.0.join("gpgsm.conf"), "disable-crl-checks\n").unwrap();

    let repo = staged(
        &sandbox,
        &[["gpg.format", "x509"], ["user.signingKey", &fingerprint]],
    );
    let check = commit_signed(sandbox.git(), &repo);
    assert_eq!(check.verdict, Verdict::Good);
    assert_eq!(check.key, fingerprint);
    let shown = shown(sandbox.git(), &repo);
    assert!(shown.contains("Good signature from"), "{shown}");
}

/// `name` resolved in `repo`.
fn rev(git: &Git, repo: &Path, name: &str) -> ObjectId {
    let out = git
        .read("rev-parse")
        .args(["--verify", name])
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// What `git log -1` says of `commit` in `format`.
fn said(git: &Git, repo: &Path, commit: ObjectId, format: &str) -> String {
    let out = git
        .read("log")
        .args(["-1", "--date=raw", &format!("--format={format}")])
        .arg(commit.to_string())
        .current_dir(repo)
        .output()
        .wait()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// A history to sign, its commits signed by an SSH key file: First; Ada's (by someone else,
/// in their own time zone) changing the line a side branch changes too; the side merged by
/// hand, its conflict resolved as neither side (a rebase would meet it again); then Last.
struct Merged {
    repo: PathBuf,
    first: ObjectId,
    adas: ObjectId,
    side: ObjectId,
    tip: ObjectId,
}

fn merged(sandbox: &Sandbox) -> Merged {
    let git = sandbox.git();
    let key = sandbox.home().join("id_ed25519");
    let allowed = allowed_signers(sandbox, &ssh_key(&key));
    let repo = sandbox.init("repo").unwrap();
    for [name, value] in [
        ["gpg.format", "ssh"],
        ["user.signingKey", key.to_str().unwrap()],
        ["gpg.ssh.allowedSignersFile", allowed.to_str().unwrap()],
    ] {
        sandbox.run(&repo, "config", &[name, value]).unwrap();
    }
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    sandbox.run(&repo, "branch", &["side"]).unwrap();
    std::fs::write(repo.join("file"), "main\n").unwrap();
    sandbox
        .run(
            &repo,
            "commit",
            &[
                "--quiet",
                "--all",
                "--message=Ada's",
                "--author=Ada Lovelace <ada@example.com>",
                "--date=@1700000000 +0900",
            ],
        )
        .unwrap();
    let adas = rev(git, &repo, "HEAD");
    sandbox.run(&repo, "switch", &["--quiet", "side"]).unwrap();
    sandbox
        .commit_file(&repo, "file", "side\n", "Side")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    let _conflict = sandbox.run(&repo, "merge", &["--quiet", "side"]);
    std::fs::write(repo.join("file"), "resolved\n").unwrap();
    sandbox
        .run(&repo, "commit", &["--quiet", "--all", "--no-edit"])
        .unwrap();
    sandbox.commit_file(&repo, "other", "x\n", "Last").unwrap();
    Merged {
        first: rev(git, &repo, "main~3"),
        side: rev(git, &repo, "side"),
        tip: rev(git, &repo, "main"),
        adas,
        repo,
    }
}

#[test]
fn signs_commits_already_made_keeping_trees_authors_and_merges() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let git = sandbox.git();
    let Merged {
        repo,
        first,
        adas,
        side,
        tip,
    } = merged(&sandbox);
    // Something not committed, which signing never touches.
    std::fs::write(repo.join("other"), "uncommitted\n").unwrap();
    let repository = Repository::discover(git, &repo).wait().unwrap();
    let cancel = CancelToken::new();

    // Ada's commit: it, the merge after it, and the last made again; the side's commit and
    // the first stay.
    let plan = plan_signing(git, &repository, &[adas], &cancel)
        .wait()
        .unwrap();
    assert_eq!((plan.asked, plan.commits.len(), plan.merges), (1, 3, 1));
    assert_eq!(plan.commits.first(), Some(&adas));
    assert!(!plan.pushed, "no upstream");
    let mut heard = Vec::new();
    let signed = sign_commits(git, &repository, &plan, &cancel, |made| heard.push(made))
        .wait()
        .unwrap();
    assert_eq!(heard, [1, 2, 3]);
    assert_eq!((signed.from, signed.made), (tip, 3));
    assert_eq!(rev(git, &repo, "main"), signed.to);

    // Each made again, signed, its tree as it was; Ada still its author, at her time.
    let after = [
        signed.to,
        rev(git, &repo, "main^"),
        rev(git, &repo, "main~2"),
    ];
    let checks = check_signatures(git, &repository, &after, &cancel)
        .wait()
        .unwrap();
    assert!(
        checks.iter().all(|check| check.verdict == Verdict::Good),
        "{checks:?}"
    );
    for (old, new) in [
        (tip, after[0]),
        (rev(git, &repo, &format!("{tip}^")), after[1]),
    ] {
        assert_eq!(
            rev(git, &repo, &format!("{old}^{{tree}}")),
            rev(git, &repo, &format!("{new}^{{tree}}"))
        );
    }
    assert_eq!(
        said(git, &repo, after[2], "%an <%ae> %ad%n%B"),
        said(git, &repo, adas, "%an <%ae> %ad%n%B")
    );
    assert_eq!(said(git, &repo, after[2], "%ad"), "1700000000 +0900\n");
    // The merge kept its resolution and its second parent; the first commit is the same.
    assert_eq!(
        said(git, &repo, after[1], "%P"),
        format!("{} {side}\n", after[2])
    );
    assert_eq!(rev(git, &repo, "main~3"), first);
    let shown_file = git
        .read("show")
        .arg("main^:file")
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(shown_file.stdout, b"resolved\n");
    // The uncommitted change is still there; the reflog says what moved the branch.
    assert_eq!(
        std::fs::read_to_string(repo.join("other")).unwrap(),
        "uncommitted\n"
    );
    let moved = git
        .read("log")
        .args(["-g", "-1", "--format=%gs", "main"])
        .current_dir(&repo)
        .output()
        .wait()
        .unwrap();
    assert_eq!(moved.stdout.trim_ascii(), b"egit: signed 3 commits");
}

#[test]
fn signing_says_what_was_pushed_and_refuses_what_it_cant_sign() {
    if !available("ssh-keygen") {
        return;
    }
    let sandbox = Sandbox::new().unwrap();
    let git = sandbox.git();
    let Merged { repo, adas, .. } = merged(&sandbox);
    sandbox
        .run(&repo, "branch", &["elsewhere", "side"])
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "elsewhere"])
        .unwrap();
    sandbox
        .commit_file(&repo, "elsewhere", "e\n", "Elsewhere")
        .unwrap();
    let elsewhere = rev(git, &repo, "elsewhere");
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    let repository = Repository::discover(git, &repo).wait().unwrap();
    let cancel = CancelToken::new();

    // Nothing asked for; a commit not in the branch.
    assert!(matches!(
        plan_signing(git, &repository, &[], &cancel).wait(),
        Err(SignError::Nothing)
    ));
    assert!(matches!(
        plan_signing(git, &repository, &[elsewhere], &cancel).wait(),
        Err(SignError::NotInBranch)
    ));

    // Pushed: the upstream holds the commit asked for, so pushing after takes a force push.
    let remote = sandbox.path("remote.git");
    sandbox
        .run(
            &repo,
            "init",
            &["--quiet", "--bare", remote.to_str().unwrap()],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "remote",
            &["add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "push",
            &["--quiet", "--set-upstream", "origin", "main"],
        )
        .unwrap();
    let plan = plan_signing(git, &repository, &[adas], &cancel)
        .wait()
        .unwrap();
    assert!(plan.pushed);

    // The branch moved after the plan was made: nothing changes.
    sandbox.commit_file(&repo, "late", "l\n", "Late").unwrap();
    let late = rev(git, &repo, "main");
    assert!(matches!(
        sign_commits(git, &repository, &plan, &cancel, |_| ()).wait(),
        Err(SignError::Moved)
    ));
    assert_eq!(rev(git, &repo, "main"), late);

    // Detached: no branch to move.
    sandbox
        .run(&repo, "switch", &["--quiet", "--detach", "main"])
        .unwrap();
    assert!(matches!(
        plan_signing(git, &repository, &[adas], &cancel).wait(),
        Err(SignError::Detached)
    ));
}
