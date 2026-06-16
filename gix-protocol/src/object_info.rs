//! The `object-info` V2 command, used to query information about objects (currently their size)
//! without fetching them.
#[cfg(any(feature = "blocking-client", feature = "async-client"))]
mod error {
    /// The error returned by invoking an [`super::function::ObjectInfoCommand`].
    #[derive(Debug, thiserror::Error)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error(transparent)]
        Io(#[from] std::io::Error),
        #[error(transparent)]
        Transport(#[from] gix_transport::client::Error),
        #[error(transparent)]
        DecodePacketline(#[from] gix_transport::packetline::decode::Error),
        #[error(transparent)]
        ArgumentValidation(#[from] crate::command::validate_argument_prefixes::Error),
        #[error("the server response could not be parsed: {line:?}")]
        MalformedResponse { line: bstr::BString },
        #[error(transparent)]
        InvalidObjectId(#[from] gix_hash::decode::Error),
    }

    impl gix_transport::IsSpuriousError for Error {
        fn is_spurious(&self) -> bool {
            match self {
                Error::Io(err) => err.is_spurious(),
                Error::Transport(err) => err.is_spurious(),
                _ => false,
            }
        }
    }
}
#[cfg(any(feature = "blocking-client", feature = "async-client"))]
pub use error::Error;

#[cfg(any(feature = "blocking-client", feature = "async-client"))]
pub(crate) mod function {
    use std::borrow::Cow;

    use bstr::{BStr, BString, ByteSlice, ByteVec};
    use gix_features::progress::Progress;
    use gix_hash::ObjectId;
    use gix_transport::client::Capabilities;

    use super::Error;
    use crate::Command;
    #[cfg(feature = "async-client")]
    use crate::transport::client::async_io::{self, TransportV2Ext as _};
    #[cfg(feature = "blocking-client")]
    use crate::transport::client::blocking_io::{self, TransportV2Ext as _};

    /// The size of an object as reported by the server.
    pub type ObjectInfo = (ObjectId, u64);

    /// A command to query information about objects from a remote Git repository without fetching them.
    ///
    /// It acts as a utility to separate the invocation into the shared blocking portion,
    /// and the one that performs IO either blocking or `async`.
    pub struct ObjectInfoCommand<'a> {
        pub(crate) capabilities: &'a Capabilities,
        features: Vec<(&'static str, Option<Cow<'static, str>>)>,
        arguments: Vec<BString>,
    }

    impl<'a> ObjectInfoCommand<'a> {
        /// Build a command to query the size of each object in `oids` from the server `capabilities`,
        /// using `agent` information to identify ourselves.
        pub fn new(
            oids: impl IntoIterator<Item = ObjectId>,
            capabilities: &'a Capabilities,
            agent: (&'static str, Option<Cow<'static, str>>),
        ) -> Self {
            let object_info = Command::ObjectInfo;
            let mut features = object_info.default_features(gix_transport::Protocol::V2, capabilities);
            features.push(agent);

            // The first argument selects the `size` attribute, then one `oid <hex>` per object.
            let mut arguments = vec![b"size".as_bstr().to_owned()];
            for oid in oids {
                let mut argument = BString::from("oid ");
                argument.push_str(oid.to_hex().to_string());
                arguments.push(argument);
            }

            Self {
                capabilities,
                features,
                arguments,
            }
        }

        /// Invoke an object-info V2 command on `transport`.
        ///
        /// `progress` is used to provide feedback.
        /// If `trace` is `true`, all packetlines received or sent will be passed to the facilities of the `gix-trace` crate.
        #[cfg(feature = "async-client")]
        pub async fn invoke_async(
            self,
            mut transport: impl async_io::Transport,
            progress: &mut impl Progress,
            trace: bool,
        ) -> Result<Vec<ObjectInfo>, Error> {
            let _span = gix_features::trace::detail!("gix_protocol::ObjectInfoCommand::invoke_async()");
            Command::ObjectInfo.validate_argument_prefixes(
                gix_transport::Protocol::V2,
                self.capabilities,
                &self.arguments,
                &self.features,
            )?;

            progress.step();
            progress.set_name("object info".into());
            let mut reader = transport
                .invoke(
                    Command::ObjectInfo.as_str(),
                    self.features.into_iter(),
                    Some(self.arguments.into_iter()),
                    trace,
                )
                .await?;

            let mut out = Vec::new();
            let mut saw_header = false;
            while let Some(line) = reader
                .readline()
                .await
                .transpose()?
                .transpose()?
                .and_then(|l| l.as_bstr())
            {
                if !saw_header {
                    // The first line echoes back the requested attributes, e.g. `size`.
                    saw_header = true;
                    continue;
                }
                out.push(parse_object_info_line(line)?);
            }
            Ok(out)
        }

        /// Invoke an object-info V2 command on `transport`.
        ///
        /// `progress` is used to provide feedback.
        /// If `trace` is `true`, all packetlines received or sent will be passed to the facilities of the `gix-trace` crate.
        #[cfg(feature = "blocking-client")]
        pub fn invoke_blocking(
            self,
            mut transport: impl blocking_io::Transport,
            progress: &mut impl Progress,
            trace: bool,
        ) -> Result<Vec<ObjectInfo>, Error> {
            let _span = gix_features::trace::detail!("gix_protocol::ObjectInfoCommand::invoke_blocking()");
            Command::ObjectInfo.validate_argument_prefixes(
                gix_transport::Protocol::V2,
                self.capabilities,
                &self.arguments,
                &self.features,
            )?;

            progress.step();
            progress.set_name("object info".into());
            let mut reader = transport.invoke(
                Command::ObjectInfo.as_str(),
                self.features.into_iter(),
                Some(self.arguments.into_iter()),
                trace,
            )?;

            let mut out = Vec::new();
            let mut saw_header = false;
            while let Some(line) = reader.readline().transpose()?.transpose()?.and_then(|l| l.as_bstr()) {
                if !saw_header {
                    // The first line echoes back the requested attributes, e.g. `size`.
                    saw_header = true;
                    continue;
                }
                out.push(parse_object_info_line(line)?);
            }
            Ok(out)
        }
    }

    /// Parse a single `<oid> SP <size>` response line. IO-flavor agnostic so the blocking and
    /// async readline loops share it.
    pub(crate) fn parse_object_info_line(line: &BStr) -> Result<ObjectInfo, Error> {
        let line = line.trim();
        let mut tokens = line.splitn(2, |b| *b == b' ');
        let oid = tokens
            .next()
            .ok_or_else(|| Error::MalformedResponse { line: line.into() })?;
        let size = tokens
            .next()
            .ok_or_else(|| Error::MalformedResponse { line: line.into() })?;
        let oid = ObjectId::from_hex(oid)?;
        let size = size
            .to_str()
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .ok_or_else(|| Error::MalformedResponse { line: line.into() })?;
        Ok((oid, size))
    }

    #[cfg(test)]
    mod tests {
        use super::parse_object_info_line;

        #[test]
        fn parses_oid_and_size() {
            let (oid, size) =
                parse_object_info_line("e3bc2bf75d3816a3e60c0a0b27f87a3b9b8a4f99 12345".into()).expect("valid line");
            assert_eq!(oid.to_hex().to_string(), "e3bc2bf75d3816a3e60c0a0b27f87a3b9b8a4f99");
            assert_eq!(size, 12345);
        }

        #[test]
        fn rejects_missing_size() {
            assert!(parse_object_info_line("e3bc2bf75d3816a3e60c0a0b27f87a3b9b8a4f99".into()).is_err());
        }

        #[test]
        fn rejects_non_numeric_size() {
            assert!(parse_object_info_line("e3bc2bf75d3816a3e60c0a0b27f87a3b9b8a4f99 not-a-number".into()).is_err());
        }
    }
}
