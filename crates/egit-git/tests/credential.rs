//! Credentials from git's helpers in a sandbox: a `store` helper's file filled from, nothing
//! asked when nobody may be asked, and a credential rejected (forgotten) and approved (kept)
//! again; through a repository's own helper and the user's.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{
    Asking, CancelToken, Credential, CredentialPlace, Repository, approve_credential,
    erase_credential, fill_credential, installed_helpers, reject_credential, store_credential,
};

const fn place(host: &str) -> CredentialPlace<'_> {
    CredentialPlace {
        protocol: "https",
        host,
        username: None,
    }
}

fn filled(sandbox: &Sandbox, repository: Option<&Repository>, host: &str) -> Option<Credential> {
    fill_credential(
        sandbox.git(),
        repository,
        place(host),
        Asking::Never,
        &CancelToken::new(),
    )
    .unwrap()
}

/// A `store` helper reading `file`, set in `dir`'s configuration (`--global` for the user's).
fn store_helper(sandbox: &Sandbox, dir: &Path, scope: &str, file: &Path) {
    let helper = format!("store --file={}", file.display());
    sandbox
        .run(dir, "config", &[scope, "credential.helper", &helper])
        .unwrap();
}

#[test]
fn a_repository_s_helper_fills_rejects_and_approves() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let file = sandbox.path("credentials");
    std::fs::write(&file, "https://ada:t0ken@example.com\n").unwrap();
    store_helper(&sandbox, &repo, "--local", &file);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();

    let credential = filled(&sandbox, Some(&repository), "example.com").unwrap();
    assert_eq!(credential.username(), "ada");
    assert_eq!(credential.password(), "t0ken");
    // Another host has none, and nothing may ask: no credential, no prompt.
    assert!(filled(&sandbox, Some(&repository), "elsewhere.example").is_none());
    // Outside the repository, its helper isn't configured.
    assert!(filled(&sandbox, None, "example.com").is_none());

    reject_credential(sandbox.git(), Some(&repository), &credential).unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("t0ken"));
    assert!(filled(&sandbox, Some(&repository), "example.com").is_none());

    approve_credential(sandbox.git(), Some(&repository), &credential).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("ada:t0ken@example.com")
    );
}

#[test]
fn the_user_s_helper_fills_from_anywhere() {
    let sandbox = Sandbox::new().unwrap();
    let file = sandbox.path("credentials");
    std::fs::write(&file, "https://bo:other@example.com:8443\n").unwrap();
    store_helper(&sandbox, sandbox.home(), "--global", &file);
    let credential = filled(&sandbox, None, "example.com:8443").unwrap();
    assert_eq!(credential.username(), "bo");
    assert_eq!(credential.password(), "other");
    assert!(filled(&sandbox, None, "example.com").is_none());
}

#[test]
fn a_secret_fed_to_git_never_shows_in_debug_output() {
    let sandbox = Sandbox::new().unwrap();
    let command = sandbox
        .git()
        .read("credential")
        .secret_stdin(zeroize::Zeroizing::new(b"password=hush\n".to_vec()));
    assert!(!format!("{command:?}").contains("hush"));
}

#[test]
fn a_secret_is_stored_for_a_user_and_erased() {
    let sandbox = Sandbox::new().unwrap();
    let file = sandbox.path("credentials");
    store_helper(&sandbox, sandbox.home(), "--global", &file);
    let key = |username| CredentialPlace {
        protocol: "https",
        host: "api.example.com",
        username: Some(username),
    };
    let kept = |username| {
        fill_credential(
            sandbox.git(),
            None,
            key(username),
            Asking::Never,
            &CancelToken::new(),
        )
        .unwrap()
    };
    store_credential(sandbox.git(), None, key("work"), "sk-work").unwrap();
    store_credential(sandbox.git(), None, key("home"), "sk-home").unwrap();
    // Each user's own.
    assert_eq!(kept("work").unwrap().password(), "sk-work");
    assert_eq!(kept("home").unwrap().password(), "sk-home");
    assert!(kept("other").is_none());

    erase_credential(sandbox.git(), None, key("work")).unwrap();
    assert!(kept("work").is_none());
    assert_eq!(kept("home").unwrap().password(), "sk-home");
    // Nothing that would inject attributes is stored.
    assert!(store_credential(sandbox.git(), None, key("work"), "a\nb").is_err());
}

#[test]
fn installed_helpers_are_found_in_git_s_exec_path() {
    let sandbox = Sandbox::new().unwrap();
    // `store` comes with every git; a name no helper has, nowhere.
    let found = installed_helpers(sandbox.git(), &["no-such-helper", "store"]).unwrap();
    assert_eq!(found, ["store"]);
}
