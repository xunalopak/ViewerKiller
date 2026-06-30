# ViewerKiller

Alternative à TeamViewer, **plus sécurisée**, pour des machines reliées par un
**VPN WireGuard**. Conçue pour Windows, écrite en Rust.

[![CI](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml/badge.svg)](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml)

## Pourquoi « plus sécurisé »

- **Aucun port exposé vers Internet** : l'agent hôte n'écoute que sur l'adresse
  de l'interface VPN. Vu d'Internet, rien n'est ouvert.
- **Chiffrement de bout en bout** (Noise `NNpsk0`, ChaCha20-Poly1305) **par-dessus
  le VPN** : même un nœud VPN compromis ne voit ni l'écran ni les frappes.
- **Authentification par code + mot de passe** intégrée au handshake : un mauvais
  mot de passe empêche toute session (pas seulement « refusée » après coup).
- **Anti-bruteforce** (verrouillage par IP), **consentement** explicite côté hôte,
  **journal d'audit**.

Pas de serveur de rendez-vous à héberger : le VPN fournit la connectivité, et le
contrôleur retrouve l'hôte par **balayage du sous-réseau** à partir d'un **code de
connexion** arbitraire (façon TeamViewer).

## Architecture

```
crates/vk-core/      protocole, cadrage des messages, crypto Noise
crates/vk-platform/  capture écran & injection d'entrées (Windows : GDI + SendInput)
crates/vk-net/       transport TCP chiffré, découverte par balayage VPN
crates/vk-media/     encodage par tuiles (JPEG, diff) + recomposition RGBA
app/viewerkiller/    orchestration hôte/contrôleur, durcissement, CLI + GUI egui
```

## Build

```bash
cargo build --release --workspace
cargo test --workspace            # 26 tests
```

Sur Linux, le code spécifique Windows se vérifie en cross-compilation :

```bash
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --workspace
```

## Utilisation

Sur deux machines du même VPN WireGuard :

**GUI** (recommandé) :

```bash
viewerkiller-gui      # côté hôte : « Héberger » → affiche un code + un mot de passe
viewerkiller-gui      # côté contrôleur : « Se connecter » → saisir code + mot de passe
```

**CLI** :

```bash
viewerkiller host                         # affiche le code + le mot de passe
viewerkiller connect <code> <mot_de_passe> [ip/prefixe]
```

Le sous-réseau est détecté automatiquement depuis l'interface VPN ; on peut le
forcer (ex. `10.0.0.0/24`).

## État

Tous les composants sont implémentés. La chaîne complète est testée sous Linux
(stubs) et le code natif Windows est vérifié à la compilation ; la **validation au
runtime se fait sur Windows**. Détails et feuille de route : voir
[`ETAT-DU-PROJET.md`](ETAT-DU-PROJET.md).

## Licence

MIT OR Apache-2.0.
