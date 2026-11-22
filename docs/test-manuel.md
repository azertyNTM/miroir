# Tests manuels

Ce que les tests automatiques ne peuvent pas vérifier : tout ce qui demande un écran, un clic dans une fenêtre du bureau, ou une seconde machine.

## Phase 1 — Capture Linux avec aperçu

Sur le PC Linux (KDE Plasma, Wayland). Lancer en mode optimisé, le mode debug est trop lent pour juger la fluidité :

```bash
cargo run --release -p miroir-app
```

1. **Choix de la source.** Cliquer « Choisir un écran ou une fenêtre ». La fenêtre de partage de KDE s'ouvre : choisir un écran entier. L'aperçu apparaît dans Miroir, curseur de la souris visible.
2. **60 fps.** Lancer un jeu ou une vidéo 60 fps sur l'écran capturé. Le compteur en haut doit afficher environ 60 fps et la taille de l'écran. Sur un écran où rien ne bouge, « 0 fps » suivi de « image fixe » est normal : KDE n'envoie une image que quand quelque chose change.
3. **Fenêtre.** « Arrêter ». Deux boutons apparaissent maintenant qu'un choix est mémorisé : cliquer « Choisir une autre source… » et prendre une fenêtre (onglet « Fenêtre » du sélecteur KDE). Redimensionner la fenêtre capturée : l'aperçu suit.
4. **Mémoire du choix.** « Arrêter », puis « Reprendre la dernière source » : la capture de la même fenêtre repart sans que KDE redemande. Fermer Miroir, le relancer : même résultat.
5. **Annulation.** « Choisir une autre source… » puis « Annuler » dans la fenêtre KDE : Miroir revient aux deux boutons, sans message d'erreur. Tant que la fenêtre KDE est ouverte, le bouton « Arrêter » de Miroir est grisé, c'est voulu.
6. **Arrêt depuis le bureau.** Pendant une capture, cliquer l'icône de partage d'écran dans la zone de notification de KDE et arrêter le partage : Miroir affiche « Capture arrêtée : … » et les boutons redeviennent disponibles.
7. **Enchaînement.** Démarrer et arrêter une capture trois fois de suite sans fermer Miroir : chaque démarrage doit fonctionner.

En cas de problème, relancer avec les journaux détaillés et renvoyer la sortie du terminal :

```bash
MIROIR_LOG=debug cargo run --release -p miroir-app
```

## Phase 2 — Boucle locale encodage / décodage

Même PC, même commande. L'image affichée n'est plus la capture brute : chaque image est compressée en H.264 puis décompressée avant d'être montrée, comme elle le sera chez l'autre personne.

L'image est encodée à sa taille d'origine jusqu'à 2560×1440 ; au-delà (4K), elle est réduite pour tenir dans 2560×1440.

```bash
cargo run --release -p miroir-app
```

Sous les boutons, une ligne d'état résume la dernière seconde :

```
60 fps — 1920×1080 — h264_vaapi (matériel) — encodage 7,0 + décodage 1,5 = 8,5 ms (pire image : 10,2 ms) — affichage 2,0 ms
```

- **encodage** : de l'image capturée aux données compressées (conversion des couleurs, envoi à la carte graphique, compression).
- **décodage** : des données compressées à l'image.
- **pire image** : encodage + décodage de l'image la plus lente de la seconde.
- **affichage** : conversion de l'image décodée pour l'écran. Elle ne fait pas partie du décodage et disparaîtra en phase 9.

1. **Encodeur matériel.** Choisir un écran. La ligne d'état doit indiquer `h264_vaapi (matériel)`.
2. **Fluidité et temps.** Lancer un jeu ou une vidéo 60 fps sur l'écran capturé : environ 60 fps. Noter le total encodage + décodage et la « pire image » observés sur un vrai jeu. Sur un écran immobile, la ligne se termine par « image fixe ». Ce que je mesure sur des images de synthèse :
   - écran 1080p : total de 8 à 11 ms, dont 4,5 ms passés dans la carte elle-même. Le critère de la spec (sous 10 ms) est à la limite de ce que peut la carte.
   - écran 1440p : encodage 10 à 11,5 ms, décodage 1,5 à 6 ms, soit un total de 12 à 17 ms. Le critère des 10 ms ne peut pas être tenu à cette taille ; les 60 fps tiennent parce que l'encodage et le décodage tournent chacun de leur côté, comme sur deux PC.
3. **Qualité.** L'image doit être nette, le texte lisible, les couleurs justes (pas de rouge et de bleu inversés, pas de voile gris ou vert), sans bandes ni décalage entre le haut et le bas de l'image.
4. **Taille de l'image.** La ligne d'état affiche la taille de l'écran capturé (2560×1440 pour un écran 1440p). Pour un écran 4K, elle affiche 2560×1440 et l'image reste entière, non rognée.
5. **Fenêtre redimensionnée.** « Choisir une autre source… », prendre une fenêtre, la redimensionner plusieurs fois : l'image suit, sans plantage ni message d'erreur.
6. **Repli logiciel.** Fermer Miroir et le relancer en forçant l'encodeur logiciel. La ligne d'état doit indiquer `libx264 (logiciel)` et l'image doit rester correcte :

   ```bash
   MIROIR_ENCODER=libx264 cargo run --release -p miroir-app
   ```

7. **Durée.** Laisser tourner deux minutes sur une vidéo : les fps et les temps restent stables.
8. **Fermeture.** Fermer la fenêtre de Miroir pendant une capture : le programme se termine sans plantage ni message d'erreur dans le terminal.

Mesure des temps sans l'interface, sur une image de synthèse en mouvement envoyée à 60 images par seconde. En 1080p puis en 1440p, elle affiche les temps et le nombre d'images au-dessus de 10 ms, et n'échoue que si 60 fps ne peuvent pas être tenus :

```bash
cargo test --release -p miroir-codec -- --ignored --nocapture
```

### Diagnostic du flux PipeWire seul

`pw_node` lit un nœud vidéo PipeWire pendant 3 secondes et affiche le format et le débit d'images, sans portail ni interface :

```bash
gst-launch-1.0 videotestsrc is-live=true ! video/x-raw,format=BGRx,width=1920,height=1080,framerate=60/1 ! pipewiresink mode=provide stream-properties="props,media.class=Video/Source,node.name=miroir-test"
```

Dans un autre terminal, repérer l'identifiant du nœud `miroir-test` avec `pw-dump`, puis :

```bash
cargo run --release -p miroir-capture --example pw_node -- <identifiant>
```

Toute la boucle de l'appli (capture, encodage, décodage, affichage) peut aussi lire ce nœud, sans passer par la fenêtre de partage de KDE. Avec les journaux détaillés, les temps sont écrits dans le terminal une fois par seconde :

```bash
MIROIR_PW_NODE=<identifiant> MIROIR_LOG=debug cargo run --release -p miroir-app
```
