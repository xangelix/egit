//! The page's WebSocket to its relay, as a `egit_git::relay::Transport`.
//!
//! The socket itself is the page's (a JavaScript object, kept on the page's one thread, in a
//! thread-local table); what egit holds is its number, so a `Relay` stays `Send` as egit's
//! futures are. Messages sent before it opened wait, and go once it has.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use egit_git::{Relay, Transport};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, MessageEvent, WebSocket};

/// A socket and what waits to be sent on it.
struct Socket {
    websocket: WebSocket,
    open: bool,
    waiting: VecDeque<Vec<u8>>,
    /// Its handlers, kept as long as it is.
    handlers: Vec<Closure<dyn FnMut(JsValue)>>,
}

thread_local! {
    static SOCKETS: RefCell<HashMap<u64, Socket>> = RefCell::new(HashMap::new());
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A socket by its number.
struct Numbered(u64);

impl Transport for Numbered {
    fn send(&self, message: Vec<u8>) {
        SOCKETS.with(|sockets| {
            if let Some(socket) = sockets.borrow_mut().get_mut(&self.0) {
                if socket.open {
                    if let Err(error) = socket.websocket.send_with_u8_array(&message) {
                        tracing::warn!(?error, "a message to the relay wasn't sent");
                    }
                } else {
                    socket.waiting.push_back(message);
                }
            }
        });
    }

    fn close(&self) {
        SOCKETS.with(|sockets| {
            if let Some(mut socket) = sockets.borrow_mut().remove(&self.0) {
                let _closed = socket.websocket.close();
                // Its handlers go with it: nothing more is heard from it.
                socket.handlers.clear();
            }
        });
    }
}

/// Opens the relay's WebSocket at `address`, saying `token` first.
pub(super) fn connect(address: &str, token: &str) -> Result<Relay, JsValue> {
    let socket = WebSocket::new(address)?;
    socket.set_binary_type(BinaryType::Arraybuffer);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    SOCKETS.with(|sockets| {
        sockets.borrow_mut().insert(
            number,
            Socket {
                websocket: socket.clone(),
                open: false,
                waiting: VecDeque::new(),
                handlers: Vec::new(),
            },
        );
    });
    let relay = Relay::new(Numbered(number), token.as_bytes());

    let opened = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        SOCKETS.with(|sockets| {
            if let Some(socket) = sockets.borrow_mut().get_mut(&number) {
                socket.open = true;
                while let Some(message) = socket.waiting.pop_front() {
                    if let Err(error) = socket.websocket.send_with_u8_array(&message) {
                        tracing::warn!(?error, "a message to the relay wasn't sent");
                    }
                }
            }
        });
    });
    let receiving = relay.clone();
    let received = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let Ok(event) = event.dyn_into::<MessageEvent>() else {
            return;
        };
        if let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
            receiving.receive(&js_sys::Uint8Array::new(&buffer).to_vec());
        }
    });
    let ending = relay.clone();
    let closed = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let why = event
            .dyn_into::<CloseEvent>()
            .ok()
            .map(|event| event.reason())
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| "The connection to the relay was lost.".to_owned());
        ending.closed(&why);
        SOCKETS.with(|sockets| sockets.borrow_mut().remove(&number));
    });
    socket.set_onopen(Some(opened.as_ref().unchecked_ref()));
    socket.set_onmessage(Some(received.as_ref().unchecked_ref()));
    socket.set_onclose(Some(closed.as_ref().unchecked_ref()));
    SOCKETS.with(|sockets| {
        if let Some(socket) = sockets.borrow_mut().get_mut(&number) {
            socket.handlers = vec![opened, received, closed];
        }
    });
    Ok(relay)
}
