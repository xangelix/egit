//! Object names: SHA-1 or SHA-256 hashes.

use std::fmt;

/// The hash function a repository names its objects with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectFormat {
    /// SHA-1: 20 bytes, 40 hexadecimal digits. Every repository before SHA-256 existed.
    Sha1,
    /// SHA-256: 32 bytes, 64 hexadecimal digits.
    Sha256,
}

impl ObjectFormat {
    /// The name git uses (`extensions.objectFormat`, `--show-object-format`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    /// Reads the name git uses.
    #[must_use]
    pub const fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"sha1" => Some(Self::Sha1),
            b"sha256" => Some(Self::Sha256),
            _ => None,
        }
    }

    /// How many bytes a name takes.
    #[must_use]
    pub const fn byte_len(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
        }
    }

    /// How many hexadecimal digits a name takes.
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

impl fmt::Display for ObjectFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The name of a git object.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectId {
    /// A SHA-1 name.
    Sha1([u8; 20]),
    /// A SHA-256 name.
    Sha256([u8; 32]),
}

impl ObjectId {
    /// Reads a full name in hexadecimal (either case); its length decides the format.
    #[must_use]
    pub fn from_hex(hex: &[u8]) -> Option<Self> {
        match hex.len() {
            40 => decode(hex).map(Self::Sha1),
            64 => decode(hex).map(Self::Sha256),
            _ => None,
        }
    }

    /// Reads a name's raw bytes; their length decides the format.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes.len() {
            20 => bytes.try_into().ok().map(Self::Sha1),
            32 => bytes.try_into().ok().map(Self::Sha256),
            _ => None,
        }
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Sha1(bytes) => bytes,
            Self::Sha256(bytes) => bytes,
        }
    }

    /// Its format.
    #[must_use]
    pub const fn format(&self) -> ObjectFormat {
        match self {
            Self::Sha1(_) => ObjectFormat::Sha1,
            Self::Sha256(_) => ObjectFormat::Sha256,
        }
    }

    /// The first `digits` hexadecimal digits (all of them if it has fewer), as people quote
    /// commits.
    #[must_use]
    pub const fn abbreviated(&self, digits: usize) -> Abbreviated<'_> {
        Abbreviated { id: self, digits }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, self.as_bytes(), usize::MAX)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({self})")
    }
}

/// An [`ObjectId`] shown with fewer digits.
#[derive(Clone, Copy, Debug)]
pub struct Abbreviated<'a> {
    id: &'a ObjectId,
    digits: usize,
}

impl fmt::Display for Abbreviated<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, self.id.as_bytes(), self.digits)
    }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8], digits: usize) -> fmt::Result {
    let mut written = 0_usize;
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            if written == digits {
                return Ok(());
            }
            let digit = HEX_DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'?');
            fmt::Write::write_char(f, char::from(digit))?;
            written = written.saturating_add(1);
        }
    }
    Ok(())
}

/// Decodes exactly `N` bytes from `2 * N` hexadecimal digits.
fn decode<const N: usize>(hex: &[u8]) -> Option<[u8; N]> {
    let (pairs, rest) = hex.as_chunks::<2>();
    if !rest.is_empty() || pairs.len() != N {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = (nibble(high)? << 4) | nibble(low)?;
    }
    Some(bytes)
}

const fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(digit.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(digit.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const SHA256: &str = "2cf8d83d9ee29543b34a87727421fdecb7e3f3a183d337639025de576db9ebb4";

    #[test]
    fn reads_both_formats_by_length() {
        let sha1 = ObjectId::from_hex(SHA1.as_bytes()).unwrap();
        assert_eq!(sha1.format(), ObjectFormat::Sha1);
        assert_eq!(sha1.as_bytes().len(), 20);
        assert_eq!(sha1.to_string(), SHA1);

        let sha256 = ObjectId::from_hex(SHA256.as_bytes()).unwrap();
        assert_eq!(sha256.format(), ObjectFormat::Sha256);
        assert_eq!(sha256.as_bytes().len(), 32);
        assert_eq!(sha256.to_string(), SHA256);
    }

    #[test]
    fn raw_bytes_round_trip() {
        for hex in [SHA1, SHA256] {
            let id = ObjectId::from_hex(hex.as_bytes()).unwrap();
            assert_eq!(ObjectId::from_bytes(id.as_bytes()), Some(id));
        }
        assert_eq!(ObjectId::from_bytes(&[0; 21]), None);
        assert_eq!(ObjectId::from_bytes(&[]), None);
    }

    #[test]
    fn accepts_upper_case_and_prints_lower_case() {
        let id = ObjectId::from_hex(SHA1.to_uppercase().as_bytes()).unwrap();
        assert_eq!(id.to_string(), SHA1);
    }

    #[test]
    fn rejects_anything_else() {
        assert_eq!(ObjectId::from_hex(b""), None);
        assert_eq!(ObjectId::from_hex(&SHA1.as_bytes()[..39]), None);
        assert_eq!(ObjectId::from_hex(format!("{SHA1}0").as_bytes()), None);
        assert_eq!(ObjectId::from_hex(SHA1.replace('c', "g").as_bytes()), None);
    }

    #[test]
    fn abbreviates() {
        let id = ObjectId::from_hex(SHA1.as_bytes()).unwrap();
        assert_eq!(id.abbreviated(7).to_string(), "ce01362");
        assert_eq!(id.abbreviated(0).to_string(), "");
        assert_eq!(id.abbreviated(100).to_string(), SHA1);
    }

    #[test]
    fn format_names_round_trip() {
        for format in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
            assert_eq!(
                ObjectFormat::from_name(format.name().as_bytes()),
                Some(format)
            );
            assert_eq!(format.hex_len(), format.byte_len() * 2);
        }
        assert_eq!(ObjectFormat::from_name(b"md5"), None);
    }
}
