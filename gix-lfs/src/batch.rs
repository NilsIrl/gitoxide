//! A minimal client for the download side of the
//! [Git LFS Batch API](https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md).
use std::collections::HashMap;
use std::io::Read;

use crate::pointer::Pointer;

/// The MIME type of Git LFS API requests and responses.
pub const MIME: &str = "application/vnd.git-lfs+json";

#[derive(serde::Serialize)]
struct BatchRequest<'a> {
    operation: &'static str,
    transfers: &'a [&'a str],
    objects: Vec<RequestObject>,
}

#[derive(serde::Serialize)]
struct RequestObject {
    oid: String,
    size: u64,
}

#[derive(serde::Deserialize)]
struct BatchResponse {
    objects: Vec<ResponseObject>,
}

#[derive(serde::Deserialize)]
struct ResponseObject {
    oid: String,
    #[serde(default)]
    actions: Option<Actions>,
    #[serde(default)]
    error: Option<ObjectError>,
}

#[derive(serde::Deserialize)]
struct Actions {
    #[serde(default)]
    download: Option<Action>,
}

#[derive(serde::Deserialize)]
struct Action {
    href: String,
    #[serde(default)]
    header: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct ObjectError {
    code: i64,
    message: String,
}

/// The error returned by [`download()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("Failed to call the LFS batch API at {url}")]
    Http { url: String, source: Box<ureq::Error> },
    #[error("Failed to parse the LFS batch API response from {url}")]
    Decode { url: String, source: serde_json::Error },
    #[error("The LFS server returned no entry for object {oid}")]
    MissingObject { oid: String },
    #[error("The LFS server reported error {code} for object {oid}: {message}")]
    Object { oid: String, code: i64, message: String },
    #[error("The LFS server offered no download action for object {oid}")]
    NoDownloadAction { oid: String },
    #[error("Failed to download object {oid} from {url}")]
    Download {
        oid: String,
        url: String,
        source: Box<ureq::Error>,
    },
    #[error("Failed to read the content of object {oid} from {url}")]
    ReadContent {
        oid: String,
        url: String,
        source: std::io::Error,
    },
}

/// Download the object `pointer` refers to from the LFS API at `base_url`,
/// sending `headers` with each request.
///
/// The returned content is size-capped by the pointer's declared size but not yet verified;
/// use [`crate::store::verify()`] or [`crate::store::Store::write()`] on the result.
pub fn download(
    agent: &ureq::Agent,
    base_url: &str,
    headers: &[(String, String)],
    pointer: &Pointer,
) -> Result<Vec<u8>, Error> {
    let oid = pointer.oid.to_string();
    let url = format!("{}/objects/batch", base_url.trim_end_matches('/'));

    let mut request = agent.post(&url).set("Accept", MIME).set("Content-Type", MIME);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let body = serde_json::to_string(&BatchRequest {
        operation: "download",
        transfers: &["basic"],
        objects: vec![RequestObject {
            oid: oid.clone(),
            size: pointer.size,
        }],
    })
    .expect("serialization of plain structs never fails");

    let response = request.send_string(&body).map_err(|err| Error::Http {
        url: url.clone(),
        source: Box::new(err),
    })?;
    let parsed: BatchResponse =
        serde_json::from_reader(response.into_reader()).map_err(|err| Error::Decode { url, source: err })?;

    let object = parsed
        .objects
        .into_iter()
        .find(|object| object.oid == oid)
        .ok_or_else(|| Error::MissingObject { oid: oid.clone() })?;
    if let Some(error) = object.error {
        return Err(Error::Object {
            oid,
            code: error.code,
            message: error.message,
        });
    }
    let action = object
        .actions
        .and_then(|actions| actions.download)
        .ok_or_else(|| Error::NoDownloadAction { oid: oid.clone() })?;

    let mut request = agent.get(&action.href);
    for (name, value) in &action.header {
        request = request.set(name, value);
    }
    let response = request.call().map_err(|err| Error::Download {
        oid: oid.clone(),
        url: action.href.clone(),
        source: Box::new(err),
    })?;

    let initial_capacity = usize::try_from(pointer.size.min(64 * 1024 * 1024)).unwrap_or(usize::MAX);
    let mut content = Vec::with_capacity(initial_capacity);
    response
        .into_reader()
        .take(pointer.size.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|err| Error::ReadContent {
            oid,
            url: action.href,
            source: err,
        })?;
    Ok(content)
}
