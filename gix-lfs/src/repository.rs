//! Locate the directories of a repository that are relevant for LFS.
use std::path::{Path, PathBuf};

/// The locations of a repository relevant to LFS operations.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The repository's own git directory.
    pub git_dir: PathBuf,
    /// Where shared repository state like the LFS object store and the configuration live.
    /// This differs from `git_dir` in linked work trees.
    pub common_dir: PathBuf,
    /// The root of the work tree, unless the repository is bare.
    pub work_dir: Option<PathBuf>,
}

/// The error returned by [`discover()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Failed to find a git repository at or above {path:?}")]
    Discover {
        path: PathBuf,
        source: gix_discover::upwards::Error,
    },
    #[error("Failed to read 'commondir' file at {path:?}")]
    CommonDir { path: PathBuf, source: std::io::Error },
}

/// Find the repository at or above `directory`, which may be a work tree or a git directory.
pub fn discover(directory: &Path) -> Result<Paths, Error> {
    let (path, _trust) = gix_discover::upwards(directory).map_err(|err| Error::Discover {
        path: directory.to_owned(),
        source: err,
    })?;
    let (git_dir, work_dir) = path.into_repository_and_work_tree_directories();

    let commondir_file = git_dir.join("commondir");
    let common_dir = match std::fs::read_to_string(&commondir_file) {
        Ok(relative) => git_dir.join(relative.trim_end()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => git_dir.clone(),
        Err(err) => {
            return Err(Error::CommonDir {
                path: commondir_file,
                source: err,
            });
        }
    };

    Ok(Paths {
        git_dir,
        common_dir,
        work_dir,
    })
}
