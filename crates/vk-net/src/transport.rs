//! Flux chiffré de bout en bout au-dessus d'un transport TCP.
//!
//! Format sur le fil après handshake : une suite d'**enregistrements**
//! `[u16 longueur][texte chiffré Noise]`. Le texte clair reconstitué est
//! lui-même un flux de messages applicatifs cadrés (préfixe u32, voir
//! [`vk_core::codec`]). Un message applicatif plus grand que la charge utile
//! maximale d'un message Noise est découpé en plusieurs enregistrements de
//! façon transparente.

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use vk_core::codec::{decode_message, encode_message, FrameReader};
use vk_core::crypto::{Handshake, Transport, MAX_NOISE_PAYLOAD};

use crate::Result;

/// Connexion chiffrée prête à échanger des messages applicatifs typés.
///
/// `recv` est **sûr vis-à-vis de l'annulation** (`tokio::select!`) : l'état de
/// lecture vit dans la structure, un futur `recv` abandonné en plein
/// enregistrement ne perd aucun octet déjà consommé sur le socket.
pub struct EncryptedStream<S> {
    inner: S,
    transport: Transport,
    reader: FrameReader,
    /// Octets bruts reçus mais pas encore consommés (enregistrement en cours).
    rx: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> EncryptedStream<S> {
    fn new(inner: S, transport: Transport) -> Self {
        Self {
            inner,
            transport,
            reader: FrameReader::new(),
            rx: Vec::new(),
        }
    }

    /// Établit la session côté **contrôleur** (initiateur du handshake Noise).
    pub async fn connect(mut inner: S, psk: &[u8; 32]) -> Result<Self> {
        let mut hs = Handshake::initiator(psk)?;
        let mut buf = [0u8; 4096];
        let mut tmp = [0u8; 4096];

        // -> e
        let n = hs.write_message(&[], &mut buf)?;
        write_record(&mut inner, &buf[..n]).await?;
        // <- e, ee
        let msg = read_record(&mut inner).await?;
        hs.read_message(&msg, &mut tmp)?;

        Ok(Self::new(inner, hs.into_transport()?))
    }

    /// Établit la session côté **hôte** (répondeur du handshake Noise).
    pub async fn accept(mut inner: S, psk: &[u8; 32]) -> Result<Self> {
        let mut hs = Handshake::responder(psk)?;
        let mut buf = [0u8; 4096];
        let mut tmp = [0u8; 4096];

        // -> e
        let msg = read_record(&mut inner).await?;
        hs.read_message(&msg, &mut tmp)?;
        // <- e, ee
        let n = hs.write_message(&[], &mut buf)?;
        write_record(&mut inner, &buf[..n]).await?;

        Ok(Self::new(inner, hs.into_transport()?))
    }

    /// Sérialise, chiffre et envoie un message applicatif.
    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let framed = encode_message(msg)?;
        for chunk in framed.chunks(MAX_NOISE_PAYLOAD) {
            let ct = self.transport.encrypt(chunk)?;
            write_record(&mut self.inner, &ct).await?;
        }
        self.inner.flush().await?;
        Ok(())
    }

    /// Reçoit, déchiffre et désérialise le prochain message applicatif.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        loop {
            if let Some(frame) = self.reader.next_frame()? {
                return Ok(decode_message(&frame)?);
            }
            let ct = self.next_record().await?;
            let pt = self.transport.decrypt(&ct)?;
            self.reader.feed(&pt);
        }
    }

    /// Extrait le prochain enregistrement `[u16 longueur][texte chiffré]`.
    ///
    /// Sûr vis-à-vis de l'annulation, contrairement à `read_exact` sur des
    /// tampons locaux (qui perd les octets déjà lus si le futur est annulé
    /// par `tokio::select!`, désynchronisant le flux → « decrypt error ») :
    /// `read` ne consomme rien s'il est abandonné, et les octets lus sont
    /// rangés dans `self.rx` avant tout nouveau point d'attente.
    async fn next_record(&mut self) -> Result<Vec<u8>> {
        loop {
            if self.rx.len() >= 2 {
                let n = u16::from_be_bytes([self.rx[0], self.rx[1]]) as usize;
                if self.rx.len() >= 2 + n {
                    let record = self.rx[2..2 + n].to_vec();
                    self.rx.drain(..2 + n);
                    return Ok(record);
                }
            }
            let mut chunk = [0u8; 16 * 1024];
            let n = self.inner.read(&mut chunk).await?;
            if n == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            self.rx.extend_from_slice(&chunk[..n]);
        }
    }
}

/// Écrit un enregistrement `[u16 longueur][données]`.
async fn write_record<S: AsyncWrite + Unpin>(s: &mut S, data: &[u8]) -> Result<()> {
    debug_assert!(
        data.len() <= u16::MAX as usize,
        "enregistrement > 65535 octets"
    );
    s.write_all(&(data.len() as u16).to_be_bytes()).await?;
    s.write_all(data).await?;
    Ok(())
}

/// Lit un enregistrement `[u16 longueur][données]`.
///
/// Réservé au handshake (séquentiel, jamais dans un `tokio::select!`) : les
/// tampons sont locaux au futur, donc **pas** sûr vis-à-vis de l'annulation.
/// Après le handshake, utiliser [`EncryptedStream::next_record`].
async fn read_record<S: AsyncRead + Unpin>(s: &mut S) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len).await?;
    let n = u16::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}
