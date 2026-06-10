//! Obtain LFS endpoint authorization from an SSH remote via `git-lfs-authenticate`.
use std::collections::HashMap;

use crate::endpoint;

#[derive(serde::Deserialize)]
struct Response {
    href: String,
    #[serde(default)]
    header: HashMap<String, String>,
}

/// The error returned by [`authenticate()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Failed to run 'ssh'")]
    Spawn { source: std::io::Error },
    #[error("'git-lfs-authenticate {path} download' failed on {destination}: {stderr}")]
    Failed {
        destination: String,
        path: String,
        stderr: String,
    },
    #[error("Failed to parse the response of 'git-lfs-authenticate' on {destination}")]
    Decode {
        destination: String,
        source: serde_json::Error,
    },
}

/// Run `ssh <destination> git-lfs-authenticate <path> download` to obtain the HTTP endpoint
/// and authorization headers for the repository behind the SSH remote `ssh`.
pub fn authenticate(ssh: &endpoint::Ssh) -> Result<endpoint::Http, Error> {
    let mut command = std::process::Command::new("ssh");
    command.arg("-o").arg("BatchMode=yes");
    if let Some(port) = &ssh.port {
        command.arg("-p").arg(port);
    }
    command
        .arg(&ssh.destination)
        .arg("git-lfs-authenticate")
        .arg(&ssh.path)
        .arg("download");

    let output = command.output().map_err(|err| Error::Spawn { source: err })?;
    if !output.status.success() {
        return Err(Error::Failed {
            destination: ssh.destination.clone(),
            path: ssh.path.clone(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let response: Response = serde_json::from_slice(&output.stdout).map_err(|err| Error::Decode {
        destination: ssh.destination.clone(),
        source: err,
    })?;
    Ok(endpoint::Http {
        url: response.href,
        headers: response.header.into_iter().collect(),
    })
}
