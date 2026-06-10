use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::endpoint::Endpoint;
use crate::pointer::Pointer;
use crate::{batch, endpoint, local, repository, ssh, store};

/// Options for [`smudge()`].
#[derive(Debug, Clone)]
pub struct Options {
    /// The maximum time a network fetch may go without receiving data before it is aborted.
    ///
    /// This deliberately bounds inactivity rather than total transfer time, so large objects
    /// on slow connections can still complete.
    pub network_idle_timeout: Duration,
    /// The maximum time to wait for a connection to the LFS server to be established.
    pub connect_timeout: Duration,
    /// The remote to fetch from, defaulting to `origin` or the only remote with a URL.
    pub remote: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            network_idle_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            remote: None,
        }
    }
}

/// The error returned by [`smudge()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Repository(#[from] repository::Error),
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error(transparent)]
    Endpoint(#[from] endpoint::Error),
    #[error(transparent)]
    Local(#[from] local::Error),
    #[error(transparent)]
    Batch(#[from] batch::Error),
    #[error(transparent)]
    Ssh(#[from] ssh::Error),
}

/// Turn `blob` of the repository at `repo` into the content it represents, like `git lfs smudge`.
///
/// If `blob` is not an LFS pointer it is returned unchanged. Otherwise the object it refers to
/// is returned, either straight from the repository's LFS store or by fetching it into the store
/// first, verifying its size and SHA-256 hash.
pub fn smudge(repo: &Path, blob: Vec<u8>, opts: &Options) -> Result<Vec<u8>, Error> {
    let Some(pointer) = Pointer::from_blob(&blob) else {
        return Ok(blob);
    };

    let paths = repository::discover(repo)?;
    let store = store::Store::at_common_dir(&paths.common_dir);
    if let Some(content) = store.read(&pointer)? {
        return Ok(content);
    }

    let endpoint = endpoint::discover(&paths.common_dir, paths.work_dir.as_deref(), opts.remote.as_deref())?;
    let content = match endpoint {
        Endpoint::LocalRepository(path) => {
            let path = absolutize(path, &paths);
            local::fetch(&path, &pointer)?
        }
        Endpoint::Http(http) => batch::download(&agent(opts), &http.url, &http.headers, &pointer)?,
        Endpoint::Ssh(destination) => {
            let http = ssh::authenticate(&destination)?;
            batch::download(&agent(opts), &http.url, &http.headers, &pointer)?
        }
    };

    store.write(&pointer, &content)?;
    Ok(content)
}

/// Resolve a relative local remote path the way git does: against the repository root.
fn absolutize(path: PathBuf, paths: &repository::Paths) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    let base = paths
        .work_dir
        .as_deref()
        .or_else(|| paths.common_dir.parent())
        .unwrap_or(&paths.common_dir);
    base.join(path)
}

fn agent(opts: &Options) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(opts.connect_timeout)
        .timeout_read(opts.network_idle_timeout)
        .timeout_write(opts.network_idle_timeout)
        .build()
}
