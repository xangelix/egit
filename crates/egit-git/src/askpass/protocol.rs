//! What the helper and the window say to each other over the socket.
//!
//! A request is `EGAP`, the version, the 32-byte token, the prompt's kind, then the prompt and
//! its context, each a little-endian `u32` length and that many bytes. The response is a status
//! byte (`0` answered, `1` refused) and the answer, length-prefixed the same way. Every length is
//! bounded before anything is allocated for it.

use std::fmt;
use std::io::{self, Read, Write};

use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"EGAP";
const VERSION: u8 = 1;

/// The longest prompt accepted (git's and ssh's are a line).
pub(crate) const MAX_PROMPT: usize = 4096;
/// The longest context accepted.
pub(crate) const MAX_CONTEXT: usize = 1024;
/// The longest answer accepted.
pub(crate) const MAX_ANSWER: usize = 4096;

/// What a prompt asks for, which decides how it is shown and answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// Text that can be shown as it is typed, such as a user name.
    Visible,
    /// A secret: a password, passphrase, or PIN. Hidden as it is typed.
    Secret,
    /// A question answered by typing `yes` or `no`, such as ssh's question about a host it
    /// hasn't seen before.
    YesNo,
    /// A confirmation answered by accepting or refusing, with nothing to type
    /// (ssh-agent's `ssh-add -c` keys).
    Confirm,
    /// Information with nothing to answer, shown until the asker goes away (a security key
    /// waiting to be touched).
    Notice,
}

impl PromptKind {
    /// Its number, as a helper (or a relay) sends it.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Visible => 0,
            Self::Secret => 1,
            Self::YesNo => 2,
            Self::Confirm => 3,
            Self::Notice => 4,
        }
    }

    /// The kind numbered `code`, if any is.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Visible),
            1 => Some(Self::Secret),
            2 => Some(Self::YesNo),
            3 => Some(Self::Confirm),
            4 => Some(Self::Notice),
            _ => None,
        }
    }
}

/// The secret a helper proves it was started by egit with.
#[derive(Clone)]
pub(crate) struct Token([u8; 32]);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never in logs.
        f.write_str("Token(..)")
    }
}

impl Token {
    /// A fresh random token.
    pub(crate) fn random() -> io::Result<Self> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        Ok(Self(bytes))
    }

    /// The token as 64 hexadecimal digits, for the environment.
    pub(crate) fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            for nibble in [byte >> 4, byte & 0x0f] {
                hex.push(char::from(
                    DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'0'),
                ));
            }
        }
        hex
    }

    /// Reads the environment's form.
    pub(crate) fn from_hex(hex: &[u8]) -> Option<Self> {
        let (pairs, rest) = hex.as_chunks::<2>();
        if !rest.is_empty() || pairs.len() != 32 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let digit = |digit: u8| {
                char::from(digit)
                    .to_digit(16)
                    .and_then(|value| u8::try_from(value).ok())
            };
            *byte = (digit(high)? << 4) | digit(low)?;
        }
        Some(Self(bytes))
    }

    /// Compares in time independent of where the tokens differ.
    pub(crate) fn matches(&self, other: &[u8; 32]) -> bool {
        self.0
            .iter()
            .zip(other)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    }
}

/// A prompt as it travels from the helper to the window.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) token: [u8; 32],
    pub(crate) kind: PromptKind,
    pub(crate) prompt: Vec<u8>,
    pub(crate) context: Vec<u8>,
}

/// Writes a request.
pub(crate) fn write_request(
    out: &mut impl Write,
    token: &Token,
    kind: PromptKind,
    prompt: &[u8],
    context: &[u8],
) -> io::Result<()> {
    let prompt = prompt.get(..MAX_PROMPT).unwrap_or(prompt);
    let context = context.get(..MAX_CONTEXT).unwrap_or(context);
    let mut message = Vec::with_capacity(
        4_usize
            .saturating_add(1 + 32 + 1 + 4 + 4)
            .saturating_add(prompt.len())
            .saturating_add(context.len()),
    );
    message.extend_from_slice(MAGIC);
    message.push(VERSION);
    message.extend_from_slice(&token.0);
    message.push(kind.code());
    push_bytes(&mut message, prompt)?;
    push_bytes(&mut message, context)?;
    out.write_all(&message)?;
    out.flush()
}

/// Reads a request.
pub(crate) fn read_request(input: &mut impl Read) -> io::Result<Request> {
    let mut magic = [0_u8; 4];
    input.read_exact(&mut magic)?;
    let mut version = [0_u8; 1];
    input.read_exact(&mut version)?;
    if &magic != MAGIC || version != [VERSION] {
        return Err(invalid("not an egit askpass request"));
    }
    let mut token = [0_u8; 32];
    input.read_exact(&mut token)?;
    let mut kind = [0_u8; 1];
    input.read_exact(&mut kind)?;
    let [kind] = kind;
    let kind = PromptKind::from_code(kind).ok_or_else(|| invalid("unknown prompt kind"))?;
    let prompt = read_bytes(input, MAX_PROMPT)?;
    let context = read_bytes(input, MAX_CONTEXT)?;
    Ok(Request {
        token,
        kind,
        prompt,
        context,
    })
}

/// Writes a response: `Some(answer)` answered, `None` refused.
pub(crate) fn write_response(out: &mut impl Write, answer: Option<&[u8]>) -> io::Result<()> {
    let answer_bytes = answer.unwrap_or_default();
    if answer_bytes.len() > MAX_ANSWER {
        return Err(invalid("the answer is too long"));
    }
    let mut message = Zeroizing::new(Vec::with_capacity(answer_bytes.len().saturating_add(5)));
    message.push(u8::from(answer.is_none()));
    push_bytes(&mut message, answer_bytes)?;
    out.write_all(&message)?;
    out.flush()
}

/// Reads a response: `Some(answer)` answered, `None` refused.
pub(crate) fn read_response(input: &mut impl Read) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    let mut status = [0_u8; 1];
    input.read_exact(&mut status)?;
    let answer = Zeroizing::new(read_bytes(input, MAX_ANSWER)?);
    match status {
        [0] => Ok(Some(answer)),
        [1] => Ok(None),
        _ => Err(invalid("unknown response status")),
    }
}

fn push_bytes(message: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| invalid("too long"))?;
    message.extend_from_slice(&length.to_le_bytes());
    message.extend_from_slice(bytes);
    Ok(())
}

fn read_bytes(input: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_le_bytes(length)).map_err(|_| invalid("too long"))?;
    if length > limit {
        return Err(invalid("a field is longer than allowed"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [PromptKind; 5] = [
        PromptKind::Visible,
        PromptKind::Secret,
        PromptKind::YesNo,
        PromptKind::Confirm,
        PromptKind::Notice,
    ];

    #[test]
    fn requests_round_trip() {
        let token = Token::random().unwrap();
        for kind in KINDS {
            let mut wire = Vec::new();
            write_request(&mut wire, &token, kind, b"Password: ", b"Fetching origin").unwrap();
            let request = read_request(&mut wire.as_slice()).unwrap();
            assert!(token.matches(&request.token));
            assert_eq!(request.kind, kind);
            assert_eq!(request.prompt, b"Password: ");
            assert_eq!(request.context, b"Fetching origin");
        }
    }

    #[test]
    fn responses_round_trip() {
        let mut wire = Vec::new();
        write_response(&mut wire, Some(b"hunter2")).unwrap();
        assert_eq!(
            read_response(&mut wire.as_slice())
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(b"hunter2".as_slice())
        );
        let mut wire = Vec::new();
        write_response(&mut wire, None).unwrap();
        assert_eq!(read_response(&mut wire.as_slice()).unwrap(), None);
    }

    #[test]
    fn long_prompts_are_cut_and_long_fields_refused() {
        let token = Token::random().unwrap();
        let mut wire = Vec::new();
        write_request(&mut wire, &token, PromptKind::Secret, &[b'x'; 10_000], b"").unwrap();
        assert_eq!(
            read_request(&mut wire.as_slice()).unwrap().prompt.len(),
            MAX_PROMPT
        );

        let mut forged = Vec::new();
        forged.extend_from_slice(MAGIC);
        forged.push(VERSION);
        forged.extend_from_slice(&[0; 32]);
        forged.push(PromptKind::Secret.code());
        forged.extend_from_slice(&u32::MAX.to_le_bytes());
        let error = read_request(&mut forged.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        assert!(write_response(&mut Vec::new(), Some(&[0; MAX_ANSWER + 1])).is_err());
    }

    #[test]
    fn rejects_other_traffic() {
        assert!(read_request(&mut b"GET / HTTP/1.1\r\n\r\n".as_slice()).is_err());
        assert!(read_request(&mut b"EGAP".as_slice()).is_err());
        let mut wrong_version = b"EGAP\x09".to_vec();
        wrong_version.extend_from_slice(&[0; 40]);
        assert!(read_request(&mut wrong_version.as_slice()).is_err());
        assert!(read_response(&mut b"\x07\0\0\0\0".as_slice()).is_err());
    }

    #[test]
    fn tokens_round_trip_through_hex_and_compare() {
        let token = Token::random().unwrap();
        let hex = token.to_hex();
        assert_eq!(hex.len(), 64);
        let read = Token::from_hex(hex.as_bytes()).unwrap();
        assert!(read.matches(&token.0));
        let other = Token::random().unwrap();
        assert!(!other.matches(&token.0));
        assert!(Token::from_hex(b"not hex").is_none());
        assert!(Token::from_hex(&hex.as_bytes()[..62]).is_none());
    }
}
