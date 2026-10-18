//! Commits as git stores them, parsed.

use bstr::{BStr, BString, ByteSlice as _};

use crate::object_id::ObjectId;

/// Who made a commit, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Their name.
    pub name: BString,
    /// Their email.
    pub email: BString,
    /// When, in seconds since the Unix epoch.
    pub time: i64,
    /// Their time zone's offset from UTC, in minutes.
    pub offset_minutes: i32,
}

/// Someone named in a message (a co-author): their name and email.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Person {
    /// Their name.
    pub name: BString,
    /// Their email.
    pub email: BString,
}

impl Person {
    /// Reads `Name <email>` (what a trailer names), or nothing when it isn't one.
    #[must_use]
    pub fn parse(text: &[u8]) -> Option<Self> {
        let open = text.find_byte(b'<')?;
        let close = text.rfind_byte(b'>')?;
        let name = text.get(..open)?.trim_ascii();
        let email = text.get(open.checked_add(1)?..close)?.trim_ascii();
        let rest = text.get(close.checked_add(1)?..)?.trim_ascii();
        (!name.is_empty() && !email.is_empty() && rest.is_empty()).then(|| Self {
            name: name.into(),
            email: email.into(),
        })
    }
}

/// The key of a trailer naming a co-author (compared in any case, as git compares keys).
pub const CO_AUTHORED_BY: &str = "Co-authored-by";

/// A commit object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// The tree it records.
    pub tree: ObjectId,
    /// Its parents, first parent first.
    pub parents: Vec<ObjectId>,
    /// Who wrote the change.
    pub author: Signature,
    /// Who made the commit.
    pub committer: Signature,
    /// The encoding of the message, when not UTF-8 (the `encoding` header).
    pub encoding: Option<BString>,
    /// Whether it carries a signature (OpenPGP, X.509, or SSH; not yet verified).
    pub signed: bool,
    /// The whole message.
    pub message: BString,
}

/// The bytes given aren't a commit egit can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a commit egit can read: {0}")]
pub struct CommitParseError(&'static str);

impl Commit {
    /// Parses a commit object's contents.
    ///
    /// # Errors
    ///
    /// When the tree, author, or committer is missing or malformed.
    pub fn parse(data: &[u8]) -> Result<Self, CommitParseError> {
        let (headers, message) = data.find(b"\n\n").map_or((data, &[][..]), |end| {
            (
                data.get(..end).unwrap_or_default(),
                data.get(end.saturating_add(2)..).unwrap_or_default(),
            )
        });
        let mut tree = None;
        let mut parents = Vec::new();
        let mut author = None;
        let mut committer = None;
        let mut encoding = None;
        let mut signed = false;
        for line in headers.split_str(b"\n") {
            // Continuation lines belong to the header above (a signature, a merged tag).
            if line.starts_with(b" ") {
                continue;
            }
            let (key, value) = line.split_once_str(b" ").unwrap_or((line, b""));
            match key {
                b"tree" => {
                    tree = Some(ObjectId::from_hex(value).ok_or(CommitParseError("bad tree"))?);
                }
                b"parent" => {
                    parents.push(ObjectId::from_hex(value).ok_or(CommitParseError("bad parent"))?);
                }
                b"author" => {
                    author = Some(parse_signature(value).ok_or(CommitParseError("bad author"))?);
                }
                b"committer" => {
                    committer =
                        Some(parse_signature(value).ok_or(CommitParseError("bad committer"))?);
                }
                b"encoding" => encoding = Some(BString::from(value)),
                b"gpgsig" | b"gpgsig-sha256" => signed = true,
                _ => {}
            }
        }
        Ok(Self {
            tree: tree.ok_or(CommitParseError("no tree"))?,
            parents,
            author: author.ok_or(CommitParseError("no author"))?,
            committer: committer.ok_or(CommitParseError("no committer"))?,
            encoding,
            signed,
            message: BString::from(message),
        })
    }

    /// The subject: the message's first paragraph, its lines joined by spaces (as git's `%s`).
    #[must_use]
    pub fn subject(&self) -> BString {
        let mut subject = BString::default();
        for line in self.message.lines() {
            let line = line.trim_ascii();
            if line.is_empty() {
                if subject.is_empty() {
                    continue;
                }
                break;
            }
            if !subject.is_empty() {
                subject.push(b' ');
            }
            subject.extend_from_slice(line);
        }
        subject
    }

    /// Who the message's `Co-authored-by` trailers name (in any case, as GitHub and git
    /// read them), in order, each once.
    #[must_use]
    pub fn co_authors(&self) -> Vec<Person> {
        let mut found: Vec<Person> = Vec::new();
        for line in self.message.lines() {
            let line = line.trim_ascii();
            let Some((key, value)) = line.split_once_str(b":") else {
                continue;
            };
            if key
                .trim_ascii()
                .eq_ignore_ascii_case(CO_AUTHORED_BY.as_bytes())
                && let Some(person) = Person::parse(value)
                && !found.contains(&person)
            {
                found.push(person);
            }
        }
        found
    }

    /// The message after the subject's paragraph, without the blank lines around it.
    #[must_use]
    pub fn body(&self) -> &BStr {
        let message = self.message.as_slice();
        let start = message.find_not_byteset(b"\n \t").unwrap_or(message.len());
        let rest = message.get(start..).unwrap_or_default();
        let body = rest
            .find(b"\n\n")
            .map_or(&[][..], |end| rest.get(end..).unwrap_or_default());
        BStr::new(body.trim_ascii())
    }
}

/// Parses `Name <email> 1700000000 +0100`.
fn parse_signature(value: &[u8]) -> Option<Signature> {
    let open = value.find_byte(b'<')?;
    let close = value.rfind_byte(b'>')?;
    if close < open {
        return None;
    }
    let name = value.get(..open)?.trim_ascii_end();
    let email = value.get(open.checked_add(1)?..close)?;
    let mut when = value
        .get(close.checked_add(1)?..)?
        .split_str(b" ")
        .filter(|field| !field.is_empty());
    let time = when.next()?.to_str().ok()?.parse().ok()?;
    let offset_minutes = when.next().map_or(Some(0), parse_offset)?;
    Some(Signature {
        name: BString::from(name),
        email: BString::from(email),
        time,
        offset_minutes,
    })
}

/// Parses a time zone offset such as `+0100` or `-0530` into minutes.
fn parse_offset(offset: &[u8]) -> Option<i32> {
    let (sign, digits) = match offset.split_first()? {
        (b'+', digits) => (1, digits),
        (b'-', digits) => (-1, digits),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let number: i32 = digits.to_str().ok()?.parse().ok()?;
    let hours = number.checked_div(100)?;
    let minutes = number.checked_rem(100)?;
    hours
        .checked_mul(60)?
        .checked_add(minutes)?
        .checked_mul(sign)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    const PARENT: &str = "ce013625030ba8dba906f756967f9e9ca394464a";

    fn commit(extra_headers: &str, message: &str) -> Vec<u8> {
        format!(
            "tree {TREE}\n{extra_headers}author Zoë Example <zoe@example.com> 1700000000 +0130\n\
             committer C. Ommitter <c@example.com> 1700000600 -0800\n\n{message}"
        )
        .into_bytes()
    }

    #[test]
    fn reads_a_root_commit() {
        let parsed = Commit::parse(&commit("", "Start\n")).unwrap();
        assert_eq!(parsed.tree.to_string(), TREE);
        assert_eq!(parsed.parents, []);
        assert_eq!(parsed.author.name, "Zoë Example");
        assert_eq!(parsed.author.email, "zoe@example.com");
        assert_eq!(parsed.author.time, 1_700_000_000);
        assert_eq!(parsed.author.offset_minutes, 90);
        assert_eq!(parsed.committer.offset_minutes, -480);
        assert!(!parsed.signed);
        assert_eq!(parsed.subject(), "Start");
        assert_eq!(parsed.body(), "");
    }

    #[test]
    fn reads_parents_signatures_and_encodings() {
        let headers = format!(
            "parent {PARENT}\nparent {TREE}\nencoding ISO-8859-1\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\n"
        );
        let parsed = Commit::parse(&commit(&headers, "Merge\n")).unwrap();
        assert_eq!(parsed.parents.len(), 2);
        assert_eq!(parsed.parents[0].to_string(), PARENT);
        assert_eq!(parsed.encoding, Some(BString::from("ISO-8859-1")));
        assert!(parsed.signed);
    }

    #[test]
    fn co_authors_come_from_their_trailers_in_any_case() {
        let parsed = Commit::parse(&commit(
            "",
            "Pair on it\n\nWhy.\n\nCo-authored-by: Ada Lovelace <ada@example.com>\n\
             co-authored-by:  Alan Turing  <alan@example.com> \n\
             Co-Authored-By: Ada Lovelace <ada@example.com>\n\
             Co-authored-by: nobody\n\
             Signed-off-by: Zoë Example <zoe@example.com>\n",
        ))
        .unwrap();
        let names: Vec<_> = parsed
            .co_authors()
            .into_iter()
            .map(|person| (person.name.to_string(), person.email.to_string()))
            .collect();
        assert_eq!(
            names,
            [
                ("Ada Lovelace".to_owned(), "ada@example.com".to_owned()),
                ("Alan Turing".to_owned(), "alan@example.com".to_owned()),
            ]
        );
        assert_eq!(Person::parse(b"Name <a@b> trailing"), None);
        assert_eq!(Person::parse(b" <a@b>"), None);
    }

    #[test]
    fn the_subject_is_the_first_paragraph_on_one_line() {
        let parsed = Commit::parse(&commit(
            "",
            "\nFix the thing\nfor real\n\nBecause.\n\nMore.\n",
        ))
        .unwrap();
        assert_eq!(parsed.subject(), "Fix the thing for real");
        assert_eq!(parsed.body(), "Because.\n\nMore.");
    }

    #[test]
    fn refuses_what_isnt_a_commit() {
        assert!(Commit::parse(b"").is_err());
        assert!(Commit::parse(b"tree nothex\n\nx").is_err());
        let no_author = format!("tree {TREE}\ncommitter A <a> 1 +0000\n\nx");
        assert!(Commit::parse(no_author.as_bytes()).is_err());
        let bad_time =
            format!("tree {TREE}\nauthor A <a> soon +0000\ncommitter A <a> 1 +0000\n\nx");
        assert!(Commit::parse(bad_time.as_bytes()).is_err());
    }

    #[test]
    fn offsets() {
        assert_eq!(parse_offset(b"+0000"), Some(0));
        assert_eq!(parse_offset(b"-0530"), Some(-330));
        assert_eq!(parse_offset(b"+1400"), Some(840));
        assert_eq!(parse_offset(b"0100"), None);
        assert_eq!(parse_offset(b"+1"), None);
    }
}
