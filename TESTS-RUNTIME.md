# ViewerKiller — recette runtime (à jouer sur 2 PC)

> Cible : **v0.1.8** (protocole **v3**). Toute la chaîne est vérifiée en CI
> (compilation + tests Linux/Windows), mais la capture d'écran, l'injection
> clavier/souris et le presse-papiers ne se valident qu'**en vrai, sur Windows**.
> Rejouer cette liste à chaque release. Coche au fur et à mesure ; note le
> numéro du test qui échoue et ce que tu as observé.

## Préparatifs

- [ ] **Même version sur les deux PC** : vérifie le **tag de la release**
      téléchargée (les binaires n'ont pas encore de flag `--version` — je peux
      l'ajouter si utile). Protocole v3 → une v0.1.5 en face **refusera** la
      connexion, c'est normal.
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
- [ ] **3.2** **Refuser** → PC-B ne prend pas la main, la session ne s'ouvre pas.
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
- [ ] **5.3** **Molette** : défilement d'une page web / document dans les deux
      sens. *(La molette horizontale n'est pas encore gérée — attendu.)*

## 6. Clavier (J8)

Ouvre le Bloc-notes sur PC-A et tape **depuis PC-B** :

- [ ] **6.1** Minuscules + **MAJUSCULES** (Shift) : `Bonjour VIEWERKILLER`.
- [ ] **6.2** **Accents / caractères FR** : `é è à ç ù €` (clavier FR : AltGr+e
      pour €).
- [ ] **6.3** Chiffres et symboles : `1234 !@#/?:;.,`.
- [ ] **6.4** **Ctrl+A** (tout sélectionner), **Ctrl+C**, **Ctrl+V**.
- [ ] **6.5** **Shift+flèches** (sélection de texte), flèches seules.
- [ ] **6.6** **Entrée**, **Tab**, **Retour arrière**, **Suppr**, **Échap**.
- [ ] **6.7** Une touche de fonction, ex. **F5** dans un navigateur (recharge).
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

## 10. GUI / ergonomie

- [ ] **10.1** Le lancement de `viewerkiller-gui.exe` **n'ouvre pas de fenêtre
      console** noire à côté.
- [ ] **10.2** L'écran « Héberger » liste bien **Wi-Fi et Ethernet** s'ils sont
      présents.
- [ ] **10.3** Les messages d'erreur (mauvais code/mdp, hôte injoignable)
      s'affichent proprement et « Retour à l'accueil » fonctionne.

---

## Si un test échoue

1. Note le **numéro** et ce que tu as vu (message exact, capture éventuelle).
2. Si c'est côté hôte, relance l'hôte en **CLI** (`viewerkiller host`, au besoin
   `set RUST_LOG=debug`) pour capturer le journal d'audit et colle-le-moi.
3. Précise si les **deux** PC sont bien en v0.1.8.
