//! A session's QUIC connection, over UDP or a Bluetooth link. Only what the session, the transfers, and the pairing
//! handshake use.

use std::ops::Deref;

use link_proto::CloseCode;
use link_proto::pairing::{EXPORTER_LABEL, EXPORTER_LEN};

use crate::Error;

#[derive(Clone, Debug)]
pub struct Connection(quinn::Connection);

impl Connection {
    /// Keying material from the handshake, which pairing binds its transcript to; the same label over either path.
    pub fn exporter(&self) -> Result<[u8; EXPORTER_LEN], Error> {
        let mut out = [0; EXPORTER_LEN];
        self.0.export_keying_material(&mut out, EXPORTER_LABEL, &[]).map_err(|_| Error::BadKey)?;
        Ok(out)
    }
}

pub struct SendStream(quinn::SendStream);

pub struct RecvStream(quinn::RecvStream);

/// Bytes read from a stream, borrowed from quinn's buffer.
pub struct Chunk(quinn::Chunk);

impl Deref for Chunk {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}

impl From<quinn::Connection> for Connection {
    fn from(connection: quinn::Connection) -> Self {
        Self(connection)
    }
}

impl Connection {
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), Error> {
        let (send, recv) = self.0.open_bi().await?;
        Ok((SendStream(send), RecvStream(recv)))
    }

    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), Error> {
        let (send, recv) = self.0.accept_bi().await?;
        Ok((SendStream(send), RecvStream(recv)))
    }

    pub async fn open_uni(&self) -> Result<SendStream, Error> {
        Ok(SendStream(self.0.open_uni().await?))
    }

    pub async fn accept_uni(&self) -> Result<RecvStream, Error> {
        Ok(RecvStream(self.0.accept_uni().await?))
    }

    /// Closes with a protocol close code.
    pub fn close(&self, code: CloseCode) {
        self.0.close(quinn::VarInt::from_u32(code as u32), code.reason().as_bytes());
    }

    pub fn is_live(&self) -> bool {
        self.0.close_reason().is_none()
    }

    /// Waits for the end and says how it came.
    pub async fn closed(&self) -> Error {
        self.0.closed().await.into()
    }

    pub fn stable_id(&self) -> usize {
        self.0.stable_id()
    }

    /// Whether this runs over a Bluetooth link, which quinn sees at a link address.
    pub fn on_bluetooth(&self) -> bool {
        crate::path::is_link_address(self.0.remote_address())
    }

    /// The QUIC connection, for what only it has, such as the peer's address.
    pub fn quic(&self) -> &quinn::Connection {
        &self.0
    }
}

impl SendStream {
    pub async fn write_all(&mut self, bytes: &[u8]) -> Result<(), Error> {
        Ok(self.0.write_all(bytes).await?)
    }

    pub fn finish(&mut self) -> Result<(), Error> {
        Ok(self.0.finish()?)
    }

    /// Waits until the peer stops the stream and returns its code; None once the stream is done or the connection
    /// ends.
    pub async fn stopped(&mut self) -> Option<u32> {
        self.0.stopped().await.ok().flatten().and_then(|code| u32::try_from(code.into_inner()).ok())
    }

    /// Queued data of higher priority goes first.
    pub fn set_priority(&self, priority: i32) {
        drop(self.0.set_priority(priority));
    }

    /// Abandons the stream with `code`; an already finished stream stays as it is.
    pub fn reset(&mut self, code: u32) {
        drop(self.0.reset(quinn::VarInt::from_u32(code)));
    }
}

impl RecvStream {
    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        Ok(self.0.read_exact(buf).await?)
    }

    /// Up to `max` bytes in order, or None at the end of the stream.
    pub async fn read_chunk(&mut self, max: usize) -> Result<Option<Chunk>, Error> {
        Ok(self.0.read_chunk(max, true).await.map_err(|_| Error::StreamEnded)?.map(Chunk))
    }

    pub fn stop(&mut self, code: u32) {
        drop(self.0.stop(quinn::VarInt::from_u32(code)));
    }
}
