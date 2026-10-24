//! egit in a browser: the page's entry. Its WebAssembly starts egit in the page's canvas
//! (`#egit`), every git run through the relay that served it (`egit-relay`); what stops it is
//! said in the page (`#problem`).
//!
//! Built by `scripts/build-web.sh` into `target/web`, which `egit-relay --web` serves. Anywhere
//! but a browser this crate is empty.

#[cfg(target_arch = "wasm32")]
mod page {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::prelude::*;

    /// Starts egit once the page has loaded its WebAssembly.
    #[wasm_bindgen(start)]
    fn main() {
        wasm_bindgen_futures::spawn_local(async {
            if let Err(error) = run().await {
                say(&error);
            }
        });
    }

    #[expect(
        clippy::future_not_send,
        reason = "a page's work runs on its one thread, eframe's runner with it"
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
