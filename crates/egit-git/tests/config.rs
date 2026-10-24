//! Reading configuration as git resolves it, and changing it through git.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    ConfigEntry, ConfigPlace, ConfigScope, ConfigSnapshot, ConfigTarget, SigningFormat, Wait as _,
    add_config_value, set_config, unset_config, unset_config_value,
};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

#[test]
fn the_users_configuration_comes_from_their_global_file() {
    let sandbox = sandbox();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    let name = snapshot.profile().name().unwrap();
    assert_eq!(name.value, Some(BStr::new("egit tests")));
    assert_eq!(name.scope, ConfigScope::Global);
    assert_eq!(name.file(), Some(sandbox.home().join(".gitconfig")));
    assert!(
        snapshot
            .entries()
            .all(|entry| entry.scope == ConfigScope::Global),
        "only the sandbox's global configuration applies outside a repository"
    );
}

#[test]
fn a_repositorys_own_values_override_the_users() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    set_config(
        sandbox.git(),
        ConfigTarget::Repository(&repo),
        "user.email",
        "repo@egit.invalid",
    )
    .wait()
    .unwrap();

    let inside = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&repo))
        .wait()
        .unwrap();
    let email = inside.profile().email().unwrap();
    assert_eq!(email.value, Some(BStr::new("repo@egit.invalid")));
    assert_eq!(email.scope, ConfigScope::Local);
    assert_eq!(email.file(), Some(repo.join(".git/config")));

    let outside = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    assert_eq!(
        outside.profile().email().unwrap().value,
        Some(BStr::new("tests@egit.invalid"))
    );
}

#[test]
fn conditional_includes_apply_inside_matching_repositories() {
    let sandbox = sandbox();
    let work_config = sandbox.home().join("work.gitconfig");
    fs::write(&work_config, "[user]\n\temail = work@egit.invalid\n").unwrap();
    let condition = format!("includeIf.gitdir:{}/.path", sandbox.path("work").display());
    set_config(
        sandbox.git(),
        ConfigTarget::Global,
        &condition,
        &work_config,
    )
    .wait()
    .unwrap();
    let work = sandbox.init("work/project").unwrap();
    let personal = sandbox.init("personal/project").unwrap();

    let at_work = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&work))
        .wait()
        .unwrap();
    let email = at_work.profile().email().unwrap();
    assert_eq!(email.value, Some(BStr::new("work@egit.invalid")));
    assert_eq!(email.scope, ConfigScope::Global);
    assert_eq!(email.file(), Some(work_config));

    let at_home = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&personal))
        .wait()
        .unwrap();
    assert_eq!(
        at_home.profile().email().unwrap().value,
        Some(BStr::new("tests@egit.invalid"))
    );
}

#[test]
fn an_empty_credential_helper_clears_the_ones_before_it() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .run(
            &repo,
            "config",
            &["--global", "--add", "credential.helper", "store"],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "config",
            &[
                "--global",
                "--add",
                "credential.https://example.invalid.helper",
                "special",
            ],
        )
        .unwrap();
    let outside = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    let helpers: Vec<_> = outside
        .profile()
        .credential_helpers()
        .iter()
        .map(|entry| entry.text())
        .collect();
    assert_eq!(helpers, ["store"]);

    sandbox
        .run(&repo, "config", &["--add", "credential.helper", ""])
        .unwrap();
    sandbox
        .run(&repo, "config", &["--add", "credential.helper", "cache"])
        .unwrap();
    let inside = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&repo))
        .wait()
        .unwrap();
    let profile = inside.profile();
    let helpers: Vec<_> = profile
        .credential_helpers()
        .iter()
        .map(|entry| (entry.text(), entry.scope))
        .collect();
    assert_eq!(helpers, [(BStr::new("cache"), ConfigScope::Local)]);
    let for_urls: Vec<_> = profile
        .credential_helpers_for_urls()
        .map(|entry| entry.key)
        .collect();
    assert_eq!(for_urls, ["credential.https://example.invalid.helper"]);
}

#[test]
fn values_round_trip_exactly() {
    let sandbox = sandbox();
    let name = "Zoë \"Q\" O'Brien \\ ; # 名前";
    set_config(sandbox.git(), ConfigTarget::Global, "user.name", name)
        .wait()
        .unwrap();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    assert_eq!(
        snapshot.profile().name().unwrap().value,
        Some(BStr::new(name))
    );
}

#[test]
fn setting_replaces_every_value_and_unsetting_removes_them() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    for value in ["one", "two"] {
        sandbox
            .run(&repo, "config", &["--add", "egit.multi", value])
            .unwrap();
    }
    let target = ConfigTarget::Repository(&repo);
    set_config(sandbox.git(), target, "egit.multi", "three")
        .wait()
        .unwrap();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&repo))
        .wait()
        .unwrap();
    let values: Vec<_> = snapshot
        .get_all("egit.multi")
        .map(ConfigEntry::text)
        .collect();
    assert_eq!(values, ["three"]);

    unset_config(sandbox.git(), target, "egit.multi")
        .wait()
        .unwrap();
    unset_config(sandbox.git(), target, "egit.multi")
        .wait()
        .unwrap();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(&repo))
        .wait()
        .unwrap();
    assert_eq!(snapshot.get("egit.multi"), None);
}

#[test]
fn reads_signing_settings() {
    let sandbox = sandbox();
    let defaults = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    let signing = defaults.profile().signing();
    assert_eq!(signing.format.value, SigningFormat::OpenPgp);
    assert_eq!(signing.format.entry, None);
    // The sandbox turns signing off explicitly.
    assert!(!signing.commits.value);
    assert_eq!(signing.key, None);

    for (key, value) in [
        ("gpg.format", "ssh"),
        ("user.signingKey", "~/.ssh/id_ed25519.pub"),
        ("commit.gpgSign", "true"),
        ("gpg.ssh.program", "/opt/signer"),
        ("gpg.ssh.allowedSignersFile", "~/.ssh/allowed_signers"),
    ] {
        set_config(sandbox.git(), ConfigTarget::Global, key, value)
            .wait()
            .unwrap();
    }
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    let signing = snapshot.profile().signing();
    assert_eq!(signing.format.value, SigningFormat::Ssh);
    assert_eq!(signing.key.unwrap().text(), "~/.ssh/id_ed25519.pub");
    assert!(signing.commits.value);
    assert!(!signing.tags.value);
    assert_eq!(signing.program.unwrap().text(), "/opt/signer");
    assert_eq!(
        signing.allowed_signers.unwrap().text(),
        "~/.ssh/allowed_signers"
    );
}

#[test]
fn the_later_of_the_two_openpgp_program_keys_applies() {
    let sandbox = sandbox();
    set_config(
        sandbox.git(),
        ConfigTarget::Global,
        "gpg.openpgp.program",
        "/new/gpg",
    )
    .wait()
    .unwrap();
    set_config(
        sandbox.git(),
        ConfigTarget::Global,
        "gpg.program",
        "/old/gpg",
    )
    .wait()
    .unwrap();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    assert_eq!(
        snapshot.profile().signing().program.unwrap().text(),
        "/old/gpg"
    );
}

#[test]
fn bytes_that_arent_utf8_survive() {
    let sandbox = sandbox();
    let config = sandbox.home().join(".gitconfig");
    let mut contents = fs::read(&config).unwrap();
    contents.extend_from_slice(b"[egit]\n\traw = caf\xe9\n");
    fs::write(&config, contents).unwrap();
    let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
        .wait()
        .unwrap();
    assert_eq!(
        snapshot.get("egit.raw").unwrap().value,
        Some(BStr::new(b"caf\xe9"))
    );
}

#[test]
fn one_value_of_many_is_added_and_removed() {
    let sandbox = sandbox();
    let values = || -> Vec<String> {
        let snapshot = ConfigSnapshot::read(sandbox.git(), ConfigPlace::User)
            .wait()
            .unwrap();
        snapshot
            .get_all("egit.githubHost")
            .map(|entry| entry.text().to_string())
            .collect()
    };
    for host in [
        "ghe.example.com",
        "a.example",
        "ghe.example.com",
        "b.example",
    ] {
        add_config_value(sandbox.git(), ConfigTarget::Global, "egit.githubHost", host)
            .wait()
            .unwrap();
    }
    assert_eq!(values(), ["ghe.example.com", "a.example", "b.example"]);
    // A value that looks like a pattern is still only itself.
    unset_config_value(
        sandbox.git(),
        ConfigTarget::Global,
        "egit.githubHost",
        "a.exampl.",
    )
    .wait()
    .unwrap();
    unset_config_value(
        sandbox.git(),
        ConfigTarget::Global,
        "egit.githubHost",
        "a.example",
    )
    .wait()
    .unwrap();
    assert_eq!(values(), ["ghe.example.com", "b.example"]);
}
