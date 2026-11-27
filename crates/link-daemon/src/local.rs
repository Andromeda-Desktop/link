//! The local byte streams that carry media between linkd and a viewer or capturer on this desktop: a Unix socket pair
//! whose far end goes to the client as a descriptor. Windows has no such stream yet, so there every stream fails to
//! open and no handle exists.

#[cfg(unix)]
mod imp {
    use std::os::unix::net::UnixStream;

    pub type Handle = std::os::fd::OwnedFd;
    pub type Stream = tokio::net::UnixStream;

    /// Our end, ready for tokio, and the end for the client.
    pub fn pair() -> std::io::Result<(Stream, Handle)> {
        let (ours, theirs) = UnixStream::pair()?;
        Ok((adopt_std(ours)?, theirs.into()))
    }

    /// A client's end, ready for tokio.
    pub fn adopt(handle: Handle) -> std::io::Result<Stream> {
        adopt_std(UnixStream::from(handle))
    }

    fn adopt_std(stream: UnixStream) -> std::io::Result<Stream> {
        stream.set_nonblocking(true)?;
        Stream::from_std(stream)
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    pub enum Handle {}
    pub type Stream = tokio::io::DuplexStream;

    pub fn pair() -> io::Result<(Stream, Handle)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no local media streams on Windows yet"))
    }

    #[expect(clippy::needless_pass_by_value, reason = "the Unix twin's signature")]
    pub fn adopt(handle: Handle) -> io::Result<Stream> {
        match handle {}
    }
}

pub use imp::{Handle, Stream, adopt, pair};
