//! The local LFS object store below `<git-dir>/lfs`.
use std::path::{Path, PathBuf};

use crate::pointer::Pointer;

/// The on-disk store for LFS objects of one repository.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// The error returned by [`Store::read()`] and [`Store::write()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Failed to access LFS object file at {path:?}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error(transparent)]
    Verify(#[from] VerifyError),
}

/// The error returned by [`verify()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum VerifyError {
    #[error("Object has {actual} bytes, but the pointer declares {expected}")]
    Size { expected: u64, actual: u64 },
    #[error("Object content hashes to {actual}, but the pointer declares {expected}")]
    Hash { expected: String, actual: String },
}

impl Store {
    /// Create the store of the repository whose *common* git directory is `common_dir`.
    pub fn at_common_dir(common_dir: &Path) -> Self {
        Store {
            root: common_dir.join("lfs"),
        }
    }

    /// Return the path at which the object `pointer` refers to would be stored.
    pub fn object_path(&self, pointer: &Pointer) -> PathBuf {
        let hex = pointer.oid.to_string();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..4]).join(hex)
    }

    /// Read the content `pointer` refers to, or `None` if it isn't present in the store.
    ///
    /// The content's size is validated against the pointer, but like `git-lfs`, content already
    /// in the store is not re-hashed.
    pub fn read(&self, pointer: &Pointer) -> Result<Option<Vec<u8>>, Error> {
        let path = self.object_path(pointer);
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(Error::Io { path, source: err }),
        };
        let actual = content.len() as u64;
        if actual != pointer.size {
            return Err(VerifyError::Size {
                expected: pointer.size,
                actual,
            }
            .into());
        }
        Ok(Some(content))
    }

    /// Verify `content` against `pointer` and write it into the store atomically.
    pub fn write(&self, pointer: &Pointer, content: &[u8]) -> Result<(), Error> {
        verify(pointer, content)?;

        let path = self.object_path(pointer);
        let io_err = |path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io { path, source }
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }

        let tmp_dir = self.root.join("tmp");
        std::fs::create_dir_all(&tmp_dir).map_err(io_err(&tmp_dir))?;
        let tmp_path = tmp_dir.join(format!("{}.{}", pointer.oid, std::process::id()));
        std::fs::write(&tmp_path, content).map_err(io_err(&tmp_path))?;
        std::fs::rename(&tmp_path, &path).map_err(io_err(&path))?;
        Ok(())
    }
}

/// Check that `content` matches the size and SHA-256 hash declared by `pointer`.
pub fn verify(pointer: &Pointer, content: &[u8]) -> Result<(), VerifyError> {
    let actual = content.len() as u64;
    if actual != pointer.size {
        return Err(VerifyError::Size {
            expected: pointer.size,
            actual,
        });
    }
    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha256);
    hasher.update(content);
    let actual = hasher
        .try_finalize()
        .expect("SHA-256 hashing is infallible, collision detection only applies to SHA-1");
    if actual != pointer.oid {
        return Err(VerifyError::Hash {
            expected: pointer.oid.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}
