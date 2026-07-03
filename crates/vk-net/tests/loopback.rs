//! Tests d'intégration en boucle locale : sonde de vérification de code,
//! handshake Noise sur TCP réel, échange chiffré avec fragmentation d'un
//! message > 64 KiB, et sûreté de `recv` face à l'annulation par
//! `tokio::select!` sur un flux fragmenté.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use vk_core::crypto::derive_psk;
use vk_core::protocol::{
    ControllerMessage, DiscoveryMessage, FrameUpdate, HostMessage, Tile, TileCodec, PROTO_VERSION,
};
use vk_net::frame::{read_framed, write_framed};
use vk_net::transport::EncryptedStream;

#[tokio::test]
async fn probe_then_encrypted_session_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let psk = derive_psk("s3cret");
    let code = "123456".to_string();

    let host_code = code.clone();
    let host = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let probe: DiscoveryMessage = read_framed(&mut sock).await.unwrap();
        let matches = matches!(&probe, DiscoveryMessage::Probe { code, .. } if *code == host_code);
        write_framed(
            &mut sock,
            &DiscoveryMessage::ProbeResult {
                matches,
                host_name: "hote-test".into(),
            },
        )
        .await
        .unwrap();
        assert!(matches);

        let mut enc = EncryptedStream::accept(sock, &psk).await.unwrap();
        enc.send(&HostMessage::ScreenInfo {
            width: 1920,
            height: 1080,
        })
        .await
        .unwrap();

        let msg: ControllerMessage = enc.recv().await.unwrap();
        assert_eq!(msg, ControllerMessage::RequestFullFrame);

        // Gros message (200 KiB) pour forcer la fragmentation sur plusieurs
        // enregistrements Noise (max ~64 KiB chacun).
        let big = vec![7u8; 200_000];
        enc.send(&HostMessage::Frame(FrameUpdate {
            seq: 1,
            tiles: vec![Tile {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                codec: TileCodec::DeflateBgra,
                data: big.clone(),
            }],
        }))
        .await
        .unwrap();
        big.len()
    });

    let mut sock = TcpStream::connect(addr).await.unwrap();
    write_framed(
        &mut sock,
        &DiscoveryMessage::Probe {
            proto_version: PROTO_VERSION,
            code: code.clone(),
        },
    )
    .await
    .unwrap();
    let resp: DiscoveryMessage = read_framed(&mut sock).await.unwrap();
    assert!(matches!(
        resp,
        DiscoveryMessage::ProbeResult { matches: true, .. }
    ));

    let mut enc = EncryptedStream::connect(sock, &psk).await.unwrap();
    let info: HostMessage = enc.recv().await.unwrap();
    assert_eq!(
        info,
        HostMessage::ScreenInfo {
            width: 1920,
            height: 1080
        }
    );

    enc.send(&ControllerMessage::RequestFullFrame)
        .await
        .unwrap();
    let frame: HostMessage = enc.recv().await.unwrap();
    match frame {
        HostMessage::Frame(fu) => {
            assert_eq!(fu.seq, 1);
            assert_eq!(fu.tiles[0].data.len(), 200_000);
        }
        other => panic!("attendu une trame, reçu {other:?}"),
    }

    assert_eq!(host.await.unwrap(), 200_000);
}

/// Livre les octets un par un, avec un `Poll::Pending` entre chaque : simule
/// un réseau réel où un enregistrement arrive fragmenté sur plusieurs réveils,
/// et ouvre une fenêtre d'annulation (`tokio::select!`) à chaque octet.
struct Trickle<S> {
    inner: S,
    armed: bool,
}

impl<S: AsyncRead + Unpin> AsyncRead for Trickle<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.armed {
            self.armed = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let mut byte = [0u8; 1];
        let mut one = ReadBuf::new(&mut byte);
        match Pin::new(&mut self.inner).poll_read(cx, &mut one) {
            Poll::Ready(Ok(())) => {
                self.armed = false;
                buf.put_slice(one.filled());
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Trickle<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Régression : `recv` doit survivre à une annulation par `tokio::select!` au
/// milieu d'un enregistrement. Les boucles de session annulent `recv` en
/// permanence (ticker d'envoi côté hôte, entrées souris côté contrôleur) ;
/// l'ancien code (`read_exact` sur tampons locaux) perdait alors les octets
/// déjà lus, désynchronisait le flux et produisait un « decrypt error » dès
/// que les enregistrements arrivaient fragmentés (réseau réel, pas loopback).
#[tokio::test]
async fn recv_survives_select_cancellation_mid_record() {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let psk = derive_psk("s3cret");

    let host = tokio::spawn(async move {
        let mut enc = EncryptedStream::accept(server, &psk).await.unwrap();
        for seq in 0..5u64 {
            enc.send(&HostMessage::Frame(FrameUpdate {
                seq,
                tiles: vec![Tile {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                    codec: TileCodec::DeflateBgra,
                    data: vec![seq as u8; 300],
                }],
            }))
            .await
            .unwrap();
        }
    });

    let trickle = Trickle {
        inner: client,
        armed: false,
    };
    let mut enc = EncryptedStream::connect(trickle, &psk).await.unwrap();

    tokio::time::timeout(Duration::from_secs(30), async {
        let mut received = 0u64;
        while received < 5 {
            tokio::select! {
                biased;
                msg = enc.recv::<HostMessage>() => {
                    match msg.unwrap() {
                        HostMessage::Frame(fu) => {
                            assert_eq!(fu.seq, received);
                            assert_eq!(fu.tiles[0].data, vec![received as u8; 300]);
                            received += 1;
                        }
                        other => panic!("attendu une trame, reçu {other:?}"),
                    }
                }
                // Branche concurrente qui gagne à répétition : annule `recv`
                // en plein enregistrement.
                _ = tokio::task::yield_now() => {}
            }
        }
    })
    .await
    .expect("timeout : le flux s'est désynchronisé (recv non sûr à l'annulation)");

    host.await.unwrap();
}
