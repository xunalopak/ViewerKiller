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
- [ ] (**J10b**) fps/qualité *dynamiques* (mesure de latence) — au-delà du
      simple Skip.

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

## J12 — Multi-écrans + curseur distant

- [ ] Énumération des moniteurs, choix côté contrôleur (message de sélection).
- [ ] Forme réelle du curseur (`CURSORINFO` / DXGI pointer shape) dessinée
      côté contrôleur.

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

### J16b — Téléchargement + remplacement

- [ ] Télécharger l'asset, **vérifier le SHA256** avant tout (intégrité).
- [ ] Swap Windows : renommer l'exe courant en `.old` (un exe en cours ne peut
      être écrasé), écrire le neuf, relancer, purger le `.old` au démarrage
      suivant. Fonctionne sans UAC tant que la distribution reste un zip
      portable en dossier utilisateur.
- [ ] Gérer les **deux** binaires (`viewerkiller.exe`, `viewerkiller-gui.exe`).

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
- [ ] Molette horizontale ignorée (`mouse_scroll` jette `dx`,
      vk-platform/windows.rs → `MOUSEEVENTF_HWHEEL`).
- [x] Le contrôleur envoie `Bye` à la fermeture locale (v0.1.9) : `input_rx`
      clos → `Bye` puis `SessionEnd::UserQuit` (chemin « Déconnecter » de la
      GUI, qui lâche `input_tx`). La fermeture de `events_rx` seule renvoie
      `UserQuit` sans `Bye`, mais la GUI lâche toujours les deux ensemble.
- [ ] fps/qualité réglables dans la GUI (constantes en dur : 15 fps,
      qualité 75).
