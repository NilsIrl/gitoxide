use std::path::Path;

use gix_lfs::pointer::Pointer;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const CONTENT: &[u8] = b"hello from the lfs object store\n";

fn content_pointer(content: &[u8]) -> Pointer {
    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha256);
    hasher.update(content);
    Pointer {
        oid: hasher.try_finalize().expect("SHA-256 is infallible"),
        size: content.len() as u64,
    }
}

fn pointer_blob(pointer: &Pointer) -> Vec<u8> {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {}\n",
        pointer.oid, pointer.size
    )
    .into_bytes()
}

/// Create a minimal repository at `dir` that `gix-discover` accepts, without running git.
fn fabricate_repo(dir: &Path, config: &str) -> Result {
    let git_dir = dir.join(".git");
    std::fs::create_dir_all(git_dir.join("objects"))?;
    std::fs::create_dir_all(git_dir.join("refs"))?;
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")?;
    std::fs::write(git_dir.join("config"), config)?;
    Ok(())
}

fn store_object(repo: &Path, pointer: &Pointer, content: &[u8]) -> Result {
    let hex = pointer.oid.to_string();
    let dir = repo
        .join(".git")
        .join("lfs")
        .join("objects")
        .join(&hex[..2])
        .join(&hex[2..4]);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(hex), content)?;
    Ok(())
}

mod pointer {
    use gix_lfs::pointer::{MAX_POINTER_SIZE, Pointer};

    use crate::{CONTENT, content_pointer, pointer_blob};

    #[test]
    fn parses_a_valid_pointer_and_legacy_hawser_version() {
        let pointer = content_pointer(CONTENT);
        assert_eq!(Pointer::from_blob(&pointer_blob(&pointer)), Some(pointer.clone()));

        let legacy = pointer_blob(&pointer).replace(
            "https://git-lfs.github.com/spec/v1".as_bytes(),
            "https://hawser.github.com/spec/v1".as_bytes(),
        );
        assert_eq!(Pointer::from_blob(&legacy), Some(pointer));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let pointer = content_pointer(CONTENT);
        let mut blob = pointer_blob(&pointer);
        blob.extend_from_slice(b"x-custom value\n");
        assert_eq!(Pointer::from_blob(&blob), Some(pointer));
    }

    #[test]
    fn non_pointers_are_rejected() {
        assert_eq!(Pointer::from_blob(b"plain text"), None, "no version line");
        assert_eq!(
            Pointer::from_blob(b"version https://example.com/spec/v1\noid sha256:00\nsize 1\n"),
            None,
            "unknown spec url"
        );
        assert_eq!(
            Pointer::from_blob(b"version https://git-lfs.github.com/spec/v1\nsize 1\n"),
            None,
            "missing oid"
        );
        assert_eq!(
            Pointer::from_blob(b"version https://git-lfs.github.com/spec/v1\noid sha256:beef\nsize 1\n"),
            None,
            "oid is not a sha256"
        );

        let pointer = content_pointer(crate::CONTENT);
        let mut blob = pointer_blob(&pointer);
        blob.resize(MAX_POINTER_SIZE + 1, b' ');
        assert_eq!(Pointer::from_blob(&blob), None, "too large to be a pointer");
    }

    trait ReplaceExt {
        fn replace(&self, from: &[u8], to: &[u8]) -> Vec<u8>;
    }

    impl ReplaceExt for Vec<u8> {
        fn replace(&self, from: &[u8], to: &[u8]) -> Vec<u8> {
            use bstr::ByteSlice;
            self.as_slice().replace(from, to)
        }
    }
}

mod store {
    use gix_lfs::store::{Store, verify};

    use crate::{CONTENT, Result, content_pointer};

    #[test]
    fn write_then_read_roundtrips_and_verifies() -> Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        let store = Store::at_common_dir(dir.path());
        let pointer = content_pointer(CONTENT);

        assert_eq!(store.read(&pointer)?, None, "not present initially");
        store.write(&pointer, CONTENT)?;
        assert_eq!(store.read(&pointer)?, Some(CONTENT.to_vec()));

        assert!(
            store.write(&pointer, b"other content of the same length").is_err(),
            "content with a different hash is rejected"
        );
        assert!(verify(&pointer, b"short").is_err(), "size mismatch is rejected");
        Ok(())
    }
}

mod smudge {
    use gix_lfs::Options;

    use crate::{CONTENT, Result, content_pointer, fabricate_repo, pointer_blob, store_object};

    #[test]
    fn non_pointers_pass_through_unchanged() -> Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        fabricate_repo(dir.path(), "")?;

        let blob = b"just a regular file\n".to_vec();
        let smudged = gix_lfs::smudge(dir.path(), blob.clone(), &Options::default())?;
        assert_eq!(smudged, blob);
        Ok(())
    }

    #[test]
    fn pointers_resolve_from_the_local_store_without_a_remote() -> Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        fabricate_repo(dir.path(), "")?;
        let pointer = content_pointer(CONTENT);
        store_object(dir.path(), &pointer, CONTENT)?;

        let smudged = gix_lfs::smudge(dir.path(), pointer_blob(&pointer), &Options::default())?;
        assert_eq!(smudged, CONTENT);
        Ok(())
    }

    #[test]
    fn pointers_fetch_from_a_local_path_remote_and_populate_the_store() -> Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        let source = dir.path().join("source");
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(&source)?;
        std::fs::create_dir_all(&clone)?;

        fabricate_repo(&source, "")?;
        let pointer = content_pointer(CONTENT);
        store_object(&source, &pointer, CONTENT)?;

        let config = format!("[remote \"origin\"]\n\turl = {}\n", source.display());
        fabricate_repo(&clone, &config)?;

        let smudged = gix_lfs::smudge(&clone, pointer_blob(&pointer), &Options::default())?;
        assert_eq!(smudged, CONTENT);

        // The object was copied into the clone's store: remove the source and read again.
        std::fs::remove_dir_all(&source)?;
        let smudged = gix_lfs::smudge(&clone, pointer_blob(&pointer), &Options::default())?;
        assert_eq!(smudged, CONTENT);
        Ok(())
    }

    #[test]
    fn missing_objects_are_an_error_not_a_passthrough() -> Result {
        let dir = gix_testtools::tempfile::TempDir::new()?;
        fabricate_repo(dir.path(), "")?;
        let pointer = content_pointer(CONTENT);

        let result = gix_lfs::smudge(dir.path(), pointer_blob(&pointer), &Options::default());
        assert!(result.is_err(), "a pointer without a fetchable object must fail");
        Ok(())
    }
}
