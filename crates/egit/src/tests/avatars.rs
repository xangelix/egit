//! People's pictures, from stand-ins for GitHub and Gravatar on the loopback address: an author
//! GitHub knows by email drawn from GitHub (though Gravatar has one too), one it doesn't (its
//! placeholder) drawn from Gravatar, one neither knows kept to initials; each answer kept on
//! disk, so a window opened after asks nothing again.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;

use egit_forge::{PictureSource, PictureSources};
use egit_git::testing::Sandbox;
use egui_kittest::Harness;
use parking_lot::Mutex;

use super::screens::{harness, history_shown, open_path, profile_loaded, render, settle};
use crate::app::EgitApp;

/// A PNG `width` pixels square, of one colour on its left half and another on its right.
fn png(width: u32, left: [u8; 4], right: [u8; 4]) -> Vec<u8> {
    let half = width >> 1;
    let picture = image::RgbaImage::from_fn(width, width, |x, _| {
        image::Rgba(if x < half { left } else { right })
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(picture)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// The stand-ins: each path answered as `answers` says (a picture, or 404), GitHub's lookup by
/// email answering its placeholder (420 pixels, whatever is asked) for every address it doesn't
/// know; and every path asked, counted.
struct Service {
    sources: PictureSources,
    asked: Arc<Mutex<HashMap<String, usize>>>,
}

impl Service {
    fn start(answers: impl Fn(&PictureSources) -> Vec<(String, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let sources = PictureSources {
            github_ids: format!("{base}u/"),
            github_logins: base.clone(),
            github_emails: format!("{base}u/e?email="),
            gravatar: format!("{base}avatar/"),
        };
        let answers: HashMap<String, Vec<u8>> = answers(&sources)
            .into_iter()
            .map(|(url, bytes)| (url.trim_start_matches(&base).to_owned(), bytes))
            .collect();
        let placeholder = png(420, [200, 200, 200, 255], [200, 200, 200, 255]);
        let asked = Arc::new(Mutex::new(HashMap::new()));
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
                let line = String::from_utf8_lossy(&request).into_owned();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .to_owned();
                let mut counts = counted.lock();
                let count: &mut usize = counts.entry(path.clone()).or_insert(0);
                *count = (*count).saturating_add(1);
                drop(counts);
                let body = answers
                    .get(&path)
                    .or_else(|| path.starts_with("u/e?").then_some(&placeholder));
                let head = body.map_or_else(
                    || {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    },
                    |body| {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\
                             Connection: close\r\n\r\n",
                            body.len()
                        )
                    },
                );
                let _ = stream.write_all(head.as_bytes());
                if let Some(body) = body {
                    let _ = stream.write_all(body);
                }
            }
        });
        Self { sources, asked }
    }

    /// How many times the paths of `email`'s sources were asked, GitHub's then Gravatar's.
    fn asked(&self, email: &str) -> Vec<usize> {
        let asked = self.asked.lock();
        let base = self
            .sources
            .github_emails
            .split("u/e?")
            .next()
            .unwrap_or_default();
        self.sources
            .urls(email, crate::avatars::MASTER)
            .iter()
            .map(|source| {
                let path = source.url().trim_start_matches(base);
                asked.get(path).copied().unwrap_or(0)
            })
            .collect()
    }
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

/// A commit by `author` (`Name <email>`) of a new file `file`.
fn commit_by(sandbox: &Sandbox, repo: &std::path::Path, file: &str, author: &str) {
    std::fs::write(repo.join(file), file).unwrap();
    sandbox.run(repo, "add", &[file]).unwrap();
    sandbox
        .run(
            repo,
            "commit",
            &["--quiet", "-m", file, &format!("--author={author}")],
        )
        .unwrap();
}

#[test]
fn pictures_are_github_s_first_then_gravatar_s_kept_and_drawn() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "a", "x\n", "Mine").unwrap();
    commit_by(&sandbox, &repo, "b", "Ada Lovelace <ada@example.com>");
    commit_by(&sandbox, &repo, "c", "Grace Hopper <grace@example.com>");
    let green = png(128, [40, 170, 90, 255], [40, 170, 90, 255]);
    let gravatar = png(160, [220, 60, 40, 255], [40, 90, 220, 255]);
    let service = Service::start(|sources| {
        let url = |email: &str, which: usize| {
            sources.urls(email, crate::avatars::MASTER)[which]
                .url()
                .to_owned()
        };
        vec![
            // Mine: GitHub's (green), though Gravatar has one too.
            (url("tests@egit.invalid", 0), green.clone()),
            (url("tests@egit.invalid", 1), gravatar.clone()),
            // Ada's: Gravatar's alone; Grace has none.
            (url("ada@example.com", 1), gravatar.clone()),
        ]
    });
    assert!(matches!(
        service.sources.urls("ada@example.com", 128)[0],
        PictureSource::GitHubByEmail(_)
    ));
    let cache = sandbox.home().join("pictures").join("2");

    let mut harness = harness(&sandbox);
    crate::avatars::install(&harness.ctx, &service.sources, Some(&cache), None);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);
    wait_for(&mut harness, "tests@egit.invalid", "ready");
    wait_for(&mut harness, "ada@example.com", "ready");
    wait_for(&mut harness, "grace@example.com", "none");
    render(&mut harness, "avatars");
    // GitHub first: Gravatar asked only where GitHub had none.
    assert_eq!(service.asked("tests@egit.invalid"), [1, 0]);
    assert_eq!(service.asked("ada@example.com"), [1, 1]);
    assert_eq!(service.asked("grace@example.com"), [1, 1]);

    // Two pictures kept, one "none".
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
    assert_eq!(kept, ["none", "png", "png"]);

    // Another window: all from the disk, nothing asked.
    let total = |service: &Service| service.asked.lock().values().sum::<usize>();
    let before = total(&service);
    let mut again = super::screens::harness(&sandbox);
    crate::avatars::install(&again.ctx, &service.sources, Some(&cache), None);
    settle(&mut again, "the profile", profile_loaded);
    open_path(&mut again, &repo);
    settle(&mut again, "the history", history_shown);
    wait_for(&mut again, "tests@egit.invalid", "ready");
    wait_for(&mut again, "grace@example.com", "none");
    assert_eq!(total(&service), before);

    // Initials only, when the preferences say so.
    crate::avatars::set_enabled(&again.ctx, false);
    assert!(crate::avatars::picture(&again.ctx, "tests@egit.invalid", 18.0).is_none());
}
