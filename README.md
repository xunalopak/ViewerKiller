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

## Dépannage

### « Connexion établie » côté hôte, mais « Session terminée » côté contrôleur

Symptôme typique sur deux machines reliées par WireGuard : l'hôte affiche
*session établie*, puis le contrôleur se déconnecte aussitôt. La découverte et le
handshake (petits paquets) passent, mais la **première image** (gros paquets)
est perdue : c'est un **trou noir MTU** dans le tunnel.

Confirmer depuis le contrôleur (l'un des deux petits pings répond, le gros avec
« interdiction de fragmenter » échoue) :

```powershell
ping <IP-VPN-hôte>                 # petit paquet : doit répondre
ping <IP-VPN-hôte> -f -l 1400      # gros paquet, DF : échoue si MTU trop bas
ping <IP-VPN-hôte> -l 1400         # gros paquet fragmenté : répond
```

Correctif : réduire le MTU WireGuard sur les **deux** configs (`[Interface]`) :

```ini
[Interface]
MTU = 1380      # descendre à 1280 si nécessaire (4G, PPPoE, double tunnel…)
```

Depuis la 0.1.1, la vraie cause de coupure est remontée jusqu'à l'UI
(« Session terminée : réception interrompue : … ») et la connexion applique un
délai maximal au lieu de bloquer indéfiniment.

### « Aucun hôte ne correspond à ce code »

- Vérifier que les deux machines sont bien sur le VPN et se pinguent.
- Si l'interface WireGuard du contrôleur est en `/32`, le balayage ne couvre que
  sa propre adresse : forcer le sous-réseau (ex. `10.0.0.0/24`) dans le champ
  *Sous-réseau* de la GUI ou en argument de la CLI.

## État

Tous les composants sont implémentés. La chaîne complète est testée sous Linux
(stubs) et le code natif Windows est vérifié à la compilation ; la **validation au
runtime se fait sur Windows**. Détails et feuille de route : voir
[`ETAT-DU-PROJET.md`](ETAT-DU-PROJET.md).

## Licence

MIT OR Apache-2.0.
