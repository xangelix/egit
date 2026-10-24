//! The files of the computer egit runs on, as [`Files`](crate::Files) are git's computer's.
//!
//! Where egit saves what it is asked to (a patch, patches into a folder), and reads what the
//! person chose (patches to apply, images for an agent).
//!
//! On a computer, `std::fs`. In a browser, which keeps no files of a page's, what the person
//! chose in the browser's dialog is given to egit (`give`) and read from the page's memory under
//! a path of its own (the latest `KEPT` files); what egit saves is offered as a download
//! named as the file would have been, the browser saving it where it saves downloads (a folder
//! asked for is a name, [`DOWNLOADS`]).

use std::io;
use std::path::Path;
#[cfg(target_arch = "wasm32")]
use std::path::PathBuf;

/// In a browser, the folder egit's saves are said to go into: the browser's downloads.
pub const DOWNLOADS: &str = "Downloads";

/// In a browser, how many files given are kept at most (the oldest forgotten first).
#[cfg(target_arch = "wasm32")]
pub const KEPT: usize = 32;

/// `path`'s bytes.
///
/// # Errors
///
/// When it can't be read; in a browser, when it wasn't given (or was forgotten since).
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    return browser::read(path);
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::read(path)
}

/// How many bytes `path` holds.
///
/// # Errors
///
/// As [`read`].
pub fn size(path: &Path) -> io::Result<u64> {
    #[cfg(target_arch = "wasm32")]
    return browser::size(path);
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::metadata(path).map(|metadata| metadata.len())
}

/// Writes `bytes` to `path` (in a browser, a download named as its file is).
///
/// # Errors
///
/// When it can't be written; in a browser, when the page can't offer it.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(target_arch = "wasm32")]
    return browser::download(path, bytes);
    #[cfg(not(target_arch = "wasm32"))]
    std::fs::write(path, bytes)
}

/// Makes `folder` and those it is in.
///
/// # Errors
///
/// When it can't be made.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_dir_all(folder: &Path) -> io::Result<()> {
    std::fs::create_dir_all(folder)
}

/// Nothing, in a browser: its downloads have a folder of their own.
///
/// # Errors
///
/// Never (as a computer's, which can fail).
#[cfg(target_arch = "wasm32")]
pub const fn create_dir_all(_folder: &Path) -> io::Result<()> {
    Ok(())
}

/// In a browser, a file the person gave the page (named `name`), kept to be read through
/// [`read`] at the path returned.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn give(name: &str, bytes: Vec<u8>) -> PathBuf {
    browser::give(name, bytes)
}

/// The page's own: the files given, and downloads.
#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use wasm_bindgen::{JsCast as _, JsValue};

    use super::KEPT;

    /// How long a download's address lives: the browser has read it long before.
    const ADDRESS_LIVES: Duration = Duration::from_secs(60);

    /// The files given (each its path and bytes), the latest last, and how many were.
    type Given = (VecDeque<(PathBuf, Arc<[u8]>)>, u64);

    thread_local! {
        static GIVEN: RefCell<Given> = const { RefCell::new((VecDeque::new(), 0)) };
    }

    pub(super) fn give(name: &str, bytes: Vec<u8>) -> PathBuf {
        GIVEN.with_borrow_mut(|(given, count)| {
            *count = count.wrapping_add(1);
            // Its own folder: two files of one name stay apart, and the name is kept whole.
            let path = PathBuf::from(format!("/given/{count}")).join(name);
            if given.len() >= KEPT {
                given.pop_front();
            }
            given.push_back((path.clone(), Arc::from(bytes)));
            path
        })
    }

    fn given(path: &Path) -> io::Result<Arc<[u8]>> {
        GIVEN.with_borrow(|(given, _)| {
            given
                .iter()
                .find(|(kept, _)| kept == path)
                .map(|(_, bytes)| Arc::clone(bytes))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "the browser gave no such file")
                })
        })
    }

    pub(super) fn read(path: &Path) -> io::Result<Vec<u8>> {
        given(path).map(|bytes| bytes.to_vec())
    }

    pub(super) fn size(path: &Path) -> io::Result<u64> {
        given(path).map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
    }

    /// Offers `bytes` as a download named as `path`'s file: a link to them, clicked.
    pub(super) fn download(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let name = path
            .file_name()
            .map_or_else(|| "download".into(), |name| name.to_string_lossy());
        let failed = |error: JsValue| io::Error::other(format!("the browser refused: {error:?}"));
        let window = web_sys::window().ok_or_else(|| io::Error::other("no window"))?;
        let document = window
            .document()
            .ok_or_else(|| io::Error::other("no document"))?;
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("application/octet-stream");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(failed)?;
        let address = web_sys::Url::create_object_url_with_blob(&blob).map_err(failed)?;
        let link = document
            .create_element("a")
            .map_err(failed)?
            .dyn_into::<web_sys::HtmlAnchorElement>()
            .map_err(|_| io::Error::other("not a link"))?;
        link.set_href(&address);
        link.set_download(&name);
        link.click();
        // Its bytes let go once the browser has them.
        let forgotten = crate::exec::spawn("forgetting a download", async move {
            crate::exec::sleep(ADDRESS_LIVES).await;
            let _revoked = web_sys::Url::revoke_object_url(&address);
        });
        if let Err(error) = forgotten {
            tracing::debug!(%error, "a download's address is kept");
        }
        Ok(())
    }
}
