//! The text grid against its worst cases (the roadmap's corpus): a million lines of code, plain
//! and highlighted, a 10 MB line wrapped and not, CJK and emoji, and resizing while wrapped at
//! line 900,000. Each case reports frame times (laid out and tessellated) jumping across the
//! text and scrolling it. Then the whole application on a commit that moves 100,000 files:
//! how soon they are listed and counted, and frames scrolling through them beside a diff.
//!
//! Not run by default, as it is only meaningful in a release build:
//!
//! ```text
//! cargo test -p egit --release --lib -- --ignored --nocapture --test-threads=1 corpus
//! ```
//!
//! Where screens are rendered, each case is saved as `target/gui-screens/corpus-*.png`.

// A measuring tool: it prints what it measures, worked out from small known numbers.
#![allow(
    clippy::print_stdout,
    clippy::integer_division,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use egit_git::testing::Sandbox;
use egit_text::{Options, Text};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::scale::{peak_memory_mb, step_until};
use super::screens::{rendering, screens};
use crate::app::{Action, EgitApp};
use crate::fonts::{self, Fallbacks};
use crate::grid::{Plain, TextGrid};
use crate::settings::Settings;
use crate::views::prompts::Prompts;

/// The grid, what it shows, and the system fonts for what egui's lack.
struct Shown {
    grid: TextGrid,
    source: Plain,
    fonts: Fallbacks,
}

/// `bytes` on the grid, highlighted as the file `path` would be (if given).
fn shown_as(bytes: Vec<u8>, path: Option<&str>, wrap: bool) -> Harness<'static, Shown> {
    let mut grid = TextGrid::new("corpus");
    grid.set_wrap(wrap);
    crate::jobs::install_runtime().unwrap();
    let started = Instant::now();
    let text = Text::new(bytes).unwrap();
    let source = match path {
        Some(path) => Plain::named(text, Options::default(), path.as_bytes()),
        None => Plain::new(text, Options::default()),
    };
    println!("  measured in {:?}", started.elapsed());
    let builder = Harness::builder().with_size([1400.0, 900.0]);
    let builder = if rendering() { builder.wgpu() } else { builder };
    let harness = builder.build_ui_state(
        |ui, shown: &mut Shown| {
            shown.fonts.poll(ui.ctx());
            shown.grid.show(ui, &shown.source);
        },
        Shown {
            grid,
            source,
            fonts: Fallbacks::default(),
        },
    );
    harness
        .ctx
        .plugin_or_default::<egui_async::EguiAsyncPlugin>();
    harness
}

/// `bytes` on the grid, plain.
fn shown(bytes: Vec<u8>, wrap: bool) -> Harness<'static, Shown> {
    shown_as(bytes, None, wrap)
}

/// Steps `frames` frames, calling `before` ahead of each, and returns each frame's time: laid
/// out, then tessellated as the window would.
fn measure_frames<S>(
    harness: &mut Harness<'_, S>,
    frames: usize,
    mut before: impl FnMut(&mut Harness<'_, S>, usize),
) -> Vec<Duration> {
    let mut times = Vec::with_capacity(frames);
    for frame in 0..frames {
        before(harness, frame);
        let started = Instant::now();
        harness.step();
        let shapes = harness.output().shapes.clone();
        let pixels_per_point = harness.output().pixels_per_point;
        let _ = harness.ctx.tessellate(shapes, pixels_per_point);
        times.push(started.elapsed());
    }
    times
}

/// Prints the frame times' slowest, 99th percentile, and mean; returns the 99th percentile.
fn report(what: &str, mut frames: Vec<Duration>) -> Duration {
    frames.sort_unstable_by(|a, b| b.cmp(a));
    let p99 = frames.get(frames.len() / 100).copied().unwrap_or_default();
    let total: Duration = frames.iter().sum();
    println!(
        "  {what}: slowest {:?}, p99 {p99:?}, mean {:?} ({} frames)",
        frames.first().copied().unwrap_or_default(),
        total / u32::try_from(frames.len().max(1)).unwrap(),
        frames.len()
    );
    p99
}

/// Renders the screen as `corpus-name.png`, where screens are rendered.
fn snapshot<S>(harness: &mut Harness<'_, S>, name: &str) {
    if rendering() {
        harness.run_steps(2);
        let image = harness.render().unwrap();
        image
            .save(screens().join(format!("corpus-{name}.png")))
            .unwrap();
    }
}

/// A pseudo-random number below `bound`.
fn random(seed: &mut u64, bound: u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed % bound.max(1)
}

/// A million lines of C, every one different (so no row is laid out from the cache).
fn code(lines: usize) -> Vec<u8> {
    let mut text = String::new();
    for line in 0..lines {
        let _ = match line % 7 {
            0 => writeln!(text, "static int probe_{line}(struct device *dev)"),
            1 => writeln!(text, "{{"),
            2 => writeln!(
                text,
                "\tif (unlikely(!dev->of_node && {line} > CONFIG_LIMIT))\t/* {line} */"
            ),
            3 => writeln!(
                text,
                "\t\treturn dev_err_probe(dev, -ENODEV, \"no node {line}\\n\");"
            ),
            4 => writeln!(
                text,
                "\tregmap_update_bits(map, REG_{line:X}, BIT({}), 0);",
                line % 32
            ),
            5 => writeln!(text, "\treturn 0;"),
            _ => writeln!(text, "}}"),
        };
    }
    text.into_bytes()
}

/// Lines mixing CJK, emoji, combining marks, and ASCII.
fn cjk(lines: usize) -> Vec<u8> {
    let mut text = String::new();
    for line in 0..lines {
        let _ = writeln!(
            text,
            "{line}: 漢字のテスト、日本語と中文の混在 — 한국어 문장 🦀🎉✨ café naïve e\u{301} \
             tab\there 全角ＡＢＣ"
        );
    }
    text.into_bytes()
}

/// A million lines of C, highlighted: jumping while it is parsed in the background, the parse,
/// then jumping and scrolling once it is done. Returns the worst 99th percentile.
fn highlighted(seed: &mut u64) -> Duration {
    println!("a million lines of C, highlighted:");
    let mut harness = shown_as(code(1_000_000), Some("probe.c"), false);
    let started = Instant::now();
    let jumps = measure_frames(&mut harness, 400, |harness, _| {
        let line = u32::try_from(random(seed, 1_000_000)).unwrap();
        harness.state_mut().grid.scroll_to_line(line);
    });
    let mut worst = report("jumping while parsed", jumps);
    let parsed = |harness: &Harness<'_, Shown>| {
        harness
            .state()
            .source
            .syntax()
            .is_some_and(crate::syntax::Syntax::is_parsed)
    };
    while !parsed(&harness) && started.elapsed() < Duration::from_secs(600) {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("  parsed in {:?}", started.elapsed());
    let jumps = measure_frames(&mut harness, 400, |harness, _| {
        let line = u32::try_from(random(seed, 1_000_000)).unwrap();
        harness.state_mut().grid.scroll_to_line(line);
    });
    worst = worst.max(report("jumping once parsed", jumps));
    harness.state_mut().grid.scroll_to_line(500_000);
    harness.hover_at(egui::pos2(700.0, 450.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -240.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("wheel scrolling", scrolling));
    snapshot(&mut harness, "code-highlighted");
    worst
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus() {
    let budget = Duration::from_millis(8);
    let mut worst = Duration::ZERO;
    let mut seed = 0x5eed_u64;

    println!("a million lines of code:");
    let mut harness = shown(code(1_000_000), false);
    harness.run_steps(2);
    snapshot(&mut harness, "code");
    let jumps = measure_frames(&mut harness, 400, |harness, _| {
        let line = u32::try_from(random(&mut seed, 1_000_000)).unwrap();
        harness.state_mut().grid.scroll_to_line(line);
    });
    worst = worst.max(report("jumping", jumps));
    harness.state_mut().grid.scroll_to_line(500_000);
    harness.hover_at(egui::pos2(700.0, 450.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -240.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("wheel scrolling", scrolling));
    drop(harness);

    worst = worst.max(highlighted(&mut seed));

    println!("resizing while wrapped at line 900,000:");
    let mut long_lines = code(1_000_000);
    long_lines.extend(b"x".repeat(5_000));
    let mut harness = shown(long_lines, true);
    harness.state_mut().grid.scroll_to_line(900_000);
    harness.run_steps(2);
    let resizing = measure_frames(&mut harness, 300, |harness, frame| {
        let width = 700.0 + f32::from(u16::try_from(frame % 60).unwrap()) * 12.0;
        harness.set_size(egui::vec2(width, 900.0));
    });
    worst = worst.max(report("resizing", resizing));
    let top = harness.state().grid.top();
    assert_eq!(top.line, 900_000, "the top line stays put while resizing");
    snapshot(&mut harness, "resized");

    println!("a 10 MB line, wrapped:");
    let line: Vec<u8> = (0..10_000_000_u32)
        .map(|index| {
            b"abcdefghijklmnopqrstuvwxyz0123456789 ;{}()"[usize::try_from(index % 42).unwrap()]
        })
        .collect();
    let mut harness = shown(line.clone(), true);
    harness.hover_at(egui::pos2(700.0, 450.0));
    let wrapped = measure_frames(&mut harness, 400, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -4_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("wheel scrolling", wrapped));
    snapshot(&mut harness, "long-line-wrapped");

    println!("a 10 MB line, scrolled sideways:");
    let mut harness = shown(line, false);
    harness.hover_at(egui::pos2(700.0, 450.0));
    let sideways = measure_frames(&mut harness, 400, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-40_000.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("scrolling sideways", sideways));

    println!("CJK and emoji:");
    let mut harness = shown(cjk(200_000), false);
    harness.step();
    let before = fonts::generation(&harness.ctx);
    let started = Instant::now();
    while fonts::generation(&harness.ctx) == before && started.elapsed() < Duration::from_secs(20) {
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "  system fonts found in {:?} ({} added)",
        started.elapsed(),
        fonts::generation(&harness.ctx) - before
    );
    harness.run_steps(4);
    snapshot(&mut harness, "cjk");
    let jumps = measure_frames(&mut harness, 400, |harness, _| {
        let line = u32::try_from(random(&mut seed, 200_000)).unwrap();
        harness.state_mut().grid.scroll_to_line(line);
    });
    worst = worst.max(report("jumping", jumps));

    if !cfg!(debug_assertions) {
        assert!(
            worst < budget,
            "a case's 99th percentile frame took {worst:?}"
        );
    }
}

/// Files the mass rename moves.
const MOVED: usize = 100_000;

/// A repository whose last commit moves `MOVED` files from `src/old` to `src/new`, every
/// thousandth also edited (a rename less than exact), made by `git fast-import`.
fn mass_rename(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("moved").unwrap();
    let mut stream = Vec::new();
    let file = |index: usize| {
        format!("/* file {index} */\nint f{index}(void)\n{{\n\treturn {index};\n}}\n")
    };
    for index in 0..MOVED {
        let data = file(index);
        let _ = write!(
            stream,
            "blob\nmark :{}\ndata {}\n{data}\n",
            index + 1,
            data.len()
        );
    }
    let _ = write!(
        stream,
        "commit refs/heads/main\ncommitter T <t@egit.invalid> 1700000000 +0000\ndata 16\nLay out the tree\n"
    );
    for index in 0..MOVED {
        let _ = writeln!(
            stream,
            "M 100644 :{} src/old/d{:03}/f{index:06}.c",
            index + 1,
            index / 1000
        );
    }
    let _ = write!(
        stream,
        "\ncommit refs/heads/main\ncommitter T <t@egit.invalid> 1700000100 +0000\ndata 15\nMove everything\nR src/old src/new\n"
    );
    for index in (0..MOVED).step_by(1000) {
        let data = format!("{}/* moved */\n", file(index));
        let _ = write!(
            stream,
            "M 100644 inline src/new/d{:03}/f{index:06}.c\ndata {}\n{data}\n",
            index / 1000,
            data.len()
        );
    }
    stream.push(b'\n');
    sandbox
        .git()
        .write("fast-import")
        .arg("--quiet")
        .current_dir(&repo)
        .stdin(stream)
        .output()
        .unwrap();
    sandbox
        .run(&repo, "symbolic-ref", &["HEAD", "refs/heads/main"])
        .unwrap();
    repo
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_mass_rename() {
    println!("a commit moving {MOVED} files:");
    let sandbox = Sandbox::new().unwrap();
    let started = Instant::now();
    let repo = mass_rename(&sandbox);
    println!("  made in {:?}", started.elapsed());
    let git = sandbox.git().clone();
    let builder = Harness::builder().with_size([1400.0, 900.0]);
    let builder = if rendering() { builder.wgpu() } else { builder };
    let mut harness = builder.build_eframe(move |creation| {
        EgitApp::with_git(
            &creation.egui_ctx,
            git,
            Settings::default(),
            None,
            Prompts::none(),
        )
    });
    harness.run_steps(2);
    harness.state_mut().state.actions.push(Action::Open(repo));
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state
            .repos
            .first()
            .is_some_and(|repo| !repo.history.is_loading() && repo.history.shown.rows() == 2)
    });

    // Its subject is read once its row is drawn.
    let started = Instant::now();
    while harness
        .query_by_role_and_label(Role::Button, "Move everything")
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no subject");
        harness.step();
    }
    let started = Instant::now();
    harness
        .get_by_role_and_label(Role::Button, "Move everything")
        .click();
    step_until(&mut harness, Duration::from_secs(60), |app| {
        app.state.repos[0].history_view.details_shown()
    });
    println!("  listed in {:?}", started.elapsed());
    step_until(&mut harness, Duration::from_secs(60), |app| {
        app.state.repos[0].history_view.counts_shown()
    });
    println!("  counted in {:?}", started.elapsed());

    let started = Instant::now();
    harness
        .get_by_role_and_label(
            Role::Button,
            "src/old/d000/f000000.c → src/new/d000/f000000.c",
        )
        .click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state.repos[0].history_view.file_shown()
            == Some(("src/new/d000/f000000.c".to_owned(), true))
    });
    println!("  a file's diff shown in {:?}", started.elapsed());

    // Through the files beside the diff, a few hundred rows a frame.
    harness.hover_at(egui::pos2(1250.0, 600.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -3_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    let worst = report("scrolling the files", scrolling);
    snapshot(&mut harness, "mass-rename");
    println!("  peak memory {:.0} MB", peak_memory_mb());
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}
