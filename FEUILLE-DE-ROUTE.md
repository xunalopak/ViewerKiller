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

- [x] `Probe.proto_version` : l'hôte refuse désormais une version incompatible
      et la journalise (`handle_connection`, host.rs) — fait en v0.1.6. (Le
      contrôleur reçoit encore un « code non reconnu » générique : un message
      dédié demanderait un champ de plus dans `ProbeResult`.)
- [ ] Molette horizontale ignorée (`mouse_scroll` jette `dx`,
      vk-platform/windows.rs → `MOUSEEVENTF_HWHEEL`).
- [ ] Le contrôleur n'envoie pas `Bye` quand l'UI ferme `events_rx`
      (controller.rs, branche `is_err()`).
- [ ] fps/qualité réglables dans la GUI (constantes en dur : 15 fps,
      qualité 75).
