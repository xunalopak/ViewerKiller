# ViewerKiller — état du projet (note de reprise)

> Alternative à TeamViewer, **plus sécurisée**, pour machines reliées par un VPN
> WireGuard. Aucun port exposé vers Internet (l'agent n'écoute que sur l'IP VPN),
> chiffrement bout-en-bout (Noise) par-dessus le VPN, code de connexion + mot de
> passe, découverte par balayage du sous-réseau.

Dernière mise à jour : **2026-06-30**. Plan détaillé :
`~/.claude/plans/j-aimerai-construire-une-alternative-peaceful-castle.md`.

## Décisions verrouillées
- Cible **Windows** (hôte + contrôleur). Cœur en **Rust**.
- Connectivité : IP du **VPN WireGuard**, **sans serveur** ; agent lié à
  l'interface VPN uniquement.
- Code de connexion **arbitraire** + **balayage** du sous-réseau.
- MVP : écran + clavier/souris + chiffrement E2E (code + mot de passe).
- Env. de dev = **Linux** ; capture/injection natives = **Windows** → isolées
  derrière des traits, build/test final sur Windows.

## Avancement par jalon
- [x] **J1 — Squelette + protocole + cadrage** (`crates/vk-core`) — testé.
- [x] **J2 — Crypto Noise NNpsk0 + dérivation PSK** (`crates/vk-core/crypto.rs`) — testé.
- [x] **J3 — Transport TCP chiffré + découverte VPN** (`crates/vk-net`) — testé.
- [x] **J4a — Encodage/diff par tuiles (portable)** (`crates/vk-media`) — testé.
- [ ] **J4b — Capture écran Windows (DXGI)** — `crates/vk-platform/src/windows.rs`
      est un **placeholder** (`WindowsCapturer::new()` renvoie une erreur). À
      implémenter sur Windows.
- [ ] **J5 — Injection clavier/souris Windows (SendInput)** — idem placeholder
      (`WindowsInjector`). À implémenter sur Windows.
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
crates/vk-net/       frame.rs (cadrage async clair), transport.rs (EncryptedStream), discovery.rs (scan)
crates/vk-media/     TileEncoder (diff+JPEG) + FrameBuffer (décode→RGBA)
app/viewerkiller/    lib : host.rs, controller.rs, security.rs
                     bin : main.rs (CLI viewerkiller), gui.rs (viewerkiller-gui, egui)
                     tests : e2e.rs, hardening.rs
```

> Note perf : l'encodage JPEG dans `host_session` est **synchrone** sur le
> runtime ; les binaires utilisent un runtime multi-thread donc OK, mais une
> optimisation future = `spawn_blocking` pour l'encode. Le test e2e force
> `multi_thread` pour éviter la starvation.

## Format réseau (rappel)
1. Découverte (en clair, dans le tunnel VPN) : `DiscoveryMessage::Probe{code}` →
   `ProbeResult{matches}` — cadrage u32 + postcard (`vk_core::codec`).
2. Handshake Noise `NNpsk0` (PSK = `blake3::derive_key(password)`), enregistrements
   `[u16 len][texte chiffré]`.
3. Session : messages applicatifs cadrés u32, fragmentés en enregistrements Noise
   ≤ 65519 o. Hôte→ctrl = `HostMessage` (ScreenInfo, Frame), ctrl→hôte =
   `ControllerMessage` (Input, RequestFullFrame, Bye).

## Build & test
```bash
cargo test --workspace      # 21 tests, tous verts sur Linux
cargo build --workspace
# CLI (Linux, capteur factice via le stub) :
cargo run -p viewerkiller -- host
cargo run -p viewerkiller -- connect <code> <mot_de_passe> [ip/prefixe]
```

## Reprise — prochaines étapes concrètes
1. **J4b/J5 (Windows)** — implémenter `WindowsCapturer` (DXGI Desktop Duplication
   via le crate `windows`, ou `scrap` pour aller vite) et `WindowsInjector`
   (`SendInput` via `enigo` ou le crate `windows`). Dépendances à décommenter dans
   `crates/vk-platform/Cargo.toml` (section `[target.'cfg(windows)'.dependencies]`).
   Mapper coordonnées absolues (MouseMove en coords écran) → unités
   `0..65535` de `SendInput`.
3. **J7 durcissement** — compteur d'échecs de mot de passe + backoff dans
   `host::handle_connection` ; message de consentement (accept/refuse) avant
   `host_session` ; log d'audit (qui/quand).
4. **Test sur Windows** — deux machines du même VPN WireGuard : `host` affiche le
   code, `connect <code> <mdp>`. Vérifier depuis un hôte hors-VPN que le port
   47600 est injoignable (`Test-NetConnection <ip-vpn> -Port 47600`).

## Pièges connus / notes
- `TileCodec` a été renommé `ZstdRgba` → `DeflateBgra` (deflate pur Rust, pas de
  dépendance C). JPEG = chemin par défaut de `TileEncoder`.
- `snow::Builder::psk()` renvoie `Builder` (pas de `?`).
- Pendant le balayage, le contrôleur sonde puis ferme ; l'hôte voit alors un EOF
  au début du handshake → loggé en `debug` (normal).
- `EncryptedStream` utilise un seul `Transport` Noise (nonces séparés par sens) ;
  ne pas tenter de splitter lecture/écriture sur deux tâches sans mutex — la
  boucle de session utilise `tokio::select!`.
