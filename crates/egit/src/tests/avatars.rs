//! People's pictures, from a stand-in for Gravatar on the loopback address: an author with
//! one drawn from it, one without (404) kept to initials; each answer kept on disk, so a
//! window opened after asks nothing again.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use egit_forge::PictureSources;
use egit_git::testing::Sandbox;
use egui_kittest::Harness;

use super::screens::{harness, history_shown, open_path, profile_loaded, render, settle};
use crate::app::EgitApp;

/// A picture service on the loopback address: `url`'s path answered with a PNG, every other
/// with 404; counts the requests.
fn service(picture_url: impl Fn(&PictureSources) -> String) -> (PictureSources, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let sources = PictureSources {
        github_ids: format!("{base}u/"),
        github_logins: base.clone(),
        gravatar: format!("{base}avatar/"),
    };
    let wanted = picture_url(&sources).trim_start_matches(&base).to_owned();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(200, 160, |x, _| {
        if x < 100 {
            image::Rgba([220, 60, 40, 255])
        } else {
            image::Rgba([40, 90, 220, 255])
        }
    }))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            counted.fetch_add(1, Ordering::SeqCst);
            let line = String::from_utf8_lossy(&request);
            let found = line
                .split_whitespace()
                .nth(1)
                .is_some_and(|path| path.trim_start_matches('/') == wanted);
            let head = if found {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    png.len()
                )
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned()
            };
            let _ = stream.write_all(head.as_bytes());
            if found {
                let _ = stream.write_all(&png);
            }
        }
    });
    (sources, asked)
}

/// Steps `harness` until `email`'s picture is known to be `state`.
fn wait_for(harness: &mut Harness<'_, EgitApp>, email: &str, state: &str) {
    for _ in 0..2_000 {
        harness.step();
        if crate::avatars::known(&harness.ctx, email) == Some(state) {
            return;
        }
    }
    panic!("{email}'s picture never became {state}");
}

#[test]
fn pictures_are_fetched_kept_and_drawn() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "x\n", "Mine").unwrap();
    std::fs::write(repo.join("b"), "y\n").unwrap();
    sandbox.run(&repo, "add", &["b"]).unwrap();
    sandbox
        .run(
            &repo,
            "commit",
            &[
                "--quiet",
                "-m",
                "Ada's",
                "--author=Ada Lovelace <ada@example.com>",
            ],
        )
        .unwrap();
    let (sources, asked) = service(|sources| sources.url("tests@egit.invalid", 128));
    let cache = sandbox.home().join("pictures");

    let mut harness = harness(&sandbox);
    crate::avatars::install(&harness.ctx, &sources, Some(&cache));
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for(&mut harness, "tests@egit.invalid", "ready");
    wait_for(&mut harness, "ada@example.com", "none");
    render(&mut harness, "avatars");
    // One picture kept, one "none".
    let mut kept: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|entry| {
            let name = entry.unwrap().file_name();
            name.to_string_lossy()
                .rsplit('.')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    kept.sort();
    assert_eq!(kept, ["none", "png"]);
    let fetched = asked.load(Ordering::SeqCst);
    assert_eq!(fetched, 2);

    // Another window: both from the disk, nothing asked.
    let mut again = super::screens::harness(&sandbox);
    crate::avatars::install(&again.ctx, &sources, Some(&cache));
    settle(&mut again, "the profile", profile_loaded);
    open_path(&mut again, &repo);
    settle(&mut again, "the history", history_shown);
    wait_for(&mut again, "tests@egit.invalid", "ready");
    wait_for(&mut again, "ada@example.com", "none");
    assert_eq!(asked.load(Ordering::SeqCst), fetched);

    // Initials only, when the preferences say so.
    crate::avatars::set_enabled(&again.ctx, false);
    assert!(crate::avatars::picture(&again.ctx, "tests@egit.invalid", 18.0).is_none());
}
