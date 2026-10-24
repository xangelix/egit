//! Files chosen in the browser's dialog: each read into the page and given to egit
//! (`egit_git::local::give`), which reads it by the path it is given under, as a desktop's
//! dialog's answer is read.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use tokio::sync::oneshot;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// The most a file chosen may hold, read whole into the page (a patch, an image).
const MOST: f64 = 256.0 * 1024.0 * 1024.0;

/// Asks the browser's dialog for files of the kinds `accept` names (an `<input>`'s `accept`),
/// several at once: each read and given to egit. Their paths; none when the dialog was
/// cancelled, or couldn't open.
pub(crate) async fn choose(accept: &'static str) -> Vec<PathBuf> {
    let (sender, receiver) = oneshot::channel();
    if let Err(error) = open(accept, sender) {
        tracing::warn!(?error, "the browser's dialog couldn't open");
        return Vec::new();
    }
    receiver.await.unwrap_or_default()
}

/// Opens the dialog: what is chosen (or that nothing was) goes to `sender`.
fn open(accept: &str, sender: oneshot::Sender<Vec<PathBuf>>) -> Result<(), JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let input = document
        .create_element("input")?
        .dyn_into::<web_sys::HtmlInputElement>()?;
    input.set_type("file");
    input.set_multiple(true);
    input.set_accept(accept);
    // Whichever comes first answers: files chosen, or the dialog cancelled.
    let sender = Rc::new(RefCell::new(Some(sender)));
    let chosen = {
        let (sender, input) = (Rc::clone(&sender), input.clone());
        Closure::once_into_js(move || {
            if let Some(sender) = sender.borrow_mut().take() {
                let files = input.files();
                wasm_bindgen_futures::spawn_local(async move {
                    // Nobody waits any more: nothing to tell.
                    let _sent = sender.send(read_all(files).await);
                });
            }
        })
    };
    let cancelled = Closure::once_into_js(move || {
        if let Some(sender) = sender.borrow_mut().take() {
            let _sent = sender.send(Vec::new());
        }
    });
    input.add_event_listener_with_callback("change", chosen.unchecked_ref())?;
    input.add_event_listener_with_callback("cancel", cancelled.unchecked_ref())?;
    input.click();
    Ok(())
}

/// Reads `files` into the page, each given to egit; one too large or unreadable is left out.
#[expect(
    clippy::future_not_send,
    reason = "the page's files, read on its one thread"
)]
async fn read_all(files: Option<web_sys::FileList>) -> Vec<PathBuf> {
    let Some(files) = files else {
        return Vec::new();
    };
    let mut given = Vec::new();
    for index in 0..files.length() {
        let Some(file) = files.item(index) else {
            continue;
        };
        if file.size() > MOST {
            tracing::warn!(name = file.name(), "a file too large to read into the page");
            continue;
        }
        match JsFuture::from(file.array_buffer()).await {
            Ok(buffer) => given.push(egit_git::local::give(
                &file.name(),
                js_sys::Uint8Array::new(&buffer).to_vec(),
            )),
            Err(error) => tracing::warn!(?error, name = file.name(), "a file couldn't be read"),
        }
    }
    given
}
