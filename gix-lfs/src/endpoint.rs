//! Determine where the LFS objects of a repository can be fetched from.
use std::path::{Path, PathBuf};

use base64::Engine as _;
use bstr::BStr;

/// A resolved source for LFS objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Objects live in the LFS store of another repository on this machine
    /// (a path or `file://` remote).
    LocalRepository(PathBuf),
    /// A Git LFS HTTP(S) API endpoint.
    Http(Http),
    /// An SSH remote that provides the endpoint and authorization via `git-lfs-authenticate`.
    Ssh(Ssh),
}

/// An HTTP(S) Git LFS API endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Http {
    /// The base URL of the LFS API, without the trailing `/objects/batch`.
    pub url: String,
    /// Headers to send with every request, e.g. credentials taken from the URL's userinfo.
    pub headers: Vec<(String, String)>,
}

/// An SSH destination to obtain LFS API access from via `git-lfs-authenticate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ssh {
    /// The `[user@]host` destination as passed to `ssh`.
    pub destination: String,
    /// The port to connect to, if it isn't the default.
    pub port: Option<String>,
    /// The repository path on the server, as passed to `git-lfs-authenticate`.
    pub path: String,
}

/// The error returned by [`discover()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Failed to read configuration at {path:?}")]
    Config {
        path: PathBuf,
        source: Box<gix_config::file::init::from_paths::Error>,
    },
    #[error("No remote with a URL was found to determine the LFS endpoint from")]
    NoRemote,
}

/// Determine the LFS endpoint of the repository whose common git directory is `common_dir`
/// and whose work tree, if any, is at `work_dir`.
///
/// Configuration is read from `<common_dir>/config` and `<work_dir>/.lfsconfig`, the former
/// taking precedence, and evaluated like `git-lfs` does: `lfs.url` wins, then
/// `remote.<name>.lfsurl`, then the endpoint is derived from `remote.<name>.url`.
/// `remote` defaults to `origin` if it has a URL, and the first remote with a URL otherwise.
pub fn discover(common_dir: &Path, work_dir: Option<&Path>, remote: Option<&str>) -> Result<Endpoint, Error> {
    let mut files = Vec::new();
    for path in [
        Some(common_dir.join("config")),
        work_dir.map(|dir| dir.join(".lfsconfig")),
    ]
    .into_iter()
    .flatten()
    {
        if path.is_file() {
            let file =
                gix_config::File::from_path_no_includes(path.clone(), gix_config::Source::Local).map_err(|err| {
                    Error::Config {
                        path,
                        source: Box::new(err),
                    }
                })?;
            files.push(file);
        }
    }
    let lookup = |key: &str| {
        files
            .iter()
            .find_map(|file| file.string(key))
            .map(|value| bstr_to_string(value.as_ref()))
    };

    if let Some(url) = lookup("lfs.url") {
        return Ok(classify(&url, UrlKind::Explicit));
    }

    let remote_name = remote
        .map(ToOwned::to_owned)
        .or_else(|| lookup("remote.origin.url").map(|_| "origin".to_owned()))
        .or_else(|| first_remote_with_url(&files))
        .ok_or(Error::NoRemote)?;

    if let Some(url) = lookup(&format!("remote.{remote_name}.lfsurl")) {
        return Ok(classify(&url, UrlKind::Explicit));
    }
    let url = lookup(&format!("remote.{remote_name}.url")).ok_or(Error::NoRemote)?;
    Ok(classify(&url, UrlKind::Remote))
}

/// How a URL came to us, determining whether the standard LFS suffix is appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UrlKind {
    /// An explicitly configured LFS endpoint (`lfs.url`, `remote.<name>.lfsurl`), used verbatim.
    Explicit,
    /// A remote URL, from which the endpoint is derived by appending `[.git]/info/lfs`.
    Remote,
}

// URLs are case-sensitive, so the `.git` suffix check shouldn't fold case.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub(crate) fn classify(url: &str, kind: UrlKind) -> Endpoint {
    if let Some(rest) = url.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        return Endpoint::LocalRepository(rest.into());
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let (url, headers) = split_userinfo(url);
        let base = url.trim_end_matches('/');
        let url = match kind {
            UrlKind::Explicit => base.to_owned(),
            UrlKind::Remote => {
                let base = if base.ends_with(".git") {
                    base.to_owned()
                } else {
                    format!("{base}.git")
                };
                format!("{base}/info/lfs")
            }
        };
        return Endpoint::Http(Http { url, headers });
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (destination, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
                (host.to_owned(), Some(port.to_owned()))
            }
            _ => (authority.to_owned(), None),
        };
        return Endpoint::Ssh(Ssh {
            destination,
            port,
            path: path.to_owned(),
        });
    }
    if let Some((destination, path)) = split_scp_like(url) {
        return Endpoint::Ssh(Ssh {
            destination,
            port: None,
            path,
        });
    }
    Endpoint::LocalRepository(url.into())
}

/// Split `user@host:path` SCP-like remote URLs, avoiding Windows drive letters like `C:\repo`.
fn split_scp_like(url: &str) -> Option<(String, String)> {
    let (head, path) = url.split_once(':')?;
    if head.contains('/') || head.len() <= 1 || path.starts_with("//") {
        return None;
    }
    Some((head.to_owned(), path.to_owned()))
}

/// Move credentials embedded in `url`'s userinfo into an `Authorization: Basic` header.
fn split_userinfo(url: &str) -> (String, Vec<(String, String)>) {
    let Some(scheme_end) = url.find("://") else {
        return (url.to_owned(), Vec::new());
    };
    let after_scheme = &url[scheme_end + 3..];
    let authority_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    match authority.rfind('@') {
        Some(at) => {
            let userinfo = &authority[..at];
            let credentials = base64::engine::general_purpose::STANDARD.encode(userinfo);
            let header = ("Authorization".to_owned(), format!("Basic {credentials}"));
            let url = format!(
                "{}{}{}",
                &url[..scheme_end + 3],
                &authority[at + 1..],
                &after_scheme[authority_end..]
            );
            (url, vec![header])
        }
        None => (url.to_owned(), Vec::new()),
    }
}

fn first_remote_with_url(files: &[gix_config::File<'static>]) -> Option<String> {
    files.iter().find_map(|file| {
        file.sections_by_name("remote").and_then(|mut sections| {
            sections.find_map(|section| {
                let name = section.header().subsection_name()?;
                section.value("url").is_some().then(|| bstr_to_string(name))
            })
        })
    })
}

fn bstr_to_string(value: &BStr) -> String {
    use bstr::ByteSlice;
    value.to_str_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::{classify, Endpoint, Http, Ssh, UrlKind};

    #[test]
    fn remote_https_urls_get_the_lfs_suffix() {
        assert_eq!(
            classify("https://example.com/org/repo", UrlKind::Remote),
            Endpoint::Http(Http {
                url: "https://example.com/org/repo.git/info/lfs".into(),
                headers: vec![]
            })
        );
        assert_eq!(
            classify("https://example.com/org/repo.git", UrlKind::Remote),
            Endpoint::Http(Http {
                url: "https://example.com/org/repo.git/info/lfs".into(),
                headers: vec![]
            })
        );
    }

    #[test]
    fn explicit_urls_are_used_verbatim() {
        assert_eq!(
            classify("https://lfs.example.com/api/", UrlKind::Explicit),
            Endpoint::Http(Http {
                url: "https://lfs.example.com/api".into(),
                headers: vec![]
            })
        );
    }

    #[test]
    fn userinfo_becomes_a_basic_authorization_header() {
        let Endpoint::Http(http) = classify("https://user:pass@example.com/r.git", UrlKind::Remote) else {
            panic!("expected an http endpoint");
        };
        assert_eq!(http.url, "https://example.com/r.git/info/lfs");
        // base64("user:pass")
        assert_eq!(
            http.headers,
            vec![("Authorization".to_owned(), "Basic dXNlcjpwYXNz".to_owned())]
        );
    }

    #[test]
    fn ssh_and_scp_like_urls() {
        assert_eq!(
            classify("ssh://git@example.com:2222/org/repo.git", UrlKind::Remote),
            Endpoint::Ssh(Ssh {
                destination: "git@example.com".into(),
                port: Some("2222".into()),
                path: "org/repo.git".into(),
            })
        );
        assert_eq!(
            classify("git@example.com:org/repo.git", UrlKind::Remote),
            Endpoint::Ssh(Ssh {
                destination: "git@example.com".into(),
                port: None,
                path: "org/repo.git".into(),
            })
        );
    }

    #[test]
    fn paths_and_file_urls_are_local() {
        assert_eq!(
            classify("/srv/git/repo", UrlKind::Remote),
            Endpoint::LocalRepository("/srv/git/repo".into())
        );
        assert_eq!(
            classify("file:///srv/git/repo", UrlKind::Remote),
            Endpoint::LocalRepository("/srv/git/repo".into())
        );
        assert_eq!(
            classify("C:\\srv\\repo", UrlKind::Remote),
            Endpoint::LocalRepository("C:\\srv\\repo".into())
        );
    }
}
