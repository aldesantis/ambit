//! SHA-256, hex-encoded, over bytes or over a directory tree.

use std::fmt::Write as _;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::util::fs::{EntryKind, read_link_text, walk_tree};

/// The prefix every [`tree_digest`] value carries, naming the hash it was made with.
pub const TREE_DIGEST_PREFIX: &str = "sha256-";

/// The lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Lowercase hex of a finished digest, such as `Sha256::finalize`'s output.
///
/// Written by hand rather than through `LowerHex`, which sha2 0.11's output array does not promise.
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }

    text
}

/// Feeds one length-prefixed field: the length as a big-endian `u64`, then the bytes.
fn field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// The content digest of a directory tree, as [`TREE_DIGEST_PREFIX`] followed by lowercase hex.
///
/// This is the one definition of the digest `ambit.lock` records for a skill or a script hook and
/// `.ambit/state.json` records for a copied directory. The recipe:
///
/// 1. List every entry under `dir` that is not a directory, by its `/`-separated path relative to
///    `dir`, sorted by [`js_cmp`](crate::util::cmp::js_cmp). Directories contribute nothing of
///    their own, so an empty one is invisible. Entries that are neither a regular file nor a
///    symlink (a socket, a device) are skipped.
/// 2. For each entry, in that order, feed SHA-256 a one-byte tag (`F` for a regular file, `L` for a
///    symlink), then the relative path as a field, then the payload as a field. A file's payload
///    is its bytes. A symlink's is its link text, `/`-separated, read and never followed.
/// 3. A field is the byte length as a big-endian `u64` followed by the bytes, so no two different
///    trees feed the same stream.
///
/// File modes, timestamps and ownership are not part of it: the same commit checks out with
/// different modes under different umasks and on Windows, and the digest has to match everywhere.
///
/// # Errors
///
/// Any I/O error from listing the tree or reading an entry, `NotFound` included.
pub fn tree_digest(dir: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();

    for entry in walk_tree(dir)? {
        let path = dir.join(&entry.relative);
        let (tag, payload) = match entry.kind {
            EntryKind::File => (b'F', std::fs::read(&path)?),
            EntryKind::Symlink => (b'L', read_link_text(&path)?.into_bytes()),
            EntryKind::Dir | EntryKind::Missing | EntryKind::Other => continue,
        };

        hasher.update([tag]);
        field(&mut hasher, entry.relative.as_bytes());
        field(&mut hasher, &payload);
    }

    Ok(format!("{TREE_DIGEST_PREFIX}{}", hex(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fs::{mkdir_p, write_text};

    #[test]
    fn hashes_the_empty_string() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn digests_an_empty_tree_as_the_empty_stream() {
        let dir = tempfile::tempdir().unwrap();

        mkdir_p(&dir.path().join("empty")).unwrap();

        assert_eq!(
            tree_digest(dir.path()).unwrap(),
            format!("{TREE_DIGEST_PREFIX}{}", sha256_hex(b""))
        );
    }

    #[test]
    fn pins_the_recipe() {
        let dir = tempfile::tempdir().unwrap();

        mkdir_p(&dir.path().join("sub")).unwrap();
        write_text(&dir.path().join("sub/b.txt"), "bee").unwrap();
        write_text(&dir.path().join("a.md"), "ay").unwrap();

        let mut expected = Vec::new();

        for (path, bytes) in [("a.md", "ay"), ("sub/b.txt", "bee")] {
            expected.push(b'F');
            expected.extend((path.len() as u64).to_be_bytes());
            expected.extend(path.as_bytes());
            expected.extend((bytes.len() as u64).to_be_bytes());
            expected.extend(bytes.as_bytes());
        }

        assert_eq!(
            tree_digest(dir.path()).unwrap(),
            format!("{TREE_DIGEST_PREFIX}{}", sha256_hex(&expected))
        );
    }

    #[test]
    fn separates_paths_from_contents() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();

        write_text(&one.path().join("ab"), "c").unwrap();
        write_text(&two.path().join("a"), "bc").unwrap();

        assert_ne!(
            tree_digest(one.path()).unwrap(),
            tree_digest(two.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn hashes_a_symlink_by_its_text() {
        use crate::util::fs::symlink_file;

        let linked = tempfile::tempdir().unwrap();
        let copied = tempfile::tempdir().unwrap();

        write_text(&linked.path().join("target"), "t").unwrap();
        symlink_file(Path::new("target"), &linked.path().join("link")).unwrap();
        write_text(&copied.path().join("target"), "t").unwrap();
        write_text(&copied.path().join("link"), "t").unwrap();

        assert_ne!(
            tree_digest(linked.path()).unwrap(),
            tree_digest(copied.path()).unwrap()
        );
    }
}
