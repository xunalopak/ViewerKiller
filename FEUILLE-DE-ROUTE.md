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

## J9 — Consentement + indicateur en GUI

L'écran « Héberger » utilise `AutoAccept` : quiconque a code + mot de passe
prend la main sans que l'hôte ne voie rien.

- [ ] Impl `Consent` (security.rs) branchée sur une boîte de dialogue egui
      (canal GUI↔tokio, timeout ~30 s = refus) ; `require_consent: true`.
- [ ] Bandeau « session en cours depuis <IP> » + bouton pour couper.

## J10 — Fluidité

- [ ] `spawn_blocking` autour de `encoder.encode(...)` dans `host_session`
      (host.rs) — l'encodage JPEG est synchrone sur le runtime.
- [ ] Capture **DXGI Desktop Duplication** (crate `windows`,
      `IDXGIOutputDuplication`) avec *dirty rects* natifs ; repli GDI
      (`WindowsCapturer` actuel) si indisponible.
- [ ] fps/qualité adaptatifs simples (baisser la cadence quand l'envoi
      précédent n'est pas terminé).

## J11 — Presse-papiers partagé

- [ ] Messages `HostMessage::Clipboard(String)` /
      `ControllerMessage::Clipboard(String)`.
- [ ] Crate `arboard` des deux côtés, synchronisation sur changement
      (sondage ~500 ms).

## J12 — Multi-écrans + curseur distant

- [ ] Énumération des moniteurs, choix côté contrôleur (message de sélection).
- [ ] Forme réelle du curseur (`CURSORINFO` / DXGI pointer shape) dessinée
      côté contrôleur.

## J13 — Reconnexion & robustesse

- [ ] Retry automatique côté contrôleur (backoff, mêmes identifiants).
- [ ] Ping/keepalive protocolaire + timeouts de session.

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

## Différé (hors cible LAN/VPN)

- IPv6 (tout est IPv4, y compris `local_ipv4_addresses`).
- Découverte LAN opt-in par mDNS (multicast propre, pas de balayage).
- Traversée de NAT / serveur de rendez-vous auto-hébergé.

## Petits correctifs au fil de l'eau (repérés en revue)

- [ ] `Probe.proto_version` ignoré par l'hôte (`handle_connection`, host.rs) →
      répondre « version incompatible » proprement.
- [ ] Molette horizontale ignorée (`mouse_scroll` jette `dx`,
      vk-platform/windows.rs → `MOUSEEVENTF_HWHEEL`).
- [ ] Le contrôleur n'envoie pas `Bye` quand l'UI ferme `events_rx`
      (controller.rs, branche `is_err()`).
- [ ] fps/qualité réglables dans la GUI (constantes en dur : 15 fps,
      qualité 75).
