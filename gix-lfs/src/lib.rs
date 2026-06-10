//! Read Git LFS pointer files and materialize the content they refer to, similar to `git lfs smudge`.
//!
//! The entry point is [`smudge()`]: given a blob from a repository, it returns the blob unchanged
//! unless it is a valid LFS pointer, in which case the referenced object is returned instead —
//! served from the repository's local LFS object store when present, and otherwise fetched and
//! stored first.
//!
//! Objects can be fetched from:
//!
//! * the LFS store of another repository on the same machine (paths and `file://` remotes),
//! * an HTTP(S) endpoint speaking the [Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md),
//!   discovered from `lfs.url`, `remote.<name>.lfsurl` or derived from the remote URL,
//! * an SSH remote, with endpoint and authorization obtained via `git-lfs-authenticate`.
//!
//! ### Limitations
//!
//! * Download only — the `upload` operation isn't implemented.
//! * Only the `basic` transfer adapter is supported.
//! * HTTP authentication is limited to credentials embedded in the endpoint URL and headers
//!   provided by `git-lfs-authenticate`; git credential helpers aren't consulted yet.
//! * `.lfsconfig` is only read from the work tree, not from `HEAD` in checkout-less clones.
#![deny(missing_docs, rust_2018_idioms)]
#![forbid(unsafe_code)]

pub mod batch;
pub mod endpoint;
pub mod local;
pub mod pointer;
pub mod repository;
pub mod ssh;
pub mod store;

mod smudge;
pub use smudge::{Error, Options, smudge};
