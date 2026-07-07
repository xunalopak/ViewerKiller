# ViewerKiller — feuille de route

> Évolutions issues de la revue de code de la v0.1.4. Cible d'usage : machines
> qui se joignent déjà en TCP (**LAN / VPN**) — la traversée de NAT est
> volontairement différée. Priorité au confort d'usage et à la fluidité.

Les jalons J1–J7 (MVP complet) sont documentés dans
[`ETAT-DU-PROJET.md`](ETAT-DU-PROJET.md).

## J8 — Clavier complet — **fait (v0.1.5)**

Avant : `egui_key_to_vk` (gui.rs) ignorait les modificateurs et il n'y avait
pas d'injection de texte Unicode : pas de majuscules, pas de Ctrl+C, pas
d'accents.

- [x] `InputEvent::Char { c: char }` (fin d'enum — postcard encode le
      discriminant par ordre de déclaration) ; `PROTO_VERSION` → 2.
- [x] `InputInjector::char_input` : `SendInput` + `KEYEVENTF_UNICODE`
      (paire de surrogates hors BMP) ; stub Linux.
- [x] GUI : `egui::Event::Text` → `Char` ; suivi des transitions de
      modificateurs (VK_SHIFT/CONTROL/MENU), relâchés à la déconnexion ;
      touches imprimables envoyées en VK seulement sous Ctrl/Alt (raccourcis),
      sauf AltGr (= Ctrl+Alt) qui passe par `Text`. (Pas de touche Win : egui
      ne l'expose pas comme modificateur sous Windows.)

## J9 — Consentement + indicateur en GUI — **fait (v0.1.6)**

Avant : l'écran « Héberger » utilisait `AutoAccept` — quiconque avait code +
mot de passe prenait la main sans que l'hôte ne voie rien.

- [x] Impl `Consent` (`GuiConsent`, gui.rs) branchée sur une boîte de dialogue
      egui (canal tokio→UI + oneshot retour, timeout 30 s = refus) ;
      `require_consent: true`. Trait `Consent` doté d'un `session_ended` (défaut
      no-op) pour retirer l'indicateur.
- [x] Bannière « 🔴 Session en cours depuis <IP> » sur l'écran hôte ; « Retour »
      arrête l'hébergement (et coupe la session en cours).

## J10 — Fluidité — **partiel (v0.1.8)**

- [x] `spawn_blocking` autour de `encoder.encode(...)` dans `host_session`
      (host.rs) — l'encodage JPEG synchrone ne monopolise plus un thread
      ouvrier du runtime ; l'encodeur (état inter-trames) est déplacé dans la
      tâche puis récupéré.
- [x] Cadence adaptative : `MissedTickBehavior::Skip` sur le ticker de trames
      (et le ticker presse-papiers) — sous charge, on saute les ticks manqués
      au lieu de les rattraper en rafale, donc pas d'accumulation de retard.
- [x] **Qualité adaptative (v0.1.19)** : `QualityController` (vk-media) mesure le
      temps réel de chaque cycle (capture + encode + envoi) vs la période cible ;
      s'il déborde (réseau lent / backpressure), la qualité JPEG baisse (jusqu'à
      `MIN_QUALITY`), et remonte vers le max quand la marge revient (hystérésis).
      Logique pure testée. (Reste J10b-capture : DXGI ci-dessous.)

## J10b — Capture DXGI Desktop Duplication

Reporté : ~300 lignes de COM `unsafe` (crate `windows`,
`IDXGIOutputDuplication`) non compilables/testables sans machine Windows. À
faire quand un build/test Windows local sera disponible.

- [ ] Capture DXGI avec *dirty rects* natifs ; repli sur `WindowsCapturer`
      (GDI) actuel si indisponible ou sur `DXGI_ERROR_ACCESS_LOST`.

## J11 — Presse-papiers partagé — **fait (v0.1.7)**

Copier/coller bidirectionnel façon RDP : ce qu'on copie d'un côté se colle de
l'autre.

- [x] Messages `HostMessage::Clipboard(String)` /
      `ControllerMessage::Clipboard(String)` ; `PROTO_VERSION` → 3.
- [x] Trait `vk_platform::Clipboard` (impl Windows via `arboard`, stub ailleurs)
      + helper `ClipboardSync` anti-boucle (sondage 500 ms). Activé par
      `HostConfig.share_clipboard` (hôte) et le paramètre `share_clipboard` de
      `controller_session` ; désactivé dans les tests.

## J12 — Multi-écrans + curseur distant — **fait (v0.1.14 + v0.1.17)**

- [x] Énumération des moniteurs (Windows `EnumDisplayMonitors`) + choix côté
      contrôleur : `HostMessage::Monitors` en début de session,
      `ControllerMessage::SelectMonitor { index }` ; `PROTO_VERSION` → 5. Bascule
      à chaud (nouvelle géométrie → `TileEncoder` repart en trame pleine).
      Sélecteur d'écran dans la barre de session GUI (affiché si >1 moniteur).
      Capture du rectangle du moniteur dans le bureau virtuel (`BitBlt`). Trait
      `ScreenCapturer::monitors`/`select_monitor` (défaut mono-écran). Testé via
      stub 2 moniteurs (`tests/multiscreen.rs`, bascule end-to-end) ; **la capture
      Windows multi-moniteur reste à valider au runtime**.
- [x] **Curseur distant (v0.1.17)** : plutôt que d'extraire le bitmap du curseur
      (fragile), l'hôte détecte le **type** de curseur (`GetCursorInfo` + comparaison
      aux curseurs système `IDC_*`) et l'envoie (`HostMessage::Cursor`, protocole
      → 6). Le contrôleur adapte son **curseur local** au survol de l'image
      (`cursor_icon_of` → `egui::CursorIcon`) : texte, main, redimensionnement… →
      **sémantique, sans latence, sans bitmap**. Mapping pur testé + round-trip codec.

## J13 — Reconnexion & robustesse — **fait (v0.1.9)**

Une coupure (VPN qui tombe, machine mise en veille) laissait auparavant la
session figée indéfiniment : TCP met très longtemps à signaler un pair disparu.

- [x] Keepalive protocolaire : `ControllerMessage::Ping` / `HostMessage::Ping`
      (fin d'enum) ; `PROTO_VERSION` → 4. Chaque pair émet un Ping toutes les
      `KEEPALIVE_INTERVAL` (5 s) quand il n'a rien d'autre à envoyer.
- [x] Chien de garde : sans **aucun** message reçu pendant `SESSION_TIMEOUT`
      (15 s), la session est fermée des deux côtés (`host_session` et
      `controller_session`) — détection en ~15-20 s au lieu de plusieurs minutes.
- [x] Reconnexion automatique côté contrôleur : `controller_session` renvoie un
      `SessionEnd` (`UserQuit` / `HostClosed` / `Dropped`) ; l'orchestrateur
      `run_controller` relance `connect_to` (même adresse + mêmes identifiants)
      avec backoff exponentiel (`ReconnectPolicy` : 1→10 s, 10 essais par
      défaut). Une fin propre (`Bye`) ou locale ne déclenche pas de reconnexion.
- [x] GUI : bannière « ⟳ Connexion perdue — reconnexion… » via
      `SessionEvent::Reconnecting` ; l'écran distant figé reste affiché et la
      session reprend d'elle-même, sans ressaisir code/mot de passe.
- [x] Test d'intégration `controller_reconnects_after_drop` (coupure simulée →
      reconnexion → reprise) + round-trip codec des `Ping`.
- [x] Fin de session robuste côté hôte (v0.1.11) : une perte de connexion
      (fermeture, reset réseau) termine la session proprement au lieu de propager
      une erreur. Sous Windows, un RST à la fermeture peut effacer le `Bye` en vol
      (aggravé par le `Ping` keepalive laissé non lu) — l'hôte le traite désormais
      comme une fin normale. Régression : `abrupt_controller_disconnect_completes_gracefully`.

## J14 — Codec vidéo H.264

Le vrai saut de fluidité plein écran (bande passante ÷ 10-20 vs tuiles JPEG).
Gros chantier, à faire après J10.

- [ ] Encodeur Media Foundation (MFT) côté hôte, décodeur côté contrôleur.
- [ ] Négociation via `proto_version`, repli tuiles JPEG.

## J15 — Intégration Windows

- [ ] Icône + installeur (winget/MSI), démarrage avec Windows en mode hôte.
- [ ] Accès non surveillé : mot de passe fixe optionnel stocké via DPAPI.
- [ ] Service Windows + UAC/bureau sécurisé (`SetThreadDesktop`) pour
      contrôler les écrans d'élévation.
- [ ] Logs fichier + panneau de diagnostic GUI (la console GUI a été retirée
      en v0.1.4).

## J16 — Auto-update depuis les releases GitHub

Résout la corvée récurrente : chaque bump de `PROTO_VERSION` impose aujourd'hui
de mettre à jour les **deux** machines à la main. Vérification via l'API GitHub
Releases (repo public, `releases/latest` sans authentification), comparaison au
`env!("CARGO_PKG_VERSION")` courant.

Approche retenue : **vérification au lancement + application manuelle** (un clic
« Mettre à jour » en GUI, sous-commande `viewerkiller update` en CLI) plutôt
qu'un remplacement silencieux — pour un outil de contrôle à distance,
l'utilisateur garde la main. Mini-updater maison avec `ureq` (HTTP léger)
plutôt que `self_update` (tire reqwest/tokio/TLS), pour des dépendances
maîtrisées et un chemin auditable.

### J16a — Notification de version (zéro risque) — **fait (v0.1.10)**

- [x] `release.yml` publie un `SHA256SUMS.txt` des trois assets (format
      `sha256sum -c`, prépare la vérification d'intégrité de J16b).
- [x] Au lancement, requête `releases/latest` (module `update.rs`, `ureq` +
      `native-tls`) → bandeau « ⬆ nouvelle version disponible » en GUI, ligne
      `ℹ` en CLI. Purement informatif, **aucun téléchargement**. Exécuté hors du
      fil principal (thread dédié / `spawn_blocking`), délai 5 s, **silencieux
      hors ligne** (VPN isolé) — ne bloque jamais le démarrage.
- Choix TLS : `native-tls` (OpenSSL sur Linux, SChannel sur Windows) plutôt que
  rustls/ring, pour garder le cross-check Windows depuis Linux (pas de C).

### J16b — Téléchargement + remplacement — **fait (v0.1.18)**

- [x] `download_and_verify` : télécharge l'asset via l'API Releases et **vérifie
      son SHA256** contre `SHA256SUMS.txt` **avant** toute utilisation (crate
      `sha2`). Parsing + hash = fonctions pures testées.
- [x] Swap : renomme l'exe courant en `.old` (un exe en cours ne peut être
      écrasé mais peut être renommé), écrit le neuf, relance, purge le `.old` au
      démarrage suivant (`cleanup_old_update`). Restaure l'ancien si l'écriture
      échoue.
- [x] Les **deux** binaires : bouton « ⬇ Mettre à jour maintenant » sur l'accueil
      GUI (asset `viewerkiller-gui.exe`) ; sous-commande `viewerkiller update`
      (asset `viewerkiller.exe`). Application **manuelle** (jamais silencieuse).
- Durcissement restant (post-jalon) : **signature minisign** — le SHA256 vient de
  la même release, il protège l'intégrité du transfert mais pas contre une release
  compromise ; une signature à clé publique embarquée le ferait.

### Points de vigilance sécurité

- Exécuter du code téléchargé = surface d'attaque réelle pour un outil qui se
  veut sécurisé. TLS (API GitHub) + **checksum obligatoire** dès J16b.
- Durcissement ultérieur : **signature minisign/cosign** (clé publique embarquée
  dans le binaire) pour l'intégrité indépendamment de GitHub ; signature
  Authenticode pour SmartScreen.

## Différé (hors cible LAN/VPN)

- IPv6 (tout est IPv4, y compris `local_ipv4_addresses`).
- Découverte LAN opt-in par mDNS (multicast propre, pas de balayage).
- Traversée de NAT / serveur de rendez-vous auto-hébergé.

## Petits correctifs au fil de l'eau (repérés en revue)

- [x] `Probe.proto_version` : l'hôte refuse désormais une version incompatible
      et la journalise (`handle_connection`, host.rs) — fait en v0.1.6. (Le
      contrôleur reçoit encore un « code non reconnu » générique : un message
      dédié demanderait un champ de plus dans `ProbeResult`.)
- [x] Molette horizontale (v0.1.12) : `mouse_scroll` transmet `dx` via
      `MOUSEEVENTF_HWHEEL` (vk-platform/windows.rs).
- [x] Le contrôleur envoie `Bye` à la fermeture locale (v0.1.9) : `input_rx`
      clos → `Bye` puis `SessionEnd::UserQuit` (chemin « Déconnecter » de la
      GUI, qui lâche `input_tx`). La fermeture de `events_rx` seule renvoie
      `UserQuit` sans `Bye`, mais la GUI lâche toujours les deux ensemble.
- [x] fps/qualité réglables dans la GUI (v0.1.12) : section « ⚙ Réglages
      d'hébergement » sur l'accueil (sliders 5–30 img/s, qualité 40–95),
      appliqués au démarrage de l'hébergement.
- [x] Copier/couper/coller (v0.1.13) : egui-winit pousse `Event::Copy`/`Cut`/
      `Paste` **à la place** de `Event::Key{C/X/V}` (retour anticipé) et supprime
      le `Text` ; ces événements étaient ignorés → **Ctrl+C/X/V ne passaient
      jamais** à l'hôte. Désormais rejoués explicitement (la lettre, Ctrl déjà
      tenu ; le collage s'appuie sur le presse-papiers hôte synchronisé).
      Traduction clavier extraite dans `translate_key_event` + 6 tests unitaires.
- [x] **Flèches + F1-F12 (v0.1.15)** : injection avec **scan code**
      (`MapVirtualKeyW`) + **`KEYEVENTF_EXTENDEDKEY`** pour les touches étendues
      (flèches, Inser/Suppr, Début/Fin, Page préc./suiv.). Avant, `wScan = 0` sans
      drapeau étendu faisait interpréter les flèches comme le pavé numérique (pas
      de sélection Maj+flèche) et certaines apps ignoraient les F-keys.
- [x] **Capture clavier exclusive (v0.1.15)** : en session, les événements
      clavier sont retirés de la file egui **avant** de dessiner l'UI (`num_presses`
      lit `events`). Sinon Tab déplaçait le focus vers « Déconnecter » et Entrée
      l'activait → **Tab+Entrée fermait la session**.
- [x] **Refus de consentement (v0.1.15)** : le contrôleur ne reconnecte plus quand
      la session n'a **jamais été établie** (aucun `ScreenInfo`). Sous Windows, le
      RST à la fermeture effaçait le `Bye` de refus → le contrôleur voyait une
      coupure → reconnexion → **la pop-up de demande rebondissait**. Délai
      d'attente pré-établissement allongé (45 s) pour couvrir le consentement.
      Régression : `no_reconnect_when_session_never_established`.
- [x] **Touches système (v0.1.16)** : Alt+Tab, touche Windows, `Win+<x>`, Alt+Échap,
      Alt+F4, Ctrl+Échap sont captées par un **hook clavier bas niveau**
      (`WH_KEYBOARD_LL`, thread dédié + boucle de messages, `vk-platform/windows.rs`)
      **uniquement en session au premier plan**, supprimées en local et relayées à
      l'hôte. Logique de sélection pure et testée (`should_capture_system_key`,
      exclut AltGr/modificateurs nus). **Reste hors de portée** : `Ctrl+Alt+Suppr`
      (SAS / bureau sécurisé, non captable par un hook standard).
