//! The real setup, opt-in (`cargo test -p egit-agents --test install -- --ignored`): the SDK
//! installed by `npm ci` from the lockfile into a temporary cache (about 250 MB from npm's
//! registry), Claude Code's binary there at the version the SDK pins, and the bridge loading
//! the real SDK to list the sessions of an empty configuration folder (local files only: no
//! account, no request to Anthropic).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use egit_agents::Launch;
use egit_agents::accounts::{AuthStatus, Signer, auth_status};
use egit_agents::install::{Install, find_node};
use egit_agents::sessions::list_sessions;
use egit_git::CancelToken;

#[test]
#[ignore = "downloads the SDK (about 250 MB) from npm's registry"]
fn the_sdk_installs_and_loads() {
    let node = find_node().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let install = Install::at(cache.path());
    install.install(&node, &CancelToken::new()).unwrap();
    assert!(install.is_ready());
    // Again: nothing to do.
    install.install(&node, &CancelToken::new()).unwrap();

    let claude = install.claude().unwrap();
    let version = std::process::Command::new(&claude)
        .arg("--version")
        .output()
        .unwrap();
    let version = String::from_utf8_lossy(&version.stdout);
    assert!(version.contains("(Claude Code)"), "{version}");

    // An empty folder of its own: nobody signed in, no sessions.
    let config = tempfile::tempdir().unwrap();
    let signer = Signer::Folder(config.path().to_owned());
    let status = auth_status(&claude, &signer, None).unwrap();
    assert_eq!(
        (status.logged_in, status.auth_method.as_str()),
        (false, "none"),
        "{status:?}"
    );
    assert_ne!(status, AuthStatus::default());
    let (env, unset) = signer.environment();
    let launch = Launch {
        node: node.node,
        bridge: install.bridge(),
        env,
        unset,
        there: None,
    };
    let work = tempfile::tempdir().unwrap();
    assert_eq!(list_sessions(&launch, work.path(), 10).unwrap(), []);
}
