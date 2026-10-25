//! egit in a browser: the page's entry. Its WebAssembly starts egit in the page's canvas
//! (`#egit`), every git run through the relay that served it (`egit-relay`); what stops it is
//! said in the page (`#problem`). The same WebAssembly, loaded again by each of the page's
//! workers (`worker.js`, on the page's memory), runs egit's work there (`egit_worker`).
//!
//! Built by `scripts/build-web.sh` into `target/web`, which `egit-relay --web` serves: threaded
//! (atomics, shared memory), as egit in a browser is. Anywhere but a browser this crate is
//! empty.

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
compile_error!(
    "egit in a browser runs threaded: build it with scripts/build-web.sh (atomics, shared \
     memory, and the standard library built with them)"
);

#[cfg(target_arch = "wasm32")]
mod page {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::prelude::*;

    /// Starts egit once the page has loaded its WebAssembly (on the page's main thread alone:
    /// a worker loading it again starts nothing here).
    #[wasm_bindgen(start)]
    fn main() {
        if web_sys::window().is_none() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async {
            if let Err(error) = run().await {
                say(&error);
            }
        });
    }

    #[expect(
        clippy::future_not_send,
        reason = "eframe's runner holds the page's own objects, on its main thread"
    )]
    async fn run() -> Result<(), JsValue> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("no document"))?;
        let canvas = document
            .get_element_by_id("egit")
            .ok_or_else(|| JsValue::from_str("the page has no canvas"))?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;
        egit_app::start(canvas).await
    }

    /// A worker's entry, once its WebAssembly runs on the page's memory: it takes egit's work.
    #[wasm_bindgen]
    #[allow(
        unreachable_pub,
        reason = "wasm-bindgen exports it to the worker's script"
    )]
    pub fn egit_worker() {
        egit_git::exec::work();
    }

    /// Says in the page why egit didn't start.
    fn say(error: &JsValue) {
        let words = error.as_string().unwrap_or_else(|| format!("{error:?}"));
        let shown = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("problem"))
            .and_then(|problem| problem.dyn_into::<web_sys::HtmlElement>().ok());
        if let Some(problem) = shown {
            problem.set_inner_text(&format!("egit couldn't start: {words}"));
            problem.set_hidden(false);
        }
    }
}
