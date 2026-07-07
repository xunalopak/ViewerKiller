# ViewerKiller — recette runtime (à jouer sur 2 PC)

> Cible : **v0.1.17** (protocole **v6**). Toute la chaîne est vérifiée en CI
> (compilation + tests Linux/Windows), mais la capture d'écran, l'injection
> clavier/souris et le presse-papiers ne se valident qu'**en vrai, sur Windows**.
> Rejouer cette liste à chaque release. Coche au fur et à mesure ; note le
> numéro du test qui échoue et ce que tu as observé.

## Préparatifs

- [ ] **Même version sur les deux PC** : vérifie le **tag de la release**
      téléchargée (les binaires n'ont pas encore de flag `--version` — je peux
      l'ajouter si utile). Protocole v4 → une version au protocole différent en
      face **refusera** la connexion, c'est normal.
- [ ] Les deux PC peuvent se joindre en TCP (même LAN, ou VPN, ou port
      redirigé). Récupère l'IP de la machine **hôte** : elle est affichée sur
      l'écran « Héberger » (Wi-Fi et Ethernet).
- [ ] Rôles : **PC-A = hôte** (celui qu'on contrôle), **PC-B = contrôleur**.

### Astuce diagnostic
La GUI n'a pas de console (logs invisibles). Si un test échoue et que tu veux
des logs côté hôte (auth, consentement, versions, sessions), lance l'hôte en
**CLI** : `viewerkiller host` — il affiche le journal d'audit. (Revers : la CLI
accepte sans boîte de consentement.) Pour plus de détail : `set RUST_LOG=debug`
avant de lancer.

---

## 1. Réseau & accessibilité

- [ ] **1.1** PC-A lance « Héberger » → un code (6 chiffres), un mot de passe et
      les adresses IP s'affichent. PC-B « Se connecter » avec code + mdp + IP de
      PC-A → la session s'ouvre.
- [ ] **1.2** (sécurité) Depuis un PC **hors du réseau** de PC-A, `Test-NetConnection <IP-A> -Port 47600`
      doit **échouer** (aucune exposition non voulue).

## 2. Authentification & anti-bruteforce

- [ ] **2.1** Mauvais **mot de passe** → la connexion échoue (pas de session).
- [ ] **2.2** Mauvais **code** → échec (« l'hôte ne reconnaît pas ce code »).
- [ ] **2.3** Enchaîne **5 mauvais mots de passe** rapidement depuis PC-B → à la
      6ᵉ tentative l'IP est **verrouillée** ~60 s, même le bon mdp est refusé.
      (Visible dans les logs si hôte lancé en CLI : « verrouillage anti-bruteforce ».)

## 3. Consentement + indicateur (J9, GUI hôte)

- [ ] **3.1** À la connexion de PC-B, PC-A affiche une **boîte « Demande de
      connexion »** (l'IP du contrôleur) avec **Accepter / Refuser**.
- [ ] **3.2** ⭐ **Refuser** → PC-B ne prend pas la main, la session ne s'ouvre
      pas, et **la pop-up ne réapparaît pas** (corrigé en v0.1.15 : ne rebondit
      plus en boucle).
- [ ] **3.3** **Accepter** → la session s'ouvre ; PC-A affiche la bannière
      **« 🔴 Session en cours depuis <IP> »**.
- [ ] **3.4** **Ne rien cliquer ~30 s** → la demande expire = refus automatique.
- [ ] **3.5** Pendant une session, PC-A clique **« Retour »** → l'hébergement
      s'arrête, PC-B est déconnecté, la bannière disparaît.

## 4. Écran (capture + affichage)

- [ ] **4.1** L'écran de PC-A s'affiche sur PC-B, **couleurs correctes** (pas de
      bleu/rouge inversé — piège BGRA/stride).
- [ ] **4.2** Bouge des fenêtres sur PC-A → l'image se met à jour sur PC-B.
- [ ] **4.3** Le ratio/dimensions sont cohérents (bandeau « Écran distant WxH »).

## 5. Souris

- [ ] **5.1** Le curseur va **au bon endroit** (coordonnées absolues précises,
      coins et bords compris).
- [ ] **5.2** **Clic gauche** (ouvrir un menu, un fichier), **clic droit** (menu
      contextuel), **double-clic**.
- [ ] **5.3** **Molette verticale** : défilement d'une page web / document dans
      les deux sens.
- [ ] **5.4** **Molette horizontale** (v0.1.12) : sur un tableur ou une page
      large, le défilement **latéral** fonctionne (molette inclinable, ou
      Shift+molette selon l'application).
- [ ] **5.5** ⭐ **Curseur distant (v0.1.17)** : survole un **champ de texte**
      distant → ton curseur devient un **I** (texte) ; un **lien** → une **main** ;
      un **bord de fenêtre** → une **flèche de redimensionnement**. La forme du
      curseur local reflète celle de l'hôte.

## 6. Clavier (J8)

Ouvre le Bloc-notes sur PC-A et tape **depuis PC-B** :

- [ ] **6.1** Minuscules + **MAJUSCULES** (Shift) : `Bonjour VIEWERKILLER`.
- [ ] **6.2** **Accents / caractères FR** : `é è à ç ù €` (clavier FR : AltGr+e
      pour €).
- [ ] **6.3** Chiffres et symboles : `1234 !@#/?:;.,`.
- [ ] **6.4** ⭐ **Ctrl+A** (tout sélectionner), **Ctrl+C** (copier), **Ctrl+X**
      (couper), **Ctrl+V** (coller) — *copier/couper/coller corrigé en v0.1.13
      (egui les livrait en `Copy`/`Cut`/`Paste`, ignorés) : à tester en priorité.*
      Vérifie un aller-retour : sélectionne + copie du texte distant, colle-le
      ailleurs sur l'hôte.
- [ ] **6.5** ⭐ **Shift+flèches** (sélection de texte), flèches seules (corrigé
      en v0.1.15 : les flèches sont désormais injectées en touches étendues).
- [ ] **6.6** **Entrée**, **Tab**, **Retour arrière**, **Suppr**, **Échap**.
- [ ] **6.7** ⭐ Touches de fonction **F1-F12**, ex. **F5** dans un navigateur
      (recharge) (corrigé en v0.1.15 : injection avec scan code).
- [ ] **6.9** **Tab** puis **Entrée** dans un formulaire distant → la session
      **ne se ferme pas** (corrigé en v0.1.15 : le clavier est capté en
      exclusivité, Entrée n'active plus « Déconnecter »).
- [ ] **6.10** ⭐ **Alt+Tab**, **touche Windows** (et **Win+D**, **Win+E**),
      **Alt+F4**, **Ctrl+Échap** agissent sur le PC **distant** (corrigé en
      v0.1.16 : hook clavier bas niveau, actif **seulement quand la fenêtre
      contrôleur a le focus**). Pour « sortir » du contrôleur, clique ailleurs à la
      **souris** (l'Alt+Tab part au distant). **Ctrl+Alt+Suppr** reste local (non
      captable par un hook — normal).
- [ ] **6.11** Vérifie que **hors session** (accueil, écran hôte), ton **Alt+Tab
      normal fonctionne** (le hook ne doit capter que fenêtre contrôleur focus +
      session active).
- [ ] **6.8** Vérifie qu'il n'y a **pas de double saisie** (ex. l'espace ou une
      lettre qui sort deux fois) ni de **touche restée bloquée** après coup
      (relâche bien Ctrl/Shift/Alt).

## 7. Presse-papiers partagé (J11)

Synchro toutes les ~0,5 s dans les deux sens.

- [ ] **7.1** **Hôte → contrôleur** : copie un texte sur **PC-A** (`Ctrl+C`),
      colle-le sur **PC-B** (`Ctrl+V` dans un éditeur local). Le texte arrive.
- [ ] **7.2** **Contrôleur → hôte** : copie sur **PC-B**, colle sur **PC-A**.
- [ ] **7.3** Avec **accents / emoji** (`café 🚀`) → intégrité conservée.
- [ ] **7.4** Pas de **boucle** ni de scintillement du presse-papiers (le texte
      ne « rebondit » pas indéfiniment).

## 8. Fluidité (J10, ressenti)

- [ ] **8.1** Lance une **vidéo plein écran** sur PC-A → l'affichage sur PC-B
      reste utilisable, **sans accumulation de retard** qui empire avec le temps
      (la cadence baisse proprement sous charge, elle ne s'effondre pas).
- [ ] **8.2** Pendant cette charge, la **souris/clavier restent réactifs**.

## 9. Robustesse & régression clé

- [ ] **9.1** ⭐ **Le bug historique « decrypt error »** : garde une session
      ouverte **plusieurs minutes avec beaucoup de mouvement à l'écran**
      (scroll, vidéo, fenêtres). Il **ne doit jamais** y avoir de coupure
      « réception interrompue : decrypt error ». C'est LE test à ne pas rater.
- [ ] **9.2** Ferme brutalement PC-B (Alt+F4) → PC-A ne plante pas : la bannière
      « Session en cours » disparaît et l'hôte se remet en attente d'une nouvelle
      connexion.
- [ ] **9.3** **Re-héberger** : sur PC-A, Retour puis « Héberger » à nouveau →
      nouveau code, et une reconnexion de PC-B fonctionne (pas d'ancien
      « listener fantôme » qui garderait l'ancien code).
- [ ] **9.4** Reconnexion simple : PC-B « Déconnecter » puis se reconnecte.
- [ ] **9.5** ⭐ **Reconnexion automatique (J13)** : session ouverte, **coupe le
      réseau** de PC-B quelques secondes (Wi-Fi off / câble débranché, ou VPN
      coupé) puis rétablis. PC-B doit afficher **« ⟳ Connexion perdue —
      reconnexion… »**, garder la dernière image figée, puis **reprendre tout
      seul** sans ressaisir code/mot de passe.
- [ ] **9.6** **Détection de pair mort (keepalive/timeout)** : coupe le réseau de
      PC-B et **ne le rétablis pas**. Côté PC-A, la session doit se fermer
      d'elle-même (**~15 s**) et l'hôte se remettre en attente (pas de session
      figée à l'infini). Inversement, si PC-A disparaît, PC-B finit par tenter la
      reconnexion puis abandonne proprement.

## 10. GUI / ergonomie

- [ ] **10.1** Le lancement de `viewerkiller-gui.exe` **n'ouvre pas de fenêtre
      console** noire à côté.
- [ ] **10.2** L'écran « Héberger » liste bien **Wi-Fi et Ethernet** s'ils sont
      présents.
- [ ] **10.3** Les messages d'erreur (mauvais code/mdp, hôte injoignable)
      s'affichent proprement et « Retour à l'accueil » fonctionne.

## 11. Notification de mise à jour (J16a)

- [ ] **11.1** Au lancement, si une **release plus récente** existe sur GitHub,
      l'accueil affiche **« ⬆ Nouvelle version disponible : vX.Y.Z »** + un lien
      « Voir la release ». (Test : lance une version antérieure au dernier tag.)
- [ ] **11.2** **Hors ligne / VPN isolé** (sans accès Internet) : le démarrage
      **n'est pas ralenti** et **aucune erreur** n'apparaît (vérification
      silencieuse, non bloquante).
- [ ] **11.3** En **CLI**, `viewerkiller host` affiche une ligne
      `ℹ Nouvelle version disponible…` le cas échéant.
- [ ] **11.4** ⭐ **Auto-update (J16b, v0.1.18)** : avec une version antérieure,
      clique **« ⬇ Mettre à jour maintenant »** sur l'accueil → téléchargement +
      vérification SHA256 → l'appli **se relance dans la nouvelle version** (vérifie
      le numéro de version). En CLI : `viewerkiller update`.
- [ ] **11.5** Après une mise à jour, **aucun fichier `.exe.old`** ne subsiste (il
      est purgé au démarrage suivant).

## 12. Réglages d'hébergement (v0.1.12)

- [ ] **12.1** Sur l'accueil, déplie **« ⚙ Réglages d'hébergement »** : deux
      curseurs **Images/s** (5–30) et **Qualité JPEG** (40–95).
- [ ] **12.2** Règle **bas** (ex. 8 img/s, qualité 45) puis Héberger → image plus
      granuleuse / moins fluide mais **moins de bande passante** ; règle **haut**
      (ex. 30 img/s, qualité 90) → plus net/fluide, plus de débit. Les réglages
      s'appliquent au **démarrage** de l'hébergement (pas en cours de session).

## 13. Multi-écrans (J12) — ⭐ *nouveau, à valider*

Nécessite un **hôte à au moins 2 moniteurs** (la capture Windows multi-moniteur
n'a pas été testée au runtime).

- [ ] **13.1** Avec un hôte multi-écrans, la barre de session affiche un
      sélecteur **« Écran : 1 (principal) 2 … »**. Avec un seul écran, aucun
      sélecteur (normal).
- [ ] **13.2** Clique **Écran 2** → l'affichage bascule sur le second moniteur,
      aux **bonnes dimensions** et **bonnes couleurs** (piège coordonnées du
      bureau virtuel / offsets négatifs pour un écran à gauche du principal).
- [ ] **13.3** Reviens sur **Écran 1** → retour correct.
- [ ] **13.4** Souris/clavier restent cohérents avec le moniteur affiché après
      bascule (les coordonnées visent le bon écran).

---

## Si un test échoue

1. Note le **numéro** et ce que tu as vu (message exact, capture éventuelle).
2. Si c'est côté hôte, relance l'hôte en **CLI** (`viewerkiller host`, au besoin
   `set RUST_LOG=debug`) pour capturer le journal d'audit et colle-le-moi.
3. Précise si les **deux** PC sont bien en v0.1.17.
