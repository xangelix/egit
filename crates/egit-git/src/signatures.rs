//! Whether commits' signatures hold, checked as `git log --show-signature` checks them: each by
//! the program its format names (gpg, gpgsm, ssh-keygen, or one the user configured), against
//! the keys and allowed signers the user trusts.

use bstr::{BString, ByteSlice as _};

use crate::cancel::CancelToken;
use crate::error::GitError;
use crate::git::Git;
use crate::object_id::ObjectId;
use crate::repo::Repository;

/// What checking a commit's signature found (git's `%G?`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A good signature by a key the user trusts.
    Good,
    /// A good signature by a key whose owner isn't known to be who it says.
    Untrusted,
    /// A signature that doesn't match the commit: it was changed, or forged.
    Bad,
    /// A good signature that has expired.
    ExpiredSignature,
    /// A good signature by a key that has expired.
    ExpiredKey,
    /// A good signature by a key that was revoked.
    RevokedKey,
    /// A signature that couldn't be checked: its key is missing, or its program failed.
    Unchecked,
    /// No signature.
    Unsigned,
}

impl Verdict {
    /// The verdict `%G?` prints.
    const fn from_letter(letter: u8) -> Option<Self> {
        Some(match letter {
            b'G' => Self::Good,
            b'U' => Self::Untrusted,
            b'B' => Self::Bad,
            b'X' => Self::ExpiredSignature,
            b'Y' => Self::ExpiredKey,
            b'R' => Self::RevokedKey,
            b'E' => Self::Unchecked,
            b'N' => Self::Unsigned,
            _ => return None,
        })
    }
}

/// A commit's signature, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureCheck {
    /// The commit.
    pub id: ObjectId,
    /// What checking it found.
    pub verdict: Verdict,
    /// Who signed it, as the key names them (empty when unsigned or unknown).
    pub signer: BString,
    /// The key it was signed with: an OpenPGP key's id, an X.509 certificate's fingerprint, an
    /// SSH key's.
    pub key: BString,
}

/// Checks the signatures of `ids` (each signed one by its program: this takes a while for
/// many), in the order given.
///
/// # Errors
///
/// When git fails, is cancelled, or prints something unexpected.
pub async fn check_signatures(
    git: &Git,
    repository: &Repository,
    ids: &[ObjectId],
    cancel: &CancelToken,
) -> Result<Vec<SignatureCheck>, GitError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = Vec::with_capacity(ids.len().saturating_mul(65));
    for id in ids {
        stdin.extend_from_slice(id.to_string().as_bytes());
        stdin.push(b'\n');
    }
    let listed = git
        .read("log")
        // The checks' own output would otherwise be mixed into the format's.
        .config("log.showSignature", "false")
        .args([
            "--no-walk=unsorted",
            "--stdin",
            "-z",
            "--format=%H%x1f%G?%x1f%GS%x1f%GK",
        ])
        .current_dir(repository.work_tree())
        .stdin(stdin)
        .cancel_token(cancel)
        .output()
        .await?;
    parse_checks(&listed.stdout)
}

/// Parses `%H %G? %GS %GK` records, separated by NUL, their fields by US.
fn parse_checks(output: &[u8]) -> Result<Vec<SignatureCheck>, GitError> {
    output
        .split_str(b"\0")
        .map(<[u8]>::trim_ascii)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let malformed = || {
                GitError::unexpected(
                    "log",
                    format!("{} isn't a signature check", record.as_bstr()),
                )
            };
            let mut fields = record.split_str(b"\x1f");
            let id = fields
                .next()
                .and_then(ObjectId::from_hex)
                .ok_or_else(malformed)?;
            let verdict = fields
                .next()
                .and_then(|field| match field {
                    [letter] => Verdict::from_letter(*letter),
                    _ => None,
                })
                .ok_or_else(malformed)?;
            Ok(SignatureCheck {
                id,
                verdict,
                signer: fields.next().map_or_default(BString::from),
                key: fields.next().map_or_default(BString::from),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_commits_check() {
        let a = "ce013625030ba8dba906f756967f9e9ca394464a";
        let b = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        let output = format!(
            "{a}\x1fG\x1fegit tests <tests@egit.invalid>\x1f72A030ECE5D5B6D6\0\n{b}\x1fN\x1f\x1f\0"
        );
        let checks = parse_checks(output.as_bytes()).unwrap();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].verdict, Verdict::Good);
        assert_eq!(checks[0].signer, "egit tests <tests@egit.invalid>");
        assert_eq!(checks[0].key, "72A030ECE5D5B6D6");
        assert_eq!(checks[1].verdict, Verdict::Unsigned);
        assert!(parse_checks(format!("{a}\x1fZ\x1f\x1f\0").as_bytes()).is_err());
        assert!(parse_checks(b"nonsense\0").is_err());
    }
}
