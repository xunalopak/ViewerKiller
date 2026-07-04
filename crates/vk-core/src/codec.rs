//! Cadrage des messages : préfixe de longueur 32 bits (big-endian) suivi d'une
//! charge utile sérialisée avec `postcard`.
//!
//! Le cadrage est volontairement indépendant du runtime : on n'y fait aucune
//! I/O. [`FrameReader`] accumule les octets reçus et restitue les trames
//! complètes, ce qui le rend trivial à tester sans réseau.

use serde::{de::DeserializeOwned, Serialize};

/// Taille maximale d'une trame acceptée (garde-fou anti-DoS).
pub const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("trame trop volumineuse : {0} octets")]
    FrameTooLarge(usize),
    #[error("erreur de (dé)sérialisation : {0}")]
    Serde(#[from] postcard::Error),
}

/// Sérialise un message et le préfixe de sa longueur (big-endian).
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>, CodecError> {
    let payload = postcard::to_allocvec(msg)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Désérialise une charge utile (sans le préfixe de longueur).
pub fn decode_message<T: DeserializeOwned>(payload: &[u8]) -> Result<T, CodecError> {
    Ok(postcard::from_bytes(payload)?)
}

/// Accumulateur d'octets qui découpe un flux en trames complètes.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute des octets reçus du réseau.
    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Extrait la prochaine trame complète, le cas échéant.
    ///
    /// Renvoie `Ok(None)` tant que la trame courante est incomplète, et une
    /// erreur si le préfixe de longueur dépasse [`MAX_FRAME_LEN`].
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, CodecError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME_LEN {
            return Err(CodecError::FrameTooLarge(len));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;
    use serde::{de::DeserializeOwned, Serialize};

    #[test]
    fn round_trip_discovery() {
        let msg = DiscoveryMessage::Probe {
            proto_version: PROTO_VERSION,
            code: "847392".into(),
        };
        let bytes = encode_message(&msg).unwrap();
        let mut reader = FrameReader::new();
        reader.feed(&bytes);
        let payload = reader.next_frame().unwrap().unwrap();
        let decoded: DiscoveryMessage = decode_message(&payload).unwrap();
        assert_eq!(msg, decoded);
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn partial_frame_returns_none_until_complete() {
        let msg = ControllerMessage::Input(InputEvent::MouseMove { x: 10, y: 20 });
        let bytes = encode_message(&msg).unwrap();
        let mut reader = FrameReader::new();
        reader.feed(&bytes[..bytes.len() - 1]);
        assert!(reader.next_frame().unwrap().is_none());
        reader.feed(&bytes[bytes.len() - 1..]);
        let payload = reader.next_frame().unwrap().unwrap();
        assert_eq!(decode_message::<ControllerMessage>(&payload).unwrap(), msg);
    }

    #[test]
    fn two_frames_back_to_back() {
        let a = HostMessage::ScreenInfo {
            width: 1920,
            height: 1080,
        };
        let b = HostMessage::Bye;
        let mut stream = encode_message(&a).unwrap();
        stream.extend(encode_message(&b).unwrap());
        let mut reader = FrameReader::new();
        reader.feed(&stream);
        let pa = reader.next_frame().unwrap().unwrap();
        let pb = reader.next_frame().unwrap().unwrap();
        assert_eq!(decode_message::<HostMessage>(&pa).unwrap(), a);
        assert_eq!(decode_message::<HostMessage>(&pb).unwrap(), b);
        assert!(reader.next_frame().unwrap().is_none());
    }

    #[test]
    fn char_event_round_trip() {
        let msg = ControllerMessage::Input(InputEvent::Char { c: 'é' });
        let bytes = encode_message(&msg).unwrap();
        let mut reader = FrameReader::new();
        reader.feed(&bytes);
        let payload = reader.next_frame().unwrap().unwrap();
        assert_eq!(decode_message::<ControllerMessage>(&payload).unwrap(), msg);
    }

    fn round_trip<T>(msg: &T) -> T
    where
        T: Serialize + DeserializeOwned,
    {
        let bytes = encode_message(msg).unwrap();
        let mut reader = FrameReader::new();
        reader.feed(&bytes);
        let payload = reader.next_frame().unwrap().unwrap();
        decode_message::<T>(&payload).unwrap()
    }

    #[test]
    fn clipboard_messages_round_trip() {
        let ctrl = ControllerMessage::Clipboard("collé é€".into());
        assert_eq!(round_trip(&ctrl), ctrl);
        let host = HostMessage::Clipboard("copié 🚀".into());
        assert_eq!(round_trip(&host), host);
    }

    #[test]
    fn keepalive_messages_round_trip() {
        assert_eq!(
            round_trip(&ControllerMessage::Ping),
            ControllerMessage::Ping
        );
        assert_eq!(round_trip(&HostMessage::Ping), HostMessage::Ping);
    }

    #[test]
    fn oversize_length_prefix_rejected() {
        let mut reader = FrameReader::new();
        let bogus = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes();
        reader.feed(&bogus);
        assert!(matches!(
            reader.next_frame(),
            Err(CodecError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn frame_update_round_trip() {
        let msg = HostMessage::Frame(FrameUpdate {
            seq: 42,
            tiles: vec![Tile {
                x: 0,
                y: 0,
                width: 16,
                height: 16,
                codec: TileCodec::Jpeg,
                data: vec![1, 2, 3, 4],
            }],
        });
        let bytes = encode_message(&msg).unwrap();
        let mut reader = FrameReader::new();
        reader.feed(&bytes);
        let payload = reader.next_frame().unwrap().unwrap();
        assert_eq!(decode_message::<HostMessage>(&payload).unwrap(), msg);
    }
}
