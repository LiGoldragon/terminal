//! Length-prefixed rkyv Signal framing.
//!
//! Every contract terminal speaks — `signal-terminal`,
//! `meta-signal-terminal`, `signal-persona` — carries its own rkyv `Signal<T>`
//! archive and nothing else. One frame is a big-endian `u32` byte count
//! followed by exactly that many archive bytes. There is no exchange
//! identifier, no lane, no sub-reply envelope and no subscription token:
//! those belonged to `signal-frame`, which the contracts replaced. A
//! streamed event is an ordinary reply frame whose value is the contract's
//! own event variant.
//!
//! The byte layer is shared; the typed layer is one small pair of functions
//! per contract, because each contract crate declares its own `Signal<T>`.

use std::io::{Read, Write};

use crate::{Error, Result};

/// The largest frame terminal will read. A peer claiming more is refused
/// before any allocation is made on its word.
pub const MAXIMUM_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Read one length-prefixed frame's payload bytes.
pub fn read_frame_bytes(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAXIMUM_FRAME_BYTES {
        return Err(Error::UnexpectedSignalFrame {
            got: format!("frame length {length} exceeds {MAXIMUM_FRAME_BYTES}"),
        });
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Write one length-prefixed frame and flush it.
pub fn write_frame_bytes(writer: &mut impl Write, bytes: &[u8]) -> Result<()> {
    write_length_prefix(writer, bytes.len())?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}

fn write_length_prefix(writer: &mut impl Write, length: usize) -> Result<()> {
    if length > MAXIMUM_FRAME_BYTES {
        return Err(Error::UnexpectedSignalFrame {
            got: format!("frame length {length} exceeds {MAXIMUM_FRAME_BYTES}"),
        });
    }
    writer.write_all(&(length as u32).to_be_bytes())?;
    Ok(())
}

/// Read one length-prefixed frame's payload bytes from a Tokio stream.
pub async fn read_frame_bytes_async(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAXIMUM_FRAME_BYTES {
        return Err(Error::UnexpectedSignalFrame {
            got: format!("frame length {length} exceeds {MAXIMUM_FRAME_BYTES}"),
        });
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// Write one length-prefixed frame to a Tokio stream and flush it.
pub async fn write_frame_bytes_async(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    bytes: &[u8],
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    if bytes.len() > MAXIMUM_FRAME_BYTES {
        return Err(Error::UnexpectedSignalFrame {
            got: format!("frame length {} exceeds {MAXIMUM_FRAME_BYTES}", bytes.len()),
        });
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// The `signal-terminal` frame pair.
pub mod terminal {
    use signal_terminal::{ByteViewable, Query, Response, Restorable, Signal, Signalizable};

    use super::{
        read_frame_bytes, read_frame_bytes_async, write_frame_bytes, write_frame_bytes_async,
    };
    use crate::{Error, Result};

    pub fn read_query(reader: &mut impl std::io::Read) -> Result<Query> {
        Signal::<Query>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_query(writer: &mut impl std::io::Write, query: &Query) -> Result<()> {
        write_frame_bytes(writer, query.signalize().map_err(archive_failure)?.bytes())
    }

    pub fn read_response(reader: &mut impl std::io::Read) -> Result<Response> {
        Signal::<Response>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_response(writer: &mut impl std::io::Write, response: &Response) -> Result<()> {
        write_frame_bytes(
            writer,
            response.signalize().map_err(archive_failure)?.bytes(),
        )
    }

    pub async fn read_query_async(
        reader: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Result<Query> {
        Signal::<Query>::from(read_frame_bytes_async(reader).await?)
            .restore()
            .map_err(restore_failure)
    }

    pub async fn write_query_async(
        writer: &mut (impl tokio::io::AsyncWrite + Unpin),
        query: &Query,
    ) -> Result<()> {
        write_frame_bytes_async(writer, query.signalize().map_err(archive_failure)?.bytes()).await
    }

    pub async fn read_response_async(
        reader: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Result<Response> {
        Signal::<Response>::from(read_frame_bytes_async(reader).await?)
            .restore()
            .map_err(restore_failure)
    }

    pub async fn write_response_async(
        writer: &mut (impl tokio::io::AsyncWrite + Unpin),
        response: &Response,
    ) -> Result<()> {
        write_frame_bytes_async(
            writer,
            response.signalize().map_err(archive_failure)?.bytes(),
        )
        .await
    }

    fn restore_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("signal-terminal archive did not restore: {error}"),
        }
    }

    fn archive_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("signal-terminal value did not archive: {error}"),
        }
    }
}

/// The `meta-signal-terminal` frame pair.
pub mod meta {
    use meta_signal_terminal::{ByteViewable, Query, Response, Restorable, Signal, Signalizable};

    use super::{
        read_frame_bytes, read_frame_bytes_async, write_frame_bytes, write_frame_bytes_async,
    };
    use crate::{Error, Result};

    pub fn read_query(reader: &mut impl std::io::Read) -> Result<Query> {
        Signal::<Query>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_query(writer: &mut impl std::io::Write, query: &Query) -> Result<()> {
        write_frame_bytes(writer, query.signalize().map_err(archive_failure)?.bytes())
    }

    pub fn read_response(reader: &mut impl std::io::Read) -> Result<Response> {
        Signal::<Response>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_response(writer: &mut impl std::io::Write, response: &Response) -> Result<()> {
        write_frame_bytes(
            writer,
            response.signalize().map_err(archive_failure)?.bytes(),
        )
    }

    pub async fn read_query_async(
        reader: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Result<Query> {
        Signal::<Query>::from(read_frame_bytes_async(reader).await?)
            .restore()
            .map_err(restore_failure)
    }

    pub async fn write_response_async(
        writer: &mut (impl tokio::io::AsyncWrite + Unpin),
        response: &Response,
    ) -> Result<()> {
        write_frame_bytes_async(
            writer,
            response.signalize().map_err(archive_failure)?.bytes(),
        )
        .await
    }

    fn restore_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("meta-signal-terminal archive did not restore: {error}"),
        }
    }

    fn archive_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("meta-signal-terminal value did not archive: {error}"),
        }
    }
}

/// The `signal-persona` supervision frame pair.
pub mod persona {
    use signal_persona::{ByteViewable, Query, Response, Restorable, Signal, Signalizable};

    use super::{read_frame_bytes, write_frame_bytes};
    use crate::{Error, Result};

    pub fn read_query(reader: &mut impl std::io::Read) -> Result<Query> {
        Signal::<Query>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_query(writer: &mut impl std::io::Write, query: &Query) -> Result<()> {
        write_frame_bytes(writer, query.signalize().map_err(archive_failure)?.bytes())
    }

    pub fn read_response(reader: &mut impl std::io::Read) -> Result<Response> {
        Signal::<Response>::from(read_frame_bytes(reader)?)
            .restore()
            .map_err(restore_failure)
    }

    pub fn write_response(writer: &mut impl std::io::Write, response: &Response) -> Result<()> {
        write_frame_bytes(
            writer,
            response.signalize().map_err(archive_failure)?.bytes(),
        )
    }

    fn restore_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("signal-persona archive did not restore: {error}"),
        }
    }

    fn archive_failure(error: rkyv::rancor::Error) -> Error {
        Error::UnexpectedSignalFrame {
            got: format!("signal-persona value did not archive: {error}"),
        }
    }
}
