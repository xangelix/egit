//! Asking the window to draw again, from wherever work is done: a job, a thread of its own, a
//! worker of the page's.
//!
//! On a computer egui's context asks. In a browser's threaded page the context lives on the main
//! thread alone (it holds the page's own objects), so the asking is handed to that thread, once
//! however often a worker asked meanwhile.

use std::time::Duration;

/// What asks the window to draw again; `Send`, so work done elsewhere holds one.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub(crate) struct Repaint(egui::Context);

/// What asks the window to draw again; `Send`, so work done elsewhere holds one.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub(crate) struct Repaint;

#[cfg(not(target_arch = "wasm32"))]
impl Repaint {
    /// What asks `ctx`'s window.
    pub(crate) fn of(ctx: &egui::Context) -> Self {
        Self(ctx.clone())
    }

    /// Draws again soon.
    pub(crate) fn request_repaint(&self) {
        self.0.request_repaint();
    }

    /// Draws again after `delay` at the latest.
    pub(crate) fn request_repaint_after(&self, delay: Duration) {
        self.0.request_repaint_after(delay);
    }
}

#[cfg(target_arch = "wasm32")]
impl Repaint {
    /// What asks `ctx`'s window, the page's one (kept on the main thread, where `ctx` lives).
    pub(crate) fn of(ctx: &egui::Context) -> Self {
        page::keep(ctx);
        Self
    }

    /// Draws again soon.
    #[expect(clippy::unused_self, reason = "a computer's asks its own context")]
    pub(crate) fn request_repaint(&self) {
        page::ask(None);
    }

    /// Draws again after `delay` at the latest.
    #[expect(clippy::unused_self, reason = "a computer's asks its own context")]
    pub(crate) fn request_repaint_after(&self, delay: Duration) {
        page::ask(Some(delay));
    }
}

/// The page's context, on its main thread, and what workers asked of it.
#[cfg(target_arch = "wasm32")]
mod page {
    use std::cell::OnceCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    thread_local! {
        /// The window's context: the main thread's alone.
        static CONTEXT: OnceCell<egui::Context> = const { OnceCell::new() };
    }

    /// Whether the main thread was asked to draw again and hasn't yet.
    static ASKED: AtomicBool = AtomicBool::new(false);

    /// Keeps `ctx` as the page's (on the main thread, where it lives).
    pub(super) fn keep(ctx: &egui::Context) {
        CONTEXT.with(|context| {
            context.get_or_init(|| ctx.clone());
        });
    }

    /// Asks the page to draw again, after `delay` at the latest (or soon): at once on the main
    /// thread, else handed to it (asked once however often meanwhile, but for delays).
    pub(super) fn ask(delay: Option<Duration>) {
        if egit_git::exec::is_main() {
            draw(delay);
            return;
        }
        if delay.is_none() && ASKED.swap(true, Ordering::AcqRel) {
            return;
        }
        egit_git::exec::on_main(move || {
            if delay.is_none() {
                ASKED.store(false, Ordering::Release);
            }
            draw(delay);
        });
    }

    /// On the main thread: draws again after `delay`, or soon.
    fn draw(delay: Option<Duration>) {
        CONTEXT.with(|context| {
            if let Some(ctx) = context.get() {
                match delay {
                    Some(delay) => ctx.request_repaint_after(delay),
                    None => ctx.request_repaint(),
                }
            }
        });
    }
}
