//! Parsing of Git LFS pointer files.
use bstr::ByteSlice;

/// The largest blob that can possibly be an LFS pointer file.
///
/// Larger blobs are never pointer candidates, mirroring `git-lfs`' `blobSizeCutoff`.
pub const MAX_POINTER_SIZE: usize = 1024;

/// The specification URLs accepted in a pointer's leading `version` line.
/// `hawser` is the pre-1.0 name of the `git-lfs` project and still accepted by it.
const VERSION_URLS: &[&[u8]] = &[
    b"https://git-lfs.github.com/spec/v1",
    b"https://hawser.github.com/spec/v1",
];

/// A parsed Git LFS pointer file, referring to content stored outside the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    /// The SHA-256 hash of the content the pointer refers to.
    pub oid: gix_hash::ObjectId,
    /// The size of the content in bytes.
    pub size: u64,
}

impl Pointer {
    /// Parse `blob` as an LFS pointer file, or return `None` if it isn't one.
    ///
    /// Like `git-lfs`, this requires the leading `version` line with a known specification URL,
    /// a `oid sha256:<hex>` line and a `size <decimal>` line, and considers only blobs of at
    /// most [`MAX_POINTER_SIZE`] bytes. Unknown keys are ignored.
    pub fn from_blob(blob: &[u8]) -> Option<Self> {
        if blob.len() > MAX_POINTER_SIZE {
            return None;
        }
        let mut lines = blob.lines();
        let version = lines.next()?.strip_prefix(b"version ")?;
        if !VERSION_URLS.contains(&version) {
            return None;
        }

        let mut oid = None;
        let mut size = None;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once_str(" ")?;
            match key {
                b"oid" => {
                    let hex = value.strip_prefix(b"sha256:")?;
                    let id = gix_hash::ObjectId::from_hex(hex).ok()?;
                    if id.kind() != gix_hash::Kind::Sha256 {
                        return None;
                    }
                    oid = Some(id);
                }
                b"size" => size = Some(value.to_str().ok()?.parse::<u64>().ok()?),
                _ => {}
            }
        }
        Some(Pointer { oid: oid?, size: size? })
    }
}
