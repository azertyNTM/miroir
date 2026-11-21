# Tests manuels

Ce que les tests automatiques ne peuvent pas vérifier : tout ce qui demande un écran, un clic dans une fenêtre du bureau, ou une seconde machine.

## Phase 1 — Capture Linux avec aperçu

Sur le PC Linux (KDE Plasma, Wayland). Lancer en mode optimisé, le mode debug est trop lent pour juger la fluidité :

```bash
cargo run --release -p miroir-app
```

1. **Choix de la source.** Cliquer « Choisir un écran ou une fenêtre ». La fenêtre de partage de KDE s'ouvre : choisir un écran entier. L'aperçu apparaît dans Miroir, curseur de la souris visible.
2. **60 fps.** Lancer un jeu ou une vidéo 60 fps sur l'écran capturé. Le compteur en haut doit afficher environ 60 fps et la taille de l'écran. Sur un écran où rien ne bouge, « 0 fps (image fixe) » est normal : KDE n'envoie une image que quand quelque chose change.
3. **Fenêtre.** « Arrêter ». Deux boutons apparaissent maintenant qu'un choix est mémorisé : cliquer « Choisir une autre source… » et prendre une fenêtre (onglet « Fenêtre » du sélecteur KDE). Redimensionner la fenêtre capturée : l'aperçu suit.
4. **Mémoire du choix.** « Arrêter », puis « Reprendre la dernière source » : la capture de la même fenêtre repart sans que KDE redemande. Fermer Miroir, le relancer : même résultat.
5. **Annulation.** « Choisir une autre source… » puis « Annuler » dans la fenêtre KDE : Miroir revient aux deux boutons, sans message d'erreur. Tant que la fenêtre KDE est ouverte, le bouton « Arrêter » de Miroir est grisé, c'est voulu.
6. **Arrêt depuis le bureau.** Pendant une capture, cliquer l'icône de partage d'écran dans la zone de notification de KDE et arrêter le partage : Miroir affiche « Capture arrêtée : … » et les boutons redeviennent disponibles.
7. **Enchaînement.** Démarrer et arrêter une capture trois fois de suite sans fermer Miroir : chaque démarrage doit fonctionner.

En cas de problème, relancer avec les journaux détaillés et renvoyer la sortie du terminal :

```bash
MIROIR_LOG=debug cargo run --release -p miroir-app
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
