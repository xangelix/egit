//! egit in a browser: what the page gives in place of a computer's own (watches, threads,
//! dialogs: `files`), and its connection to its relay.
//!
//! The page runs threaded: its main thread draws and alone touches the page's own objects, and a
//! pool of web workers sharing its WebAssembly and memory runs egit's work (`egit_git::exec`).
//!
//! The page egit is served from is its relay's (`egit-relay`): its address carries the relay's
//! token after `#`, which a browser never sends in a request. The token is kept for the tab
//! (`sessionStorage`, so reloading needs no address again) and taken out of the address bar
//! (history, bookmarks, and a screen shared show no token). Then egit opens the relay's
//! WebSocket (`relay`, beside the page) and runs every git through it.
//!
//! Whatever the page asks of its relay is relative to the page's own address, never the
//! host's root: a proxy may serve the relay under a path of its own (code-server's
//! `/proxy/<port>/`), and `index.html` adds the last slash to that path where it was left off.

pub(crate) mod files;
pub(crate) mod notify;
mod socket;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use crate::app::EgitApp;

/// Where the token is kept for the tab.
const TOKEN_KEY: &str = "egit-relay-token";

/// Whether a field of egit's has the keys, as the last frame left it.
static TYPING: AtomicBool = AtomicBool::new(false);

/// The browser's language, as the page started: workers read it, and have no window to ask.
static LANGUAGE: OnceLock<String> = OnceLock::new();

/// The script a worker of the page's runs (`egit-web`'s `worker.js`, beside the page).
const WORKER: &str = "./worker.js";

/// The most workers the page starts.
const MOST_WORKERS: u32 = 8;

/// Starts egit in `canvas`, its git through the relay that served the page.
///
/// # Errors
///
/// When the page has no relay's token, its WebSocket can't be opened, or egui can't start.
#[expect(
    clippy::future_not_send,
    reason = "the page's own future, on its one thread, holding the page's objects"
)]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    // Its warnings in the page's console (a logger set already is the one kept).
    let _logging = eframe::WebLogger::init(log::LevelFilter::Info);
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    // This is the page's main thread, which draws; egit's work is its workers'.
    egit_git::exec::serve_main();
    prepare(&window);
    egit_git::exec::start_workers(WORKER, workers(&window)).map_err(|error| {
        JsValue::from_str(&format!("the page's workers couldn't start: {error}"))
    })?;
    let token = token(&window)?;
    let location = window.location();
    // Beside the page, wherever it was served: the host's root, or a proxy's path.
    let address = web_sys::Url::new_with_base("relay", &location.href()?)?;
    let secure = address.protocol() == "https:";
    address.set_protocol(if secure { "wss:" } else { "ws:" });
    let address = address.href();
    let paths = wanted(&window)?;
    // Out of the address bar, its history, and a screen shared: the token; and what was opened
    // (a reload opens it again with the tabs left open).
    if let Ok(history) = window.history() {
        let path = location.pathname()?;
        let _replaced = history.replace_state_with_url(&JsValue::NULL, "", Some(&path));
    }
    let relay = socket::connect(&address, &token)?;
    keep_shortcuts(&window)?;
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(move |creation| Ok(Box::new(EgitApp::in_browser(creation, relay, paths)))),
        )
        .await
}

/// Makes now, on the main thread, what both it and the workers read (they mustn't race to make
/// it: the main thread may never wait), and notes what only the main thread can ask the page.
fn prepare(window: &web_sys::Window) {
    crate::syntax::prepare();
    for weight in crate::theme::Weight::ALL {
        let _ = weight.family();
    }
    let _ = crate::history::gix_allowed();
    let _ = crate::format::local_zone();
    let _ = is_mac();
    let language = window.navigator().language().unwrap_or_default();
    let _ = LANGUAGE.set(language);
}

/// How many workers the page starts: one for each core but the main thread's, two at least
/// (one computing leaves another waiting on the relay), [`MOST_WORKERS`] at most.
fn workers(window: &web_sys::Window) -> usize {
    let cores = window.navigator().hardware_concurrency();
    let counted = (1..=MOST_WORKERS)
        .take_while(|count| f64::from(*count) < cores)
        .last()
        .unwrap_or(1);
    usize::try_from(counted.max(2)).unwrap_or(2)
}

/// The repositories the address asks to open (`?open=<path>`, each the relay's computer's;
/// `~` its home folder), as egit's command line names them.
fn wanted(window: &web_sys::Window) -> Result<Vec<PathBuf>, JsValue> {
    let search = window.location().search()?;
    let asked = web_sys::UrlSearchParams::new_with_str(&search)?.get_all("open");
    Ok(asked
        .iter()
        .filter_map(|path| path.as_string())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Titles the page by the repository in front (`repo — egit`), or none (`egit`); the page is
/// touched only when that changes.
pub(crate) fn title(front: Option<&str>) {
    thread_local! {
        static SHOWN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }
    let title = front.map_or_else(|| "egit".to_owned(), |front| format!("{front} — egit"));
    SHOWN.with_borrow_mut(|shown| {
        if shown.as_deref() == Some(title.as_str()) {
            return;
        }
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&title);
        }
        *shown = Some(title);
    });
}

/// Notes whether a field of egit's has the keys (each frame), for [`keep_shortcuts`].
pub(crate) fn note_typing(typing: bool) {
    TYPING.store(typing, Ordering::Relaxed);
}

/// The browser's language (`ja-JP`), or none; on any thread.
pub(crate) fn language() -> String {
    LANGUAGE.get().cloned().unwrap_or_default()
}

/// Whether the browser runs on a Mac (its ⌘ is egit's command key, and named so).
pub(crate) fn is_mac() -> bool {
    static MAC: OnceLock<bool> = OnceLock::new();
    *MAC.get_or_init(|| {
        web_sys::window()
            .and_then(|window| window.navigator().platform().ok())
            .is_some_and(|platform| platform.starts_with("Mac") || platform.starts_with("iP"))
    })
}

/// Keeps egit's shortcuts from the browser, which would act on them too (finding in the page,
/// its address bar, reloading it): eframe keeps only a few. Before eframe sees a key (the
/// window's, while capturing), its default is prevented; egit still gets it. Those the browser
/// never gives a page (⌘W, ⌘1 … ⌘9) are ⌥'s here (`commands`), but where ⌥ types a character.
fn keep_shortcuts(window: &web_sys::Window) -> Result<(), JsValue> {
    let keep =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(|event: web_sys::KeyboardEvent| {
            if is_egits(&event) {
                event.prevent_default();
            }
        });
    window.add_event_listener_with_callback_and_bool(
        "keydown",
        keep.as_ref().unchecked_ref(),
        true,
    )?;
    // Listening for the page's life.
    keep.forget();
    Ok(())
}

/// Whether `event` is one of egit's shortcuts (`commands`, and the views' own) the browser
/// would also act on. Copying, cutting, and pasting are the browser's to do: egui hears them
/// as its events.
fn is_egits(event: &web_sys::KeyboardEvent) -> bool {
    let key = event.key();
    if key == "F1" {
        return true;
    }
    let command = if is_mac() {
        event.meta_key()
    } else {
        event.ctrl_key()
    };
    if command {
        // Text's size (⌘+ ⌘− ⌘0), the palette, search, the sidebar and its filter, the output,
        // the agents, reading again, undoing and redoing.
        return matches!(
            key.to_ascii_lowercase().as_str(),
            "k" | "f" | "b" | "j" | "l" | "r" | "z" | "y" | "=" | "+" | "-" | "0"
        );
    }
    // ⌥W and ⌥1 … ⌥9, by where they are (a Mac's ⌥ makes them other characters).
    event.alt_key() && !TYPING.load(Ordering::Relaxed) && {
        let code = event.code();
        code == "KeyW"
            || code.strip_prefix("Digit").is_some_and(|digit| {
                matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
    }
}

/// Opens `url` in a new tab of this browser (as a link would: nothing of egit's page passed to
/// it). A browser blocking pop-ups may refuse; it says so itself.
pub(crate) fn open_page(url: &str) {
    let opened = web_sys::window().map(|window| {
        window.open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer")
    });
    if let Some(Err(error)) = opened {
        tracing::warn!(url, ?error, "the page couldn't be opened");
    }
}

/// The relay's token: in the address after `#` (kept for the tab, and taken out of the
/// address), or kept from before.
fn token(window: &web_sys::Window) -> Result<String, JsValue> {
    let kept = window.session_storage().ok().flatten();
    let location = window.location();
    let given = location
        .hash()?
        .strip_prefix('#')
        .filter(|hash| !hash.is_empty())
        .map(str::to_owned);
    if let Some(token) = given {
        if let Some(kept) = &kept {
            // Not kept, it is asked for again on a reload: no worse.
            let _kept = kept.set_item(TOKEN_KEY, &token);
        }
        return Ok(token);
    }
    kept.and_then(|kept| kept.get_item(TOKEN_KEY).ok().flatten())
        .ok_or_else(|| {
            JsValue::from_str(
                "this page needs its relay's address, with the token after #: as egit-relay \
                 printed it",
            )
        })
}
