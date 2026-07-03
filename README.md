# ViewerKiller

Alternative à TeamViewer, **plus sécurisée**, pour deux machines qui se
connectent **directement** (IP + port ouvert), sans serveur de rendez-vous.
Conçue pour Windows, écrite en Rust.

[![CI](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml/badge.svg)](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml)

## Pourquoi « plus sécurisé »

- **Chiffrement de bout en bout** (Noise `NNpsk0`, ChaCha20-Poly1305) : personne
  entre l'hôte et le contrôleur ne voit ni l'écran ni les frappes.
- **Authentification par code + mot de passe** intégrée au handshake : un mauvais
  mot de passe empêche toute session (pas seulement « refusée » après coup).
- **Anti-bruteforce** (verrouillage par IP), **consentement** explicite côté hôte,
  **journal d'audit**.

Pas de serveur de rendez-vous à héberger : l'hôte écoute sur un port TCP et
attend, le contrôleur se connecte **directement** à son adresse IP avec un
**code de connexion** + mot de passe (façon TeamViewer, sans découverte réseau).
L'accessibilité de ce port (VPN, réseau local, redirection de port…) reste à la
charge de l'utilisateur.

## Architecture

```
crates/vk-core/      protocole, cadrage des messages, crypto Noise
crates/vk-platform/  capture écran & injection d'entrées (Windows : GDI + SendInput)
crates/vk-net/       transport TCP chiffré
crates/vk-media/     encodage par tuiles (JPEG, diff) + recomposition RGBA
app/viewerkiller/    orchestration hôte/contrôleur, durcissement, CLI + GUI egui
```

## Build

```bash
cargo build --release --workspace
cargo test --workspace            # 28 tests
```

Sur Linux, le code spécifique Windows se vérifie en cross-compilation :

```bash
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --workspace
```

## Utilisation

Sur deux machines qui peuvent se joindre en TCP (même réseau local, VPN, port
redirigé…) :

**GUI** (recommandé) :

```bash
viewerkiller-gui      # côté hôte : « Héberger » → affiche un code + un mot de passe
viewerkiller-gui      # côté contrôleur : « Se connecter » → saisir code + mot de passe + IP de l'hôte
```

L'écran « Héberger » liste les adresses IP locales (Wi-Fi, Ethernet, VPN…) à
communiquer au contrôleur.

**CLI** :

```bash
viewerkiller host [ip[:port]]             # écoute et affiche le code + le mot de passe
viewerkiller connect <code> <mot_de_passe> <ip[:port]>
```

Par défaut l'hôte écoute sur toutes les interfaces (port `47600`) ; le
contrôleur se connecte directement à l'adresse indiquée.

## État

Tous les composants sont implémentés. La chaîne complète est testée sous Linux
(stubs) et le code natif Windows est vérifié à la compilation ; la **validation au
runtime se fait sur Windows**. Détails : voir
[`ETAT-DU-PROJET.md`](ETAT-DU-PROJET.md). Évolutions prévues :
[`FEUILLE-DE-ROUTE.md`](FEUILLE-DE-ROUTE.md).

## Licence

MIT OR Apache-2.0.
