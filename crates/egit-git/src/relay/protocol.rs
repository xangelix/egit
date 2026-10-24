//! What egit and a relay say to each other: binary messages over one WebSocket.
//!
//! Each is its kind (a byte), the id of what it is about (four bytes), then its fields: numbers
//! little-endian, and each byte string its length (four bytes) and its bytes.
//!
//! The ids are the client's: a process's from its [`ToRelay::Start`] on, a prompt's or a
//! request's as the relay names it. Bytes are as the relay's computer has them (paths, arguments,
//! a program's output), never re-encoded.

use std::fmt;

use zeroize::Zeroizing;

use crate::output::Exit;

/// The protocol both sides speak; a relay refuses another.
pub const VERSION: u16 = 2;

/// The longest message either side takes: a command's arguments and environment are the largest,
/// and its output comes a chunk at a time.
pub const MOST: usize = 16 << 20;

/// How much of a process's standard output the relay sends before the client says it took it
/// in: the client keeps at most this much of each process's output waiting.
pub const WINDOW: u32 = 1 << 20;

/// What a program to run is: as `exec` takes it, where the relay runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    /// The program: a name on `PATH` there, or a path.
    pub program: Vec<u8>,
    /// Its arguments.
    pub args: Vec<Vec<u8>>,
    /// The folder it runs in (else the relay's home folder).
    pub dir: Option<Vec<u8>>,
    /// Variables set for it, over the relay's own.
    pub set: Vec<(Vec<u8>, Vec<u8>)>,
    /// Variables it doesn't get.
    pub unset: Vec<Vec<u8>>,
    /// Whether it reads standard input (else it reads nothing).
    pub stdin: bool,
}

/// A request a relay makes for egit (to a forge's API, a download, a picture), as `egit_forge`
/// makes it here.
#[derive(Clone, PartialEq, Eq)]
pub struct Fetch {
    /// GET or POST.
    pub method: Method,
    /// Where.
    pub url: String,
    /// Its headers: names, and values (a bearer's token among them, wiped when dropped).
    pub headers: Vec<(String, Zeroizing<Vec<u8>>)>,
    /// What a POST sends.
    pub body: Option<Vec<u8>>,
    /// How it is made.
    pub mode: FetchMode,
    /// Where a successful answer's body goes, when not to egit: the browser's downloads.
    pub deliver: Option<Delivery>,
}

/// A download handed to the browser rather than to egit.
///
/// A successful answer's body goes to the browser's downloads as `name`, by the relay
/// ([`FromRelay::Delivering`]), never through egit's connection; checked against `sha256`
/// where given (one that doesn't match fails in the browser, its last part held back).
#[derive(Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The file's name.
    pub name: String,
    /// Its bytes' SHA-256, where the forge said.
    pub sha256: Option<[u8; 32]>,
}

impl fmt::Debug for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Delivery")
            .field("name", &self.name)
            .field("checked", &self.sha256.is_some())
            .finish()
    }
}

/// A request's method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
}

/// How a request is made: what it may take, and whether redirects are followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchMode {
    /// A forge's API: redirects said, never followed (a credential never travels to another
    /// place), a minute in all.
    Api,
    /// A download (an artifact): redirects said, a minute for the answer to begin, its body as
    /// long as it takes (hours).
    Download,
    /// A picture: no credential, redirects followed over HTTPS alone, half a minute.
    Picture,
}

impl fmt::Debug for Fetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fetch")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("body", &self.body.as_ref().map(Vec::len))
            .field("mode", &self.mode)
            .field("deliver", &self.deliver)
            .finish()
    }
}

/// What egit says to the relay.
#[derive(PartialEq, Eq)]
pub enum ToRelay {
    /// The first message: who it is (the token the relay printed) and what it speaks.
    Hello {
        /// [`VERSION`].
        version: u16,
        /// The relay's token.
        token: Zeroizing<Vec<u8>>,
    },
    /// Runs `program` as process `id`.
    Start {
        /// The process.
        id: u32,
        /// What runs.
        program: Program,
    },
    /// More of process `id`'s standard input.
    Stdin {
        /// The process.
        id: u32,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Process `id`'s standard input ends.
    CloseStdin {
        /// The process.
        id: u32,
    },
    /// Stops process `id` (and every process it started): SIGTERM to its group.
    Terminate {
        /// The process.
        id: u32,
    },
    /// Process `id`'s standard output was taken in, `bytes` more of it: the relay may send that
    /// much more.
    Credit {
        /// The process.
        id: u32,
        /// How much more.
        bytes: u32,
    },
    /// Prompt `id`'s answer, or none (refused).
    Answer {
        /// The prompt.
        id: u32,
        /// What was typed, or chosen.
        answer: Option<Zeroizing<Vec<u8>>>,
    },
    /// Makes `fetch` as request `id`: its answer's head comes as [`FromRelay::Head`], its body
    /// as a process's output does (and as little of it ahead), its end as a process's.
    Fetch {
        /// The request.
        id: u32,
        /// What is asked.
        fetch: Fetch,
    },
}

/// What the relay says to egit.
#[derive(Debug, PartialEq, Eq)]
pub enum FromRelay {
    /// The answer to [`ToRelay::Hello`]: welcome, and where.
    Welcome {
        /// [`VERSION`].
        version: u16,
        /// The relay's home folder.
        home: Vec<u8>,
        /// The name of the computer it runs on.
        host: String,
    },
    /// The relay won't speak: a wrong token, another version. The connection ends.
    Refused {
        /// Why, in words.
        why: String,
    },
    /// Process `id` couldn't be started.
    Failed {
        /// The process.
        id: u32,
        /// Whether what was missing was the program or the folder it runs in.
        missing: Missing,
        /// The relay's words.
        message: String,
    },
    /// More of process `id`'s standard output.
    Stdout {
        /// The process.
        id: u32,
        /// The bytes.
        data: Vec<u8>,
    },
    /// More of process `id`'s standard error.
    Stderr {
        /// The process.
        id: u32,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Process `id` ended (after the last of its output).
    Exited {
        /// The process.
        id: u32,
        /// How.
        exit: Exit,
    },
    /// A program the relay runs asks something (git's or ssh's askpass): answered with
    /// [`ToRelay::Answer`].
    Prompt {
        /// The prompt.
        id: u32,
        /// What kind of answer it takes (askpass's `PromptKind`, as a number).
        kind: u8,
        /// What egit was doing, when said.
        context: Option<String>,
        /// The question.
        text: String,
    },
    /// Prompt `id`'s asker went away: it needs no answer.
    Withdrawn {
        /// The prompt.
        id: u32,
    },
    /// Request `id`'s answer begins: its status and headers.
    Head {
        /// The request.
        id: u32,
        /// Its status.
        status: u16,
        /// Its headers, names in lower case.
        headers: Vec<(String, String)>,
    },
    /// Request `id`'s successful answer, to be delivered ([`Fetch::deliver`]), waits for the
    /// browser at `path` (the relay's own, once): opened there, it is downloaded.
    Delivering {
        /// The request.
        id: u32,
        /// Where the browser takes it.
        path: String,
    },
    /// So much of request `id`'s answer went to the browser.
    Delivered {
        /// The request.
        id: u32,
        /// How much, in all.
        bytes: u64,
    },
}

/// What a process that couldn't start lacked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Nothing: it failed otherwise.
    Nothing,
    /// The program.
    Program,
    /// The folder it was to run in.
    Folder,
}

/// A message that isn't one: cut short, of no kind known, or too long.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a relay's message is garbled: {0}")]
pub struct Garbled(&'static str);

impl fmt::Debug for ToRelay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello { version, .. } => f
                .debug_struct("Hello")
                .field("version", version)
                .field("token", &"<secret>")
                .finish(),
            Self::Start { id, program } => f
                .debug_struct("Start")
                .field("id", id)
                .field("program", &bstr::BStr::new(&program.program))
                .field("args", &program.args.len())
                .finish_non_exhaustive(),
            Self::Stdin { id, data } => f
                .debug_struct("Stdin")
                .field("id", id)
                .field("bytes", &data.len())
                .finish(),
            Self::CloseStdin { id } => f.debug_struct("CloseStdin").field("id", id).finish(),
            Self::Terminate { id } => f.debug_struct("Terminate").field("id", id).finish(),
            Self::Credit { id, bytes } => f
                .debug_struct("Credit")
                .field("id", id)
                .field("bytes", bytes)
                .finish(),
            Self::Answer { id, answer } => f
                .debug_struct("Answer")
                .field("id", id)
                .field("answered", &answer.is_some())
                .finish(),
            Self::Fetch { id, fetch } => f
                .debug_struct("Fetch")
                .field("id", id)
                .field("fetch", fetch)
                .finish(),
        }
    }
}

mod kind {
    pub(super) const HELLO: u8 = 0x01;
    pub(super) const START: u8 = 0x02;
    pub(super) const STDIN: u8 = 0x03;
    pub(super) const CLOSE_STDIN: u8 = 0x04;
    pub(super) const TERMINATE: u8 = 0x05;
    pub(super) const CREDIT: u8 = 0x06;
    pub(super) const ANSWER: u8 = 0x07;
    pub(super) const FETCH: u8 = 0x08;

    pub(super) const WELCOME: u8 = 0x81;
    pub(super) const REFUSED: u8 = 0x82;
    pub(super) const FAILED: u8 = 0x83;
    pub(super) const STDOUT: u8 = 0x84;
    pub(super) const STDERR: u8 = 0x85;
    pub(super) const EXITED: u8 = 0x86;
    pub(super) const PROMPT: u8 = 0x87;
    pub(super) const WITHDRAWN: u8 = 0x88;
    pub(super) const HEAD: u8 = 0x89;
    pub(super) const DELIVERING: u8 = 0x8a;
    pub(super) const DELIVERED: u8 = 0x8b;
}

/// A message being written.
struct Writer(Vec<u8>);

impl Writer {
    fn new(kind: u8, id: u32, size: usize) -> Self {
        // Its kind, id, and a length or two, besides `size`.
        let mut bytes = Vec::with_capacity(size.saturating_add(16));
        bytes.push(kind);
        bytes.extend_from_slice(&id.to_le_bytes());
        Self(bytes)
    }

    fn u8(mut self, value: u8) -> Self {
        self.0.push(value);
        self
    }

    fn u16(mut self, value: u16) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn i32(mut self, value: i32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn u64(mut self, value: u64) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn bytes(mut self, value: &[u8]) -> Self {
        // A message longer than 4 GiB is never made: `MOST` refuses it first.
        let length = u32::try_from(value.len()).unwrap_or(u32::MAX);
        self.0.extend_from_slice(&length.to_le_bytes());
        self.0.extend_from_slice(value);
        self
    }

    fn optional(self, value: Option<&[u8]>) -> Self {
        match value {
            Some(value) => self.u8(1).bytes(value),
            None => self.u8(0),
        }
    }

    fn list<'a>(self, values: impl ExactSizeIterator<Item = &'a [u8]>) -> Self {
        let count = u32::try_from(values.len()).unwrap_or(u32::MAX);
        values.fold(self.u32(count), Self::bytes)
    }

    fn done(self) -> Vec<u8> {
        self.0
    }
}

/// A message being read: its fields in turn, each checked against what is left.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn take(&mut self, count: usize) -> Result<&'a [u8], Garbled> {
        if count > self.rest.len() {
            return Err(Garbled("cut short"));
        }
        let (taken, rest) = self.rest.split_at(count);
        self.rest = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, Garbled> {
        self.take(1)?.first().copied().ok_or(Garbled("cut short"))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Garbled> {
        self.take(N)?.try_into().map_err(|_| Garbled("cut short"))
    }

    fn u16(&mut self) -> Result<u16, Garbled> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, Garbled> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, Garbled> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Garbled> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn bytes(&mut self) -> Result<&'a [u8], Garbled> {
        let length = usize::try_from(self.u32()?).map_err(|_| Garbled("too long"))?;
        self.take(length)
    }

    fn owned(&mut self) -> Result<Vec<u8>, Garbled> {
        self.bytes().map(<[u8]>::to_vec)
    }

    fn text(&mut self) -> Result<String, Garbled> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    fn optional(&mut self) -> Result<Option<&'a [u8]>, Garbled> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.bytes().map(Some),
            _ => Err(Garbled("a field that is neither there nor not")),
        }
    }

    fn list(&mut self) -> Result<Vec<Vec<u8>>, Garbled> {
        let count = usize::try_from(self.u32()?).map_err(|_| Garbled("too long"))?;
        // Each entry takes four bytes at least: a count past that is a lie.
        if count > self.rest.len() >> 2 {
            return Err(Garbled("a list longer than its message"));
        }
        (0..count).map(|_| self.owned()).collect()
    }

    /// A request's fields.
    fn fetch(&mut self) -> Result<Fetch, Garbled> {
        let method = match self.u8()? {
            0 => Method::Get,
            1 => Method::Post,
            _ => return Err(Garbled("a method of no kind known")),
        };
        let mode = match self.u8()? {
            0 => FetchMode::Api,
            1 => FetchMode::Download,
            2 => FetchMode::Picture,
            _ => return Err(Garbled("a request of no kind known")),
        };
        let url =
            String::from_utf8(self.owned()?).map_err(|_| Garbled("an address that isn't text"))?;
        let names = self.list()?;
        let values = self.list()?;
        if names.len() != values.len() {
            return Err(Garbled("headers without values"));
        }
        let headers = names
            .into_iter()
            .zip(values)
            .map(|(name, value)| {
                String::from_utf8(name)
                    .map(|name| (name, Zeroizing::new(value)))
                    .map_err(|_| Garbled("a header's name that isn't text"))
            })
            .collect::<Result<_, _>>()?;
        let body = self.optional()?.map(<[u8]>::to_vec);
        let deliver = match self.u8()? {
            0 => None,
            1 => Some(Delivery {
                name: self.text()?,
                sha256: self
                    .optional()?
                    .map(|digest| {
                        <[u8; 32]>::try_from(digest)
                            .map_err(|_| Garbled("a digest that isn't SHA-256's"))
                    })
                    .transpose()?,
            }),
            _ => return Err(Garbled("a delivery that is neither asked nor not")),
        };
        Ok(Fetch {
            method,
            url,
            headers,
            body,
            mode,
            deliver,
        })
    }

    const fn end(&self) -> Result<(), Garbled> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(Garbled("more than its fields"))
        }
    }
}

/// Splits `message` into its kind, id, and fields.
fn open(message: &[u8]) -> Result<(u8, u32, Reader<'_>), Garbled> {
    if message.len() > MOST {
        return Err(Garbled("too long"));
    }
    let mut reader = Reader { rest: message };
    let kind = reader.u8()?;
    let id = reader.u32()?;
    Ok((kind, id, reader))
}

impl ToRelay {
    /// The message as sent.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Hello { version, token } => Writer::new(kind::HELLO, 0, token.len())
                .u16(*version)
                .bytes(token)
                .done(),
            Self::Start { id, program } => {
                let size = program
                    .args
                    .iter()
                    .chain(program.set.iter().flat_map(|(key, value)| [key, value]))
                    .fold(program.program.len(), |size, field| {
                        size.saturating_add(field.len()).saturating_add(4)
                    });
                Writer::new(kind::START, *id, size)
                    .bytes(&program.program)
                    .list(program.args.iter().map(Vec::as_slice))
                    .optional(program.dir.as_deref())
                    .list(program.set.iter().map(|(key, _)| key.as_slice()))
                    .list(program.set.iter().map(|(_, value)| value.as_slice()))
                    .list(program.unset.iter().map(Vec::as_slice))
                    .u8(u8::from(program.stdin))
                    .done()
            }
            Self::Stdin { id, data } => {
                Writer::new(kind::STDIN, *id, data.len()).bytes(data).done()
            }
            Self::CloseStdin { id } => Writer::new(kind::CLOSE_STDIN, *id, 0).done(),
            Self::Terminate { id } => Writer::new(kind::TERMINATE, *id, 0).done(),
            Self::Credit { id, bytes } => Writer::new(kind::CREDIT, *id, 4).u32(*bytes).done(),
            Self::Answer { id, answer } => Writer::new(kind::ANSWER, *id, 0)
                .optional(answer.as_ref().map(|answer| answer.as_slice()))
                .done(),
            Self::Fetch { id, fetch } => {
                let writer = Writer::new(
                    kind::FETCH,
                    *id,
                    fetch
                        .url
                        .len()
                        .saturating_add(fetch.body.as_ref().map_or(0, Vec::len)),
                )
                .u8(match fetch.method {
                    Method::Get => 0,
                    Method::Post => 1,
                })
                .u8(match fetch.mode {
                    FetchMode::Api => 0,
                    FetchMode::Download => 1,
                    FetchMode::Picture => 2,
                })
                .bytes(fetch.url.as_bytes())
                .list(fetch.headers.iter().map(|(name, _)| name.as_bytes()))
                .list(fetch.headers.iter().map(|(_, value)| value.as_slice()))
                .optional(fetch.body.as_deref());
                match &fetch.deliver {
                    Some(delivery) => writer
                        .u8(1)
                        .bytes(delivery.name.as_bytes())
                        .optional(delivery.sha256.as_ref().map(<[u8; 32]>::as_slice)),
                    None => writer.u8(0),
                }
                .done()
            }
        }
    }

    /// Reads a message egit sent.
    ///
    /// # Errors
    ///
    /// When it isn't one.
    pub fn decode(message: &[u8]) -> Result<Self, Garbled> {
        let (kind, id, mut reader) = open(message)?;
        let decoded = match kind {
            kind::HELLO => Self::Hello {
                version: reader.u16()?,
                token: Zeroizing::new(reader.owned()?),
            },
            kind::START => {
                let program = reader.owned()?;
                let args = reader.list()?;
                let dir = reader.optional()?.map(<[u8]>::to_vec);
                let keys = reader.list()?;
                let values = reader.list()?;
                if keys.len() != values.len() {
                    return Err(Garbled("variables without values"));
                }
                let unset = reader.list()?;
                let stdin = reader.u8()? != 0;
                Self::Start {
                    id,
                    program: Program {
                        program,
                        args,
                        dir,
                        set: keys.into_iter().zip(values).collect(),
                        unset,
                        stdin,
                    },
                }
            }
            kind::STDIN => Self::Stdin {
                id,
                data: reader.owned()?,
            },
            kind::CLOSE_STDIN => Self::CloseStdin { id },
            kind::TERMINATE => Self::Terminate { id },
            kind::CREDIT => Self::Credit {
                id,
                bytes: reader.u32()?,
            },
            kind::ANSWER => Self::Answer {
                id,
                answer: reader
                    .optional()?
                    .map(|answer| Zeroizing::new(answer.to_vec())),
            },
            kind::FETCH => Self::Fetch {
                id,
                fetch: reader.fetch()?,
            },
            _ => return Err(Garbled("of no kind known")),
        };
        reader.end()?;
        Ok(decoded)
    }
}

impl FromRelay {
    /// The message as sent.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Welcome {
                version,
                home,
                host,
            } => Writer::new(kind::WELCOME, 0, home.len().saturating_add(host.len()))
                .u16(*version)
                .bytes(home)
                .bytes(host.as_bytes())
                .done(),
            Self::Refused { why } => Writer::new(kind::REFUSED, 0, why.len())
                .bytes(why.as_bytes())
                .done(),
            Self::Failed {
                id,
                missing,
                message,
            } => Writer::new(kind::FAILED, *id, message.len())
                .u8(match missing {
                    Missing::Nothing => 0,
                    Missing::Program => 1,
                    Missing::Folder => 2,
                })
                .bytes(message.as_bytes())
                .done(),
            Self::Stdout { id, data } => Writer::new(kind::STDOUT, *id, data.len())
                .bytes(data)
                .done(),
            Self::Stderr { id, data } => Writer::new(kind::STDERR, *id, data.len())
                .bytes(data)
                .done(),
            Self::Exited { id, exit } => {
                let writer = Writer::new(kind::EXITED, *id, 10);
                let writer = match exit.code() {
                    Some(code) => writer.u8(1).i32(code),
                    None => writer.u8(0),
                };
                match exit.signal() {
                    Some(signal) => writer.u8(1).i32(signal),
                    None => writer.u8(0),
                }
                .done()
            }
            Self::Prompt {
                id,
                kind,
                context,
                text,
            } => Writer::new(kind::PROMPT, *id, text.len())
                .u8(*kind)
                .optional(context.as_deref().map(str::as_bytes))
                .bytes(text.as_bytes())
                .done(),
            Self::Withdrawn { id } => Writer::new(kind::WITHDRAWN, *id, 0).done(),
            Self::Head {
                id,
                status,
                headers,
            } => Writer::new(kind::HEAD, *id, 256)
                .u16(*status)
                .list(headers.iter().map(|(name, _)| name.as_bytes()))
                .list(headers.iter().map(|(_, value)| value.as_bytes()))
                .done(),
            Self::Delivering { id, path } => Writer::new(kind::DELIVERING, *id, path.len())
                .bytes(path.as_bytes())
                .done(),
            Self::Delivered { id, bytes } => {
                Writer::new(kind::DELIVERED, *id, 8).u64(*bytes).done()
            }
        }
    }

    /// Reads a message the relay sent.
    ///
    /// # Errors
    ///
    /// When it isn't one.
    pub fn decode(message: &[u8]) -> Result<Self, Garbled> {
        let (kind, id, mut reader) = open(message)?;
        let decoded = match kind {
            kind::WELCOME => Self::Welcome {
                version: reader.u16()?,
                home: reader.owned()?,
                host: reader.text()?,
            },
            kind::REFUSED => Self::Refused {
                why: reader.text()?,
            },
            kind::FAILED => Self::Failed {
                id,
                missing: match reader.u8()? {
                    0 => Missing::Nothing,
                    1 => Missing::Program,
                    2 => Missing::Folder,
                    _ => return Err(Garbled("missing what")),
                },
                message: reader.text()?,
            },
            kind::STDOUT => Self::Stdout {
                id,
                data: reader.owned()?,
            },
            kind::STDERR => Self::Stderr {
                id,
                data: reader.owned()?,
            },
            kind::EXITED => {
                let code = match reader.u8()? {
                    0 => None,
                    _ => Some(reader.i32()?),
                };
                let signal = match reader.u8()? {
                    0 => None,
                    _ => Some(reader.i32()?),
                };
                Self::Exited {
                    id,
                    exit: Exit::new(code, signal),
                }
            }
            kind::PROMPT => Self::Prompt {
                id,
                kind: reader.u8()?,
                context: reader
                    .optional()?
                    .map(|context| String::from_utf8_lossy(context).into_owned()),
                text: reader.text()?,
            },
            kind::WITHDRAWN => Self::Withdrawn { id },
            kind::HEAD => {
                let status = reader.u16()?;
                let names = reader.list()?;
                let values = reader.list()?;
                if names.len() != values.len() {
                    return Err(Garbled("headers without values"));
                }
                let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
                Self::Head {
                    id,
                    status,
                    headers: names
                        .into_iter()
                        .zip(values)
                        .map(|(name, value)| (text(name), text(value)))
                        .collect(),
                }
            }
            kind::DELIVERING => Self::Delivering {
                id,
                path: reader.text()?,
            },
            kind::DELIVERED => Self::Delivered {
                id,
                bytes: reader.u64()?,
            },
            _ => return Err(Garbled("of no kind known")),
        };
        reader.end()?;
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_reads_back_as_it_was_written() {
        let to = [
            ToRelay::Hello {
                version: VERSION,
                token: Zeroizing::new(b"t0k3n".to_vec()),
            },
            ToRelay::Start {
                id: 7,
                program: Program {
                    program: b"git".to_vec(),
                    args: vec![b"log".to_vec(), Vec::new(), vec![0xff, 0, b'\n']],
                    dir: Some(b"/home/ada/repo".to_vec()),
                    set: vec![(b"LC_MESSAGES".to_vec(), b"C".to_vec())],
                    unset: vec![b"GIT_DIR".to_vec()],
                    stdin: true,
                },
            },
            ToRelay::Stdin {
                id: 7,
                data: b"contents abc\n".to_vec(),
            },
            ToRelay::CloseStdin { id: 7 },
            ToRelay::Terminate { id: 7 },
            ToRelay::Credit {
                id: 7,
                bytes: 65_536,
            },
            ToRelay::Answer {
                id: 3,
                answer: Some(Zeroizing::new(b"hunter2".to_vec())),
            },
            ToRelay::Answer {
                id: 4,
                answer: None,
            },
            ToRelay::Fetch {
                id: 5,
                fetch: Fetch {
                    method: Method::Post,
                    url: "https://gitlab.example.com/api/graphql".to_owned(),
                    headers: vec![(
                        "Authorization".to_owned(),
                        Zeroizing::new(b"Bearer glpat-x".to_vec()),
                    )],
                    body: Some(b"{}".to_vec()),
                    mode: FetchMode::Api,
                    deliver: None,
                },
            },
            ToRelay::Fetch {
                id: 6,
                fetch: Fetch {
                    method: Method::Get,
                    url: "https://objects.example.com/artifacts.zip?signed".to_owned(),
                    headers: Vec::new(),
                    body: None,
                    mode: FetchMode::Download,
                    deliver: Some(Delivery {
                        name: "artifacts – build.zip".to_owned(),
                        sha256: Some([7; 32]),
                    }),
                },
            },
            ToRelay::Fetch {
                id: 7,
                fetch: Fetch {
                    method: Method::Get,
                    url: "https://gitlab.example.com/api/v4/x".to_owned(),
                    headers: Vec::new(),
                    body: None,
                    mode: FetchMode::Download,
                    deliver: Some(Delivery {
                        name: "report.xml".to_owned(),
                        sha256: None,
                    }),
                },
            },
        ];
        for message in to {
            assert_eq!(ToRelay::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn every_answer_reads_back_as_it_was_written() {
        let from = [
            FromRelay::Welcome {
                version: VERSION,
                home: b"/home/ada".to_vec(),
                host: "devbox".to_owned(),
            },
            FromRelay::Refused {
                why: "That isn't this relay's token.".to_owned(),
            },
            FromRelay::Failed {
                id: 2,
                missing: Missing::Folder,
                message: "No such file or directory".to_owned(),
            },
            FromRelay::Stdout {
                id: 2,
                data: vec![0; 70_000],
            },
            FromRelay::Stderr {
                id: 2,
                data: b"fatal: not a git repository\n".to_vec(),
            },
            FromRelay::Exited {
                id: 2,
                exit: Exit::new(Some(128), None),
            },
            FromRelay::Exited {
                id: 3,
                exit: Exit::new(None, Some(15)),
            },
            FromRelay::Prompt {
                id: 1,
                kind: 2,
                context: Some("Fetching origin".to_owned()),
                text: "Password for 'https://ada@example.com': ".to_owned(),
            },
            FromRelay::Withdrawn { id: 1 },
            FromRelay::Head {
                id: 5,
                status: 200,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            },
            FromRelay::Delivering {
                id: 6,
                path: "/delivery/0123abcd".to_owned(),
            },
            FromRelay::Delivered {
                id: 6,
                bytes: 5 << 32,
            },
        ];
        for message in from {
            assert_eq!(FromRelay::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn a_garbled_message_says_so() {
        let start = ToRelay::Start {
            id: 1,
            program: Program {
                program: b"git".to_vec(),
                args: vec![b"status".to_vec()],
                ..Program::default()
            },
        }
        .encode();
        // Cut anywhere, it is cut short.
        for end in 0..start.len() {
            assert!(ToRelay::decode(&start[..end]).is_err(), "{end}");
        }
        let mut longer = start;
        longer.push(0);
        assert_eq!(
            ToRelay::decode(&longer),
            Err(Garbled("more than its fields"))
        );
        assert_eq!(
            ToRelay::decode(&[0x7f, 0, 0, 0, 0]),
            Err(Garbled("of no kind known"))
        );
        // A list claiming more entries than its message could hold.
        let lying = Writer::new(kind::START, 1, 0)
            .bytes(b"git")
            .u32(u32::MAX)
            .done();
        assert!(ToRelay::decode(&lying).is_err());
        // Nor a header's value.
        let fetch = Fetch {
            method: Method::Get,
            url: "https://api.github.com/user".to_owned(),
            headers: vec![(
                "Authorization".to_owned(),
                Zeroizing::new(b"Bearer hunter2".to_vec()),
            )],
            body: None,
            mode: FetchMode::Api,
            deliver: None,
        };
        assert!(!format!("{:?}", ToRelay::Fetch { id: 1, fetch }).contains("hunter2"));
        // A token never shows in debug output.
        let hello = ToRelay::Hello {
            version: VERSION,
            token: Zeroizing::new(b"hunter2".to_vec()),
        };
        assert!(!format!("{hello:?}").contains("hunter2"));
    }
}
