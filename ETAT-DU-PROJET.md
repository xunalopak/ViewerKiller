# ViewerKiller — état du projet (note de reprise)

> Alternative à TeamViewer, **plus sécurisée**, pour deux machines qui se
> connectent **directement** par IP + port, sans serveur de rendez-vous.
> Chiffrement bout-en-bout (Noise), code de connexion + mot de passe.

Dernière mise à jour : **2026-07-04** (v0.1.10, J16a notification de MàJ). Plan
détaillé : `~/.claude/plans/j-aimerai-construire-une-alternative-peaceful-castle.md`.

## Décisions verrouillées
- Cible **Windows** (hôte + contrôleur). Cœur en **Rust**.
- Connectivité : **connexion TCP directe** à une adresse IP fournie par
  l'utilisateur, **sans serveur** ; l'accessibilité du port (VPN, LAN, port
  forwarding…) reste à sa charge. Pas de balayage réseau ni de détection
  d'interface.
- Code de connexion **arbitraire** + mot de passe, vérifiés au handshake.
- MVP : écran + clavier/souris + chiffrement E2E (code + mot de passe).
- Env. de dev = **Linux** ; capture/injection natives = **Windows** → isolées
  derrière des traits, build/test final sur Windows.

## Avancement par jalon
- [x] **J1 — Squelette + protocole + cadrage** (`crates/vk-core`) — testé.
- [x] **J2 — Crypto Noise NNpsk0 + dérivation PSK** (`crates/vk-core/crypto.rs`) — testé.
- [x] **J3 — Transport TCP chiffré** (`crates/vk-net`) — testé. (La découverte
      par balayage du sous-réseau VPN a été retirée : connexion directe à une
      adresse fournie par l'utilisateur.)
- [x] **J4a — Encodage/diff par tuiles (portable)** (`crates/vk-media`) — testé.
- [x] **J4b — Capture écran Windows** — `WindowsCapturer` via **GDI BitBlt**
      (crate `windows`) dans `crates/vk-platform/src/windows.rs`.
      **Vérifié à la compilation** en cross-build Windows ; reste à tester au
      runtime sur Windows. DXGI = optimisation perf future.
- [x] **J5 — Injection clavier/souris Windows** — `WindowsInjector` via
      **`SendInput`** (souris absolue 0..65535, molette, touches VK). Vérifié à
      la compilation en cross-build ; reste à tester au runtime.
- [x] **J6 — UI egui** (`app/viewerkiller/src/gui.rs`, bin `viewerkiller-gui`) —
      **compile** sur Linux ; exécution nécessite un affichage. Accueil, écran
      hôte (code+mdp), session contrôleur (rendu texture + capture souris/clavier
      → `InputEvent`). egui/eframe **épinglés en 0.29** (les 0.3x ont refondu
      l'API ; mettre à jour plus tard si besoin).
- [x] **J7 — orchestration + durcissement** (`app/viewerkiller`) — testé e2e
      headless (`tests/e2e.rs`) et durcissement (`tests/hardening.rs`,
      `security.rs`) : anti-bruteforce par IP, consentement, audit.

## Carte des crates
```
crates/vk-core/      protocole (protocol.rs), cadrage (codec.rs), crypto Noise (crypto.rs)
crates/vk-platform/  traits ScreenCapturer/InputInjector ; stub (Linux), windows.rs (placeholder)
crates/vk-net/       frame.rs (cadrage async clair), transport.rs (EncryptedStream)
crates/vk-media/     TileEncoder (diff+JPEG) + FrameBuffer (décode→RGBA)
app/viewerkiller/    lib : host.rs, controller.rs, security.rs
                     bin : main.rs (CLI viewerkiller), gui.rs (viewerkiller-gui, egui)
                     tests : e2e.rs, hardening.rs, reconnect.rs
```

> Note perf (v0.1.8, J10) : l'encodage JPEG de `host_session` tourne désormais
> dans `spawn_blocking` (ne monopolise plus un thread ouvrier) et le ticker de
> trames est en `MissedTickBehavior::Skip` (cadence adaptative, pas
> d'accumulation sous charge). Optimisation restante = capture DXGI (J10b).

> Note robustesse (v0.1.9, J13) : keepalive `Ping` bidirectionnel +
> chien de garde `SESSION_TIMEOUT` (15 s) des deux côtés, et reconnexion
> automatique côté contrôleur (`run_controller` + `ReconnectPolicy`, backoff
> exponentiel). Voir [`FEUILLE-DE-ROUTE.md`](FEUILLE-DE-ROUTE.md) J13.

> Note mise à jour (v0.1.10, J16a) : `update.rs` interroge l'API GitHub Releases
> au lancement (`ureq` + `native-tls`, hors fil principal, silencieux hors ligne)
> et signale une version plus récente (bandeau GUI / ligne CLI) — informatif, sans
> téléchargement. `release.yml` publie un `SHA256SUMS.txt` (prépare J16b).

> Note multi-écrans (v0.1.14, J12) : l'hôte annonce ses moniteurs
> (`HostMessage::Monitors`, `EnumDisplayMonitors` sous Windows) ; le contrôleur en
> choisit un (`ControllerMessage::SelectMonitor`, sélecteur dans la barre GUI) et
> l'hôte bascule la capture à chaud. Trait `ScreenCapturer::monitors`/
> `select_monitor` (défaut mono-écran, stub Linux). **Capture Windows
> multi-moniteur à valider au runtime.** Le curseur distant reste à faire (J12).

## Format réseau (rappel)
1. Connexion TCP directe du contrôleur vers `ip:port` de l'hôte, puis
   vérification du code (en clair) : `DiscoveryMessage::Probe{code}` →
   `ProbeResult{matches}` — cadrage u32 + postcard (`vk_core::codec`).
2. Handshake Noise `NNpsk0` (PSK = `blake3::derive_key(password)`), enregistrements
   `[u16 len][texte chiffré]`.
3. Session : messages applicatifs cadrés u32, fragmentés en enregistrements Noise
   ≤ 65519 o. Hôte→ctrl = `HostMessage` (ScreenInfo, Frame, Clipboard, Ping,
   Monitors), ctrl→hôte = `ControllerMessage` (Input, RequestFullFrame, Clipboard,
   Ping, SelectMonitor, Bye). Ping toutes les 5 s ; session fermée si le pair est
   muet > 15 s. `Monitors` annonce les écrans, `SelectMonitor` en bascule (J12).

## Build & test
```bash
cargo test --workspace      # 45 tests, tous verts sur Linux
cargo build --workspace
# Vérif du code Windows (#[cfg(windows)]) sans machine Windows, type-check seul :
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --workspace
# CLI (Linux, capteur factice via le stub) :
cargo run -p viewerkiller -- host
cargo run -p viewerkiller -- connect <code> <mot_de_passe> <ip[:port]>
```

## Reprise — prochaines étapes concrètes
Les évolutions (J8 et suivants) sont planifiées dans
[`FEUILLE-DE-ROUTE.md`](FEUILLE-DE-ROUTE.md). Tous les jalons MVP sont codés et
vérifiés (tests Linux + cross-check Windows). Reste la **validation runtime**
et la perf :
1. **Test runtime sur Windows** — `cargo run --bin viewerkiller-gui` (ou la CLI)
   sur deux machines qui peuvent se joindre en TCP (LAN, VPN, port forwardé…).
   L'hôte affiche le code+mdp et écoute ; le contrôleur saisit code+mdp+IP →
   écran distant + contrôle. Points à valider en conditions réelles : capture
   GDI (couleurs BGRA, stride), `SendInput` (coords absolues, molette).
2. **Perf** — encoder dans un `spawn_blocking` (l'encode JPEG est synchrone) ;
   envisager DXGI Desktop Duplication + un codec vidéo (H.264/VP9) pour le plein
   écran fluide ; tuiles natives « dirty rects ».

## Pièges connus / notes
- **PROTO_VERSION = 5** : v0.1.5 = `InputEvent::Char` (texte Unicode, J8) ;
  v0.1.7 = messages `Clipboard` (presse-papiers, J11) ; v0.1.9 = messages `Ping`
  keepalive (J13) ; v0.1.14 = `Monitors` / `SelectMonitor` (multi-écrans, J12).
  Les deux machines doivent exécuter la même version. Les nouveaux variants d'enum
  s'ajoutent **en fin** (postcard encode le discriminant par ordre de déclaration) ;
  l'hôte refuse et journalise une version incompatible.
- `TileCodec` a été renommé `ZstdRgba` → `DeflateBgra` (deflate pur Rust, pas de
  dépendance C). JPEG = chemin par défaut de `TileEncoder`.
- `snow::Builder::psk()` renvoie `Builder` (pas de `?`).
- La découverte par balayage de sous-réseau (`vk_net::discovery`, dépendance
  `if-addrs`) a été retirée : le contrôleur se connecte directement à l'IP
  fournie par l'utilisateur. L'hôte écoute par défaut sur `0.0.0.0:47600`.
- `EncryptedStream` utilise un seul `Transport` Noise (nonces séparés par sens) ;
  ne pas tenter de splitter lecture/écriture sur deux tâches sans mutex — la
  boucle de session utilise `tokio::select!`.
- **`recv` doit rester sûr vis-à-vis de l'annulation** : l'état de lecture
  (tampon `rx`) vit dans `EncryptedStream`, pas dans le futur. Ne jamais
  revenir à un `read_exact` sur tampons locaux : un `recv` annulé par
  `select!` (ticker hôte, entrées contrôleur) perdrait les octets déjà lus et
  produirait des « decrypt error » aléatoires — uniquement sur réseau réel,
  jamais en loopback (enregistrements livrés entiers). Bug v0.1.3, régression
  couverte par `recv_survives_select_cancellation_mid_record`.
- Capture Windows = **GDI BitBlt** (pas DXGI) : `scrap` ne cross-compile pas
  (backend X11 mal gardé). `WindowsCapturer` est marqué `unsafe impl Send`
  (handles GDI mono-thread).
- egui/eframe **0.29** (les 0.3x ont refondu l'API : `App::update`→`App::ui`,
  etc.) — ne pas monter de version sans réécrire `gui.rs`.
