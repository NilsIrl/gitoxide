//! Fetch LFS objects from the store of another repository on the same machine.
use std::path::{Path, PathBuf};

use crate::pointer::Pointer;
use crate::{repository, store};

/// The error returned by [`fetch()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Repository(#[from] repository::Error),
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("Object {oid} was not found in the LFS store of {repo:?}")]
    NotFound { oid: String, repo: PathBuf },
}

/// Read the content `pointer` refers to from the LFS store of the repository at `remote_repo`.
pub fn fetch(remote_repo: &Path, pointer: &Pointer) -> Result<Vec<u8>, Error> {
    let paths = repository::discover(remote_repo)?;
    let store = store::Store::at_common_dir(&paths.common_dir);
    store.read(pointer)?.ok_or_else(|| Error::NotFound {
        oid: pointer.oid.to_string(),
        repo: remote_repo.to_owned(),
    })
}
