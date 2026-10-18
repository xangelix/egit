//! The text grid against its worst cases (the roadmap's corpus): a million lines of code, plain
//! and highlighted, a 10 MB line wrapped and not, CJK and emoji, and resizing while wrapped at
//! line 900,000. Each case reports frame times (laid out and tessellated) jumping across the
//! text and scrolling it. Then the whole application on a commit that moves 100,000 files:
//! how soon they are listed and counted, and frames scrolling through them beside a diff. And
//! the merge editor on 100,000 lines of C with 1,000 conflicts: how soon git merges them and
//! the document is made (open, and again after a pick), and frames stepping through them. And
//! the interactive rebase editor on 10,000 commits: how soon they are read, and frames
//! scrolling and stepping through them. And the blame of 100,000 lines of C that 100 commits
//! rewrote here and there: how soon its first lines are labelled and all of them, and frames
//! jumping while git still blames and once it is done, and scrolling. And 1,000 open pull
//! requests from a fake GitHub (then merge requests from a fake GitLab), each the head of one
//! of 10,000 commits: how soon the first page shows and all of them, and frames scrolling and
//! stepping through the list, and the commit table under their badges. And the sidebar on
//! 100,000 refs (50,000 branches in 100 folders, 30,000 of a remote's, 20,000 tags): how soon
//! they are listed (and the longest frame meanwhile, as they are sorted), and frames scrolling
//! through them, unfolding the tags, and typing in the filter. And a Markdown document of
//! almost 1 MiB changed, previewed before and after: how soon it is read and drawn, and frames
//! still and wheel-scrolling through it.
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

use egit_forge::ForgeKind;
use egit_forge::testing::{FakeForge, FakePull, FakeRepository};
use egit_git::testing::Sandbox;
use egit_text::{Options, Text};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::scale::{peak_memory_mb, reset_peak_memory, step_until};
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
    let worst_tree = mass_rename_tree(&mut harness);
    println!("  peak memory {:.0} MB", peak_memory_mb());
    if !cfg!(debug_assertions) {
        assert!(
            worst.max(worst_tree) < Duration::from_millis(8),
            "the 99th percentile frame took {:?}",
            worst.max(worst_tree)
        );
    }
}

/// The mass rename's files as a tree: how soon it is arranged (in a job), its longest frame
/// meanwhile, then scrolling it, and a folder folded; returns the 99th percentile frame.
fn mass_rename_tree(harness: &mut Harness<'_, EgitApp>) -> Duration {
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    let started = Instant::now();
    harness
        .get_by_label("Show the files in a tree of their folders")
        .click();
    let mut longest = Duration::ZERO;
    let folder = format!("src/new/, {} files", crate::format::count(100_000));
    while harness.query_by_label(&folder).is_none() {
        assert!(started.elapsed() < Duration::from_secs(30), "no tree");
        let frame = Instant::now();
        harness.step();
        longest = longest.max(frame.elapsed());
    }
    println!(
        "  arranged in a tree {:?} after asking (its longest frame {longest:?})",
        started.elapsed()
    );
    // Folded and unfolded: every row worked out again, each in a frame.
    for done in ["folded away", "unfolded"] {
        harness.get_by_label(&folder).click();
        let frame = Instant::now();
        harness.step();
        println!(
            "  its 100,000 files {done} in a frame of {:?}",
            frame.elapsed()
        );
    }
    snapshot(harness, "mass-rename-tree");
    harness.hover_at(egui::pos2(1250.0, 600.0));
    let scrolling = measure_frames(harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -3_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    report("scrolling the tree", scrolling)
}

/// Lines of the merge editor's file, and how often both sides changed one.
const MERGED_LINES: usize = 100_000;
const CONFLICT_EVERY: usize = 100;

/// The merge editor's file: the base, and each side changing every hundredth line its way.
fn sides() -> [Vec<u8>; 3] {
    let base = code(MERGED_LINES);
    let changed = |side: &str| {
        let mut text = Vec::with_capacity(base.len() + MERGED_LINES);
        for (index, line) in base.split_inclusive(|&byte| byte == b'\n').enumerate() {
            text.extend_from_slice(line);
            if index % CONFLICT_EVERY == 50 {
                text.pop();
                text.extend_from_slice(format!(" /* {side} */\n").as_bytes());
            }
        }
        text
    };
    [changed("current"), base.clone(), changed("incoming")]
}

/// The merge editor's document on the grid, its selected conflict marked.
struct Merging {
    grid: TextGrid,
    document: std::sync::Arc<crate::merge::MergeDocument>,
    selected: usize,
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_merge() {
    use egit_git::{CancelToken, ConflictSides, Merge, ObjectId, Repository, TreeEntry};

    use crate::merge::{Choice, Labels, MergeDocument, Shown};

    let conflicts = MERGED_LINES / CONFLICT_EVERY;
    println!("{MERGED_LINES} lines of C with {conflicts} conflicts:");
    // Highlighting parses in the background (without a runtime, it would parse in place).
    crate::jobs::install_runtime().unwrap();
    let sandbox = Sandbox::new().unwrap();
    let path = sandbox.init("repo").unwrap();
    let repository = Repository::discover(sandbox.git(), &path).unwrap();
    let ids = sides().map(|text| {
        let hashed = sandbox
            .git()
            .write("hash-object")
            .args(["-w", "--stdin"])
            .current_dir(&path)
            .stdin(text)
            .output()
            .unwrap();
        ObjectId::from_hex(hashed.stdout.trim_ascii()).unwrap()
    });
    let entry = |id| {
        Some(TreeEntry {
            mode: 0o100_644,
            id,
        })
    };
    let sides = ConflictSides {
        current: entry(ids[0]),
        base: entry(ids[1]),
        incoming: entry(ids[2]),
    };
    let started = Instant::now();
    let merge = egit_git::merge_sides(sandbox.git(), &repository, &sides, &CancelToken::new());
    let Ok(Merge::Text(merged)) = merge else {
        panic!("not merged: {merge:?}");
    };
    println!("  merged by git in {:?}", started.elapsed());
    assert_eq!(merged.conflicts(), conflicts);

    let labels = Labels {
        current: "HEAD (main)".to_owned(),
        incoming: "side · 1234abcd A change".to_owned(),
    };
    let path_name = bstr::BStr::new("probe.c");
    let mut choices = vec![Choice::Open; conflicts];
    let started = Instant::now();
    let document = MergeDocument::new(&merged, path_name, &choices, false, &labels);
    println!("  shown open in {:?}", started.elapsed());
    choices[0] = Choice::CurrentThenIncoming;
    let started = Instant::now();
    drop(MergeDocument::new(
        &merged, path_name, &choices, false, &labels,
    ));
    println!("  made again after a pick in {:?}", started.elapsed());

    let builder = Harness::builder().with_size([1400.0, 900.0]);
    let builder = if rendering() { builder.wgpu() } else { builder };
    let mut harness = builder.build_ui_state(
        |ui, merging: &mut Merging| {
            let shown = Shown {
                document: &merging.document,
                selected: Some(merging.selected),
            };
            merging.grid.show(ui, &shown);
        },
        Merging {
            grid: TextGrid::new("corpus-merge"),
            document: std::sync::Arc::new(document),
            selected: 0,
        },
    );
    harness
        .ctx
        .plugin_or_default::<egui_async::EguiAsyncPlugin>();
    harness.run_steps(2);
    // From each conflict to the next, as Next or Alt+↓ steps.
    let stepping = measure_frames(&mut harness, conflicts, |harness, frame| {
        let merging = harness.state_mut();
        merging.selected = frame;
        let start = merging.document.start(frame).unwrap();
        merging.grid.scroll_to_line(start.saturating_sub(2));
    });
    let worst = report("stepping through the conflicts", stepping);
    snapshot(&mut harness, "merge");
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}

/// Commits in the interactive rebase editor's measurement.
const REBASED: usize = 10_000;

/// A repository whose `main` holds `REBASED` commits, each changing one file.
fn long_branch(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("long").unwrap();
    let mut stream = Vec::new();
    for index in 0..REBASED {
        let message = format!("Commit {index}\n");
        let data = format!("{index}\n");
        let _ = write!(
            stream,
            "commit refs/heads/main\ncommitter T <t@egit.invalid> {} +0000\ndata {}\n{message}\
             M 100644 inline counter\ndata {}\n{data}\n",
            1_700_000_000 + index,
            message.len(),
            data.len()
        );
    }
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
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();
    repo
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_rebase() {
    println!("the interactive rebase editor on {REBASED} commits:");
    let sandbox = Sandbox::new().unwrap();
    let repo = long_branch(&sandbox);
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
        app.state.repos.first().is_some_and(|repo| {
            !repo.history.is_loading()
                && repo.history.shown.rows() == u64::try_from(REBASED).unwrap()
        })
    });

    // To the root (End), then its menu.
    let newest = format!("Commit {}", REBASED - 1);
    let started = Instant::now();
    while harness
        .query_by_role_and_label(Role::Button, &newest)
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no subject");
        harness.step();
    }
    harness.get_by_role_and_label(Role::Button, &newest).click();
    harness.run_steps(2);
    harness.key_press(egui::Key::End);
    let started = Instant::now();
    while harness
        .query_by_role_and_label(Role::Button, "Commit 0")
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no root");
        harness.step();
    }
    harness.run_steps(2);
    harness
        .get_by_role_and_label(Role::Button, "Commit 0")
        .click_secondary();
    harness.run_steps(2);
    let started = Instant::now();
    harness
        .get_by_label("Rebase interactively from here…")
        .click();
    let read = format!("{REBASED} commits, from the root");
    while harness.query_by_label(&read).is_none() {
        assert!(started.elapsed() < Duration::from_secs(30), "not read");
        harness.step();
    }
    println!("  read and shown in {:?}", started.elapsed());

    // The newest selected (stepping starts there), then the plan scrolled.
    harness.get_by_role_and_label(Role::Button, &newest).click();
    harness.run_steps(2);
    harness.hover_at(egui::pos2(450.0, 400.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -600.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    let mut worst = report("wheel scrolling the plan", scrolling);
    let stepping = measure_frames(&mut harness, 600, |harness, _| {
        harness.key_press(egui::Key::ArrowDown);
    });
    worst = worst.max(report("stepping down the plan", stepping));
    snapshot(&mut harness, "rebase");
    println!("  peak memory {:.0} MB", peak_memory_mb());
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}

/// Lines of the blamed file, the commits writing it, and the lines each commit after the
/// first rewrites.
const BLAMED_LINES: u32 = 100_000;
const BLAME_COMMITS: usize = 100;
const REWRITTEN: usize = 500;

/// A repository whose `probe.c` holds `BLAMED_LINES` lines of C, written by its first commit
/// and rewritten by each of the next `REWRITTEN` lines at a time, scattered: a blame of tens
/// of thousands of short runs.
fn rewritten(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("blamed").unwrap();
    let code = String::from_utf8(code(usize::try_from(BLAMED_LINES).unwrap())).unwrap();
    let mut lines: Vec<String> = code.lines().map(str::to_owned).collect();
    let mut seed = 0xb1a3_u64;
    let mut stream = Vec::new();
    for index in 0..BLAME_COMMITS {
        if index > 0 {
            for _ in 0..REWRITTEN {
                let at = usize::try_from(random(&mut seed, u64::from(BLAMED_LINES))).unwrap();
                lines[at] = format!("\tvalue_{at} = rewrite({index}); /* {index} */");
            }
        }
        let message = format!("Rewrite {index}\n");
        let data = lines.iter().fold(String::new(), |mut data, line| {
            data.push_str(line);
            data.push('\n');
            data
        });
        let _ = write!(
            stream,
            "commit refs/heads/main\ncommitter T <t@egit.invalid> {} +0000\ndata {}\n{message}\
             M 100644 inline probe.c\ndata {}\n{data}\n",
            1_700_000_000 + index * 86_400,
            message.len(),
            data.len()
        );
    }
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
    sandbox.run(&repo, "reset", &["--quiet", "--hard"]).unwrap();
    repo
}

/// The application showing the newest commit's `probe.c` in `repo`.
fn newest_file(sandbox: &Sandbox, repo: std::path::PathBuf) -> Harness<'static, EgitApp> {
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
        app.state.repos.first().is_some_and(|repo| {
            !repo.history.is_loading()
                && repo.history.shown.rows() == u64::try_from(BLAME_COMMITS).unwrap()
        })
    });

    // The newest commit's file.
    let newest = format!("Rewrite {}", BLAME_COMMITS - 1);
    let started = Instant::now();
    while harness
        .query_by_role_and_label(Role::Button, &newest)
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no subject");
        harness.step();
    }
    harness.get_by_role_and_label(Role::Button, &newest).click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        let view = &mut app.state.repos[0].history_view;
        view.details_shown() && view.counts_shown()
    });
    harness
        .get_by_role_and_label(Role::Button, "probe.c")
        .click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state.repos[0].history_view.file_shown() == Some(("probe.c".to_owned(), true))
    });
    harness
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_blame() {
    println!("the blame of {BLAMED_LINES} lines rewritten by {BLAME_COMMITS} commits:");
    let sandbox = Sandbox::new().unwrap();
    let started = Instant::now();
    let repo = rewritten(&sandbox);
    println!("  made in {:?}", started.elapsed());
    reset_peak_memory();
    let mut harness = newest_file(&sandbox, repo);
    let started = Instant::now();
    harness.get_by_label("Blame").click();
    let progress = |harness: &mut Harness<'_, EgitApp>| {
        harness.state_mut().state.repos[0]
            .history_view
            .blame_progress()
            .unwrap_or((0, false))
    };
    while progress(&mut harness).0 == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "nothing blamed"
        );
        harness.step();
    }
    println!(
        "  first {} lines blamed in {:?}",
        progress(&mut harness).0,
        started.elapsed()
    );

    // Jumping while git blames (as long as it does, up to 400 frames).
    let mut seed = 0x5eed_u64;
    let mut jumps = Vec::new();
    while !progress(&mut harness).1 && jumps.len() < 400 {
        jumps.extend(measure_frames(&mut harness, 1, |harness, _| {
            let line = u32::try_from(random(&mut seed, u64::from(BLAMED_LINES))).unwrap();
            harness.state_mut().state.repos[0]
                .history_view
                .blame_scroll_to(line);
        }));
    }
    let mut worst = if jumps.is_empty() {
        Duration::ZERO
    } else {
        report("jumping while blamed", jumps)
    };
    while !progress(&mut harness).1 {
        assert!(started.elapsed() < Duration::from_secs(600), "never blamed");
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "  all {} lines blamed in {:?}",
        progress(&mut harness).0,
        started.elapsed()
    );

    let jumps = measure_frames(&mut harness, 400, |harness, _| {
        let line = u32::try_from(random(&mut seed, u64::from(BLAMED_LINES))).unwrap();
        harness.state_mut().state.repos[0]
            .history_view
            .blame_scroll_to(line);
    });
    worst = worst.max(report("jumping once blamed", jumps));
    harness.state_mut().state.repos[0]
        .history_view
        .blame_scroll_to(50_000);
    harness.hover_at(egui::pos2(450.0, 500.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -240.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("wheel scrolling", scrolling));
    snapshot(&mut harness, "blame");
    println!(
        "  peak memory {:.0} MB (the repository made beforehand not counted)",
        peak_memory_mb()
    );
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}

/// Open pull requests in their measurement.
const PULLS: u32 = 1_000;

/// [`long_branch`] on a fake forge: `origin` is `octo/tool` there (GitLab's in a subgroup),
/// signed in through a `store` helper, with [`PULLS`] open requests, each the head of one of
/// the newest commits.
fn on_forge(sandbox: &Sandbox, fake: &FakeForge) -> std::path::PathBuf {
    let repo = long_branch(sandbox);
    let (path, key, statuses) = match fake.kind() {
        ForgeKind::GitHub => (
            "octo/tool",
            "egit.githubHost",
            ["SUCCESS", "FAILURE", "PENDING"],
        ),
        ForgeKind::GitLab => (
            "octo/sub/tool",
            "egit.gitlabHost",
            ["SUCCESS", "FAILED", "RUNNING"],
        ),
    };
    let url = format!("http://{}/{path}.git", fake.address());
    sandbox
        .run(&repo, "remote", &["add", "origin", &url])
        .unwrap();
    sandbox
        .run(&repo, "config", &[key, &fake.host_entry()])
        .unwrap();
    let store = sandbox.path("credentials");
    std::fs::write(&store, format!("http://ada:t0ken@{}\n", fake.address())).unwrap();
    // Through git's shell, which takes `/` between folders everywhere (and a backslash as an
    // escape).
    let helper = format!(
        "store --file={}",
        store.display().to_string().replace('\\', "/")
    );
    sandbox
        .run(&repo, "config", &["credential.helper", &helper])
        .unwrap();
    let heads = sandbox
        .run(
            &repo,
            "rev-list",
            &[&format!("--max-count={PULLS}"), "main"],
        )
        .unwrap();
    let pulls = String::from_utf8(heads.stdout)
        .unwrap()
        .lines()
        .zip((1..=PULLS).rev())
        .map(|(head, number)| FakePull {
            number,
            title: format!("Change number {number}, which does something worth reviewing"),
            author: format!("author{}", number % 37),
            head_branch: format!("topic-{number}"),
            base_branch: "main".to_owned(),
            head_commit: head.to_owned(),
            review: Some(
                ["APPROVED", "REVIEW_REQUIRED", "CHANGES_REQUESTED"]
                    [usize::try_from(number % 3).unwrap()]
                .to_owned(),
            ),
            checks: Some(statuses[usize::try_from(number % 3).unwrap()].to_owned()),
            ..FakePull::default()
        })
        .collect();
    fake.state().repositories.push(FakeRepository {
        path: path.to_owned(),
        id: 1,
        default_branch: "main".to_owned(),
        parent: None,
        pulls,
    });
    repo
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_pull_requests() {
    for kind in [ForgeKind::GitHub, ForgeKind::GitLab] {
        requests_on(kind);
    }
}

/// [`PULLS`] open requests on `kind`'s fake, measured.
fn requests_on(kind: ForgeKind) {
    let nouns = match kind {
        ForgeKind::GitHub => "pull requests",
        ForgeKind::GitLab => "merge requests",
    };
    println!(
        "{PULLS} open {nouns} ({}) on a history of {REBASED} commits:",
        kind.name()
    );
    let sandbox = Sandbox::new().unwrap();
    let fake = FakeForge::start(kind, "t0ken", "ada").unwrap();
    let repo = on_forge(&sandbox, &fake);
    reset_peak_memory();
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
    let started = Instant::now();
    harness.state_mut().state.actions.push(Action::Open(repo));
    let read = |harness: &mut Harness<'_, EgitApp>| {
        harness
            .state()
            .state
            .repos
            .first()
            .map_or(0, |repo| repo.forge.found().pulls.len())
    };
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state
            .repos
            .first()
            .is_some_and(|repo| !repo.forge.found().pulls.is_empty())
    });
    println!(
        "  the first {} shown {:?} after opening",
        read(&mut harness),
        started.elapsed()
    );
    step_until(&mut harness, Duration::from_secs(60), |app| {
        app.state.repos.first().is_some_and(|repo| {
            !repo.forge.is_reading()
                && repo.forge.found().pulls.len() == usize::try_from(PULLS).unwrap()
                && !repo.history.is_loading()
        })
    });
    println!(
        "  all {} read {:?} after opening",
        read(&mut harness),
        started.elapsed()
    );

    // The commit table, every row of its first tenth badged: scrolled through those.
    harness.hover_at(egui::pos2(450.0, 500.0));
    let badged = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -30.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    let mut worst = report("scrolling the badged commits", badged);
    let screen = kind.name().to_ascii_lowercase();
    snapshot(&mut harness, &format!("requests-badges-{screen}"));

    harness
        .get_by_label_contains(&format!("1,000 {nouns}"))
        .click();
    harness.run_steps(4);
    harness.hover_at(egui::pos2(450.0, 250.0));
    let scrolling = measure_frames(&mut harness, 600, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("scrolling the list", scrolling));
    let stepping = measure_frames(&mut harness, 600, |harness, _| {
        harness.key_press(egui::Key::ArrowDown);
    });
    worst = worst.max(report("stepping through them (each read)", stepping));
    snapshot(&mut harness, &format!("requests-{screen}"));
    println!(
        "  peak memory {:.0} MB (the repository made beforehand not counted)",
        peak_memory_mb()
    );
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}

/// The refs of the sidebar's measurement: branches in folders, a remote's branches, and tags.
const SIDEBAR_BRANCHES: usize = 50_000;
const SIDEBAR_REMOTE_BRANCHES: usize = 30_000;
const SIDEBAR_TAGS: usize = 20_000;
const SIDEBAR_COMMITS: usize = 1_000;

/// A history of `SIDEBAR_COMMITS` commits, and 100,000 refs scattered over them (packed, as a
/// repository with that many has them): branches in 100 folders, `origin`'s branches, tags.
fn many_refs(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("refs").unwrap();
    let mut stream = Vec::new();
    for index in 0..SIDEBAR_COMMITS {
        let message = format!("Commit {index}\n");
        let _ = write!(
            stream,
            "commit refs/heads/main\nmark :{}\ncommitter T <t@egit.invalid> {} +0000\ndata {}\n\
             {message}\n",
            index + 1,
            1_700_000_000 + index,
            message.len(),
        );
    }
    let mut seed = 7_u64;
    let commits = u64::try_from(SIDEBAR_COMMITS).unwrap();
    let mut at = || random(&mut seed, commits) + 1;
    for index in 0..SIDEBAR_BRANCHES {
        let _ = write!(
            stream,
            "reset refs/heads/team{}/feature-{index}\nfrom :{}\n\n",
            index % 100,
            at()
        );
    }
    for index in 0..SIDEBAR_REMOTE_BRANCHES {
        let _ = write!(
            stream,
            "reset refs/remotes/origin/topic-{index}\nfrom :{}\n\n",
            at()
        );
    }
    for index in 0..SIDEBAR_TAGS {
        let _ = write!(
            stream,
            "reset refs/tags/v{}.{}.{}\nfrom :{}\n\n",
            index / 1000,
            (index / 10) % 100,
            index % 10,
            at()
        );
    }
    sandbox
        .git()
        .write("fast-import")
        .arg("--quiet")
        .current_dir(&repo)
        .stdin(stream)
        .output()
        .unwrap();
    for args in [
        &["symbolic-ref", "HEAD", "refs/heads/main"][..],
        &["reset", "--quiet", "--hard"],
        &["remote", "add", "origin", "file:///nowhere"],
        &["pack-refs", "--all"],
    ] {
        sandbox.run(&repo, args[0], &args[1..]).unwrap();
    }
    repo
}

/// The command palette over the corpus's branches: every one offered to switch to, gathered
/// as it opens, then narrowed a letter at a time; the slower 99th percentile.
fn palette_on_many_branches(harness: &mut Harness<'_, EgitApp>) -> Duration {
    let opening = measure_frames(harness, 1, |harness, _| {
        harness.event(egui::Event::Key {
            key: egui::Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
    });
    let opened = report("the palette opened", opening);
    harness.run_steps(2);
    let typed = "switch feature-4999";
    let narrowing = measure_frames(harness, typed.len(), |harness, at| {
        let letter = typed.get(at..=at).unwrap_or_default();
        harness.event(egui::Event::Text(letter.to_owned()));
    });
    let typing = report("typing in the palette", narrowing);
    harness.run_steps(2);
    // Only the palette's rows say so: the sidebar's are the branches' names.
    assert!(
        harness
            .query_all_by_label_contains("/feature-4999")
            .any(|row| {
                row.accesskit_node()
                    .label()
                    .is_some_and(|label| label.starts_with("Switch to"))
            })
    );
    snapshot(harness, "palette");
    opened.max(typing)
}

/// The sidebar's filter typed a letter at a time (each narrowing every list again, in a job),
/// then the rows it narrowed to awaited; returns the typing's 99th percentile frame.
fn filter_many_refs(harness: &mut Harness<'_, EgitApp>) -> Duration {
    harness
        .get_by_role_and_label(Role::TextInput, "Filter")
        .focus();
    harness.run_steps(2);
    let typed = "feature-499";
    let typing = measure_frames(harness, typed.len(), |harness, at| {
        let letter = typed.get(at..=at).unwrap_or_default();
        harness.event(egui::Event::Text(letter.to_owned()));
    });
    let worst = report("typing in the filter", typing);
    let matching = (0..SIDEBAR_BRANCHES)
        .filter(|index| format!("feature-{index}").contains(typed))
        .count();
    // The rows land a frame or a few later.
    let started = Instant::now();
    let counted = format!("Branches: {matching}");
    while harness.query_by_label(&counted).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{counted} never shown"
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("  narrowed {:?} after the last letter", started.elapsed());
    snapshot(harness, "sidebar-filtered");
    worst
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_sidebar() {
    let total = SIDEBAR_BRANCHES + SIDEBAR_REMOTE_BRANCHES + SIDEBAR_TAGS;
    println!("the sidebar on {total} refs over {SIDEBAR_COMMITS} commits:");
    let sandbox = Sandbox::new().unwrap();
    let repo = many_refs(&sandbox);
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
    reset_peak_memory();
    let started = Instant::now();
    harness.state_mut().state.actions.push(Action::Open(repo));
    // `main` too.
    let branches = format!("Branches: {}", SIDEBAR_BRANCHES + 1);
    let mut longest = Duration::ZERO;
    while harness.query_by_label(&branches).is_none() {
        assert!(started.elapsed() < Duration::from_secs(60), "no sidebar");
        let frame = Instant::now();
        harness.step();
        longest = longest.max(frame.elapsed());
    }
    println!(
        "  listed {:?} after opening (its longest frame {longest:?}: the refs sorted)",
        started.elapsed()
    );
    step_until(&mut harness, Duration::from_secs(60), |app| {
        app.state
            .repos
            .first()
            .is_some_and(|repo| !repo.history.is_loading())
    });

    // Scrolled through the branches, folded into 100 folders of 500.
    harness.hover_at(egui::pos2(130.0, 500.0));
    let wheel = |harness: &mut Harness<'_, EgitApp>, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -600.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    };
    let mut worst = report(
        "wheel scrolling the branches",
        measure_frames(&mut harness, 600, wheel),
    );

    // Back to the top, the branches and the remote folded, and 20,000 tags unfolded, then
    // scrolled through.
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 1.0e9),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(3);
    for header in [branches, "Remotes: 1".to_owned()] {
        harness.get_by_role_and_label(Role::Button, &header).click();
        harness.run_steps(2);
    }
    let tags = format!("Tags: {SIDEBAR_TAGS}");
    harness.get_by_role_and_label(Role::Button, &tags).click();
    let started = Instant::now();
    harness.step();
    println!("  tags unfolded in a frame of {:?}", started.elapsed());
    worst = worst.max(report(
        "wheel scrolling to and through the tags",
        measure_frames(&mut harness, 600, wheel),
    ));
    snapshot(&mut harness, "sidebar");

    worst = worst.max(filter_many_refs(&mut harness));

    worst = worst.max(palette_on_many_branches(&mut harness));
    println!("  peak memory {:.0} MB", peak_memory_mb());
    if !cfg!(debug_assertions) {
        assert!(
            worst < Duration::from_millis(8),
            "the 99th percentile frame took {worst:?}"
        );
    }
}

/// A Markdown document of about `bytes`: sections of headings, paragraphs with emphasis, code,
/// and links, lists (tasks among them), code blocks, and tables; `edition` changes its words.
fn document(bytes: usize, edition: &str) -> String {
    let mut text = String::with_capacity(bytes + 1024);
    let mut section = 0;
    while text.len() < bytes {
        section += 1;
        let _ = write!(
            text,
            "## Section {section} ({edition})\n\nThe *parser* reads `input-{section}` and \
             writes **output**, as [the guide](https://example.com/guide/{section}) says. \
             Every line is checked twice, then kept.\n\n- first point\n- second point, longer \
             than the first\n- [ ] a task left\n- [x] a task done\n\n```rust\nfn section_\
             {section}() -> u32 {{\n    {section}\n}}\n```\n\n| Option | Meaning |\n|---|---|\n\
             | `-q` | quiet |\n| `-v` | verbose |\n\n"
        );
    }
    text
}

#[test]
#[ignore = "a release-build measurement: see the module's documentation"]
fn corpus_markdown() {
    let largest = usize::try_from(crate::markdown::LARGEST).unwrap() - 4096;
    println!(
        "a Markdown document of {} KiB changed, previewed:",
        largest / 1024
    );
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "README.md", &document(largest, "first"), "Write it")
        .unwrap();
    sandbox
        .commit_file(
            &repo,
            "README.md",
            &document(largest, "second"),
            "Rewrite it",
        )
        .unwrap();
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
    let started = Instant::now();
    while harness
        .query_by_role_and_label(Role::Button, "Rewrite it")
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(30), "no subject");
        harness.step();
    }
    harness
        .get_by_role_and_label(Role::Button, "Rewrite it")
        .click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state.repos[0].history_view.details_shown()
    });
    harness
        .get_by_role_and_label(Role::Button, "README.md")
        .click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state.repos[0].history_view.file_shown() == Some(("README.md".to_owned(), true))
    });
    reset_peak_memory();
    let started = Instant::now();
    harness.get_by_label("Preview").click();
    step_until(&mut harness, Duration::from_secs(30), |app| {
        app.state.repos[0].history_view.file_shown() == Some(("README.md".to_owned(), true))
    });
    harness.run_steps(1);
    println!("  read and first drawn in {:?}", started.elapsed());
    let idle = measure_frames(&mut harness, 60, |_, _| {});
    let mut worst = report("still", idle);
    harness.hover_at(egui::pos2(1000.0, 500.0));
    let scrolling = measure_frames(&mut harness, 300, |harness, _| {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -600.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
    });
    worst = worst.max(report("wheel scrolling", scrolling));
    snapshot(&mut harness, "markdown");
    println!("  peak memory {} MB", peak_memory_mb());
    assert!(worst < Duration::from_millis(8), "p99 {worst:?} past 8 ms");
}
