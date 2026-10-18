//! An image's change shown side by side: a commit's, and one in the work tree not staged yet.

use std::io::Cursor;
use std::path::Path;

use egit_git::testing::Sandbox;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;

use super::screens::{
    harness, history_shown, open_path, profile_loaded, render, settle, uncommitted, wait_for_label,
};
use super::search::details_shown;

/// Writes a PNG of `width` by `height` pixels: a gradient, half transparent at the right.
// The relaxations `clippy.toml` gives `#[test]` functions, for this helper.
#[allow(clippy::arithmetic_side_effects, clippy::integer_division)]
fn png(path: &Path, width: u32, height: u32) {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let alpha = if x * 2 > width { 96 } else { 255 };
        image::Rgba([
            u8::try_from(x * 255 / width).unwrap(),
            u8::try_from(y * 255 / height).unwrap(),
            160,
            alpha,
        ])
    });
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn an_image_s_change_shows_both_sides() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    png(&repo.join("logo.png"), 120, 80);
    sandbox.run(&repo, "add", &["logo.png"]).unwrap();
    sandbox
        .run(&repo, "commit", &["-m", "Add the logo"])
        .unwrap();
    png(&repo.join("logo.png"), 200, 100);
    sandbox
        .run(&repo, "commit", &["-am", "A wider logo"])
        .unwrap();
    let mut harness = harness(&sandbox);
    settle(&mut harness, "the profile", profile_loaded);
    open_path(&mut harness, &repo);
    settle(&mut harness, "the history", history_shown);

    // A commit's: the blobs before and after.
    harness
        .get_by_role_and_label(Role::Button, "A wider logo")
        .click();
    settle(&mut harness, "the details", details_shown);
    harness
        .get_by_role_and_label(Role::Button, "logo.png")
        .click();
    wait_for_label(&mut harness, "After");
    for size in ["120 × 80 pixels", "200 × 100 pixels"] {
        assert_eq!(
            harness.query_all_by_label_contains(size).count(),
            1,
            "{size}"
        );
    }
    render(&mut harness, "image-diff");

    // The work tree's: the index's blob, and the file.
    png(&repo.join("logo.png"), 64, 64);
    harness.key_press(egui::Key::Escape);
    wait_for_label(&mut harness, &uncommitted(1));
    harness.get_by_label(&uncommitted(1)).click();
    wait_for_label(&mut harness, "Stage logo.png");
    harness
        .get_by_role_and_label(Role::Button, "logo.png")
        .click();
    wait_for_label(&mut harness, "Before");
    assert_eq!(
        harness
            .query_all_by_label_contains("64 × 64 pixels")
            .count(),
        1
    );
}
