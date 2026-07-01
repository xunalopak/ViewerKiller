//! Lecture/écriture asynchrone d'un message unique cadré par préfixe de
//! longueur u32 (big-endian), **en clair**.
//!
//! Utilisé pour la phase d'authentification par code (sondes
//! `Probe`/`ProbeResult`), qui précède le handshake chiffré.

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use vk_core::codec::{decode_message, encode_message, CodecError, MAX_FRAME_LEN};

use crate::{NetError, Result};

/// Écrit un message unique cadré, puis vide le tampon.
pub async fn write_framed<S, T>(stream: &mut S, msg: &T) -> Result<()>
where
    S: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = encode_message(msg)?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Lit un message unique cadré.
pub async fn read_framed<S, T>(stream: &mut S) -> Result<T>
where
    S: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(NetError::Codec(CodecError::FrameTooLarge(len)));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(decode_message(&buf)?)
}
