//! Tests d'intégration en boucle locale : sonde de vérification de code,
//! handshake Noise sur TCP réel, et échange chiffré avec fragmentation d'un
//! message > 64 KiB.

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
