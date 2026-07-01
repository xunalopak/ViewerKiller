# ViewerKiller

Alternative à TeamViewer, **plus sécurisée**, pour des machines d'un **même
réseau local (usage interne)**. Conçue pour Windows, écrite en Rust.

[![CI](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml/badge.svg)](https://github.com/xunalopak/ViewerKiller/actions/workflows/ci.yml)

## Pourquoi « plus sécurisé »

- **Usage strictement interne** : l'agent hôte n'écoute que sur l'adresse de son
  interface réseau locale — pas de serveur de rendez-vous, rien à exposer vers
  Internet.
- **Chiffrement de bout en bout** (Noise `NNpsk0`, ChaCha20-Poly1305) : même un
  autre appareil du LAN qui capterait le trafic ne voit ni l'écran ni les frappes.
- **Authentification par code + mot de passe** intégrée au handshake : un mauvais
  mot de passe empêche toute session (pas seulement « refusée » après coup).
- **Anti-bruteforce** (verrouillage par IP), **consentement** explicite côté hôte,
  **journal d'audit**.

Pas de serveur de rendez-vous à héberger : le réseau local fournit la
connectivité, et le contrôleur retrouve l'hôte par **balayage du sous-réseau** à
partir d'un **code de connexion** arbitraire (façon TeamViewer).

## Architecture

```
crates/vk-core/      protocole, cadrage des messages, crypto Noise
crates/vk-platform/  capture écran & injection d'entrées (Windows : GDI + SendInput)
crates/vk-net/       transport TCP chiffré, découverte par balayage du LAN
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

Sur deux machines du même réseau local :

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

Le sous-réseau est détecté automatiquement depuis l'interface réseau locale ; on
peut le forcer (ex. `192.168.1.0/24`).

## Dépannage

### « decrypt error » / session coupée dès la première image

Handshake OK (donc mots de passe identiques), puis coupure sur le premier gros
message. Sur une **carte réseau virtuelle** (VirtualBox Host-Only, VMware…), la
cause habituelle est la **corruption par déchargement TCP** (LSO/TSO + checksum
offload) : les petits paquets passent, la première image plein écran (grosse,
segmentée) est mal réassemblée → l'AEAD Noise la rejette.

Correctif — désactiver le déchargement sur l'adaptateur concerné, **des deux
côtés** (PowerShell admin) :

```powershell
Get-NetAdapter | Select-Object Name,InterfaceDescription,Status
Disable-NetAdapterLso              -Name "<nom-de-la-carte>"
Disable-NetAdapterChecksumOffload  -Name "<nom-de-la-carte>"
```

### « Aucun hôte ne correspond à ce code »

- Vérifier que les deux machines sont bien sur le **même sous-réseau local** et
  se pinguent.
- Sur une machine à plusieurs cartes, forcer le sous-réseau (ex.
  `192.168.1.0/24`) dans le champ *Sous-réseau* de la GUI ou en argument de la
  CLI.

## État

Tous les composants sont implémentés. La chaîne complète est testée sous Linux
(stubs) et le code natif Windows est vérifié à la compilation ; la **validation au
runtime se fait sur Windows**. Détails et feuille de route : voir
[`ETAT-DU-PROJET.md`](ETAT-DU-PROJET.md).

## Licence

MIT OR Apache-2.0.
