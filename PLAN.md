# Plan complet — Miroir (nom provisoire)

Application de partage d'écran entre deux PC, en Rust, pour streamer un jeu ou une vidéo à un ami en 1080p 60 fps, dans les deux sens. Pas de chat, pas d'appel vocal, pas d'envoi de fichiers : uniquement le partage d'écran (image + son).

Ce document est destiné à Claude Code. Il décrit l'objectif, l'architecture, les choix techniques, l'ordre de construction et les critères de validation de chaque étape.

---

## 0. Contexte des machines

- Mon PC : Linux (EndeavourOS / Arch, KDE Plasma 6 sous Wayland, PipeWire), Ryzen 5 2600X, Radeon RX 5500 XT (encodage matériel VAAPI H.264 / HEVC).
- PC de l'ami : Windows 11, carte graphique inconnue. L'application doit donc détecter seule l'encodeur et le décodeur disponibles (NVIDIA, AMD ou Intel, sinon logiciel) et afficher dans les stats (F3) lequel est utilisé.
- Serveur : VPS Hostinger (accès root, IP publique). Il héberge uniquement la signalisation et le relais TURN.
- Débit internet : pas encore mesuré, on ajustera pendant les tests réels. Le débit vidéo doit donc être réglable et s'adapter tout seul (phase 7).
- Tests côté Windows : je n'ai pas de machine Windows. Tout ce qui concerne Windows se teste sur le PC de l'ami. Conséquence pour Claude Code : chaque phase qui touche Windows doit produire un `.exe` de test téléchargeable (artefact de la CI GitHub Actions) et un fichier de logs facile à me renvoyer (bouton « Ouvrir le dossier des logs » dans l'appli dès la phase 3).

---

## 1. Instructions pour Claude Code

- Travailler phase par phase, dans l'ordre du chapitre 9. Ne pas passer à la phase suivante tant que les critères de validation de la phase en cours ne sont pas remplis.
- À la fin de chaque phase, s'arrêter, résumer ce qui a été fait, et dire précisément ce que je dois tester à la main (sur quelle machine, quelle commande, ce que je dois voir).
- Privilégier la simplicité : d'abord une version qui marche (même avec des copies mémoire), ensuite l'optimisation. Le chemin « zéro copie » GPU est une phase à part.
- Vérifier la dernière version de chaque crate sur crates.io avant de l'ajouter, et lire sa documentation plutôt que de deviner son API.
- Interface en français. Code et commentaires en anglais.
- Journalisation avec `tracing` partout (niveau réglable par variable d'environnement `MIROIR_LOG`), car le débogage à distance chez l'ami se fera avec les logs.
- Le projet doit compiler et tourner sur Linux (Arch / EndeavourOS, KDE Plasma 6, Wayland, PipeWire) et Windows 10/11 x64.

---

## 2. Objectif et critères de réussite globaux

L'application est réussie quand :

- Moi (Linux) ou mon ami (Windows) peut cliquer sur « Partager mon écran », choisir un écran ou une fenêtre, et l'autre reçoit une notification « Alexis partage son écran — Regarder ».
- L'image arrive en 1080p à 60 fps, fluide, avec une latence de bout en bout inférieure à 100 ms sur une connexion fibre.
- Le son du jeu ou de la vidéo est transmis avec l'image, sans écho de notre appel vocal (qui se fait à côté, avec un autre outil).
- Les deux peuvent partager en même temps (chacun regarde l'écran de l'autre).
- L'ami installe un seul fichier `.exe`, entre un pseudo et un code d'invitation une seule fois, et n'a plus jamais rien à configurer.
- Une session de 2 heures tient sans coupure ni dérive de latence.
- Impact sur les performances du jeu de celui qui partage : quelques fps au maximum (l'encodage se fait sur la puce dédiée de la carte graphique).

Hors périmètre : chat, voix, fichiers, plus de deux personnes, mobile, HDR, enregistrement.

---

## 3. Architecture

```
  PC qui partage                       Serveur (le mien)                 PC qui regarde
  ---------------                      -----------------                 --------------
  Capture écran/fenêtre                miroir-server (signalisation,     Réception WebRTC
  Capture son de l'appli               WebSocket sécurisé)               Décodage matériel
  Encodage matériel H.264   <--- mise en relation (SDP, ICE) --->        Affichage wgpu
  Encodage son Opus                    coturn (relais TURN,              Lecture du son
  Envoi WebRTC  ========== flux direct pair-à-pair (UDP) ==========>     
                           (passe par coturn seulement si le direct échoue)
```

Principes :

- Pair-à-pair : le flux vidéo va directement d'un PC à l'autre. Le serveur sert uniquement à mettre les deux PC en relation et de relais de secours.
- Une session WebRTC par direction de partage. Si les deux partagent en même temps, il y a deux sessions indépendantes. C'est plus simple qu'une session bidirectionnelle.
- WebRTC apporte gratuitement le chiffrement (DTLS-SRTP), la traversée des box (ICE / STUN / TURN), la gestion des pertes (NACK, demande d'image clé) et l'estimation de bande passante.

---

## 4. Choix techniques

| Besoin | Choix | Pourquoi |
|---|---|---|
| Langage | Rust (édition 2021 ou plus récente), workspace Cargo | demandé |
| Asynchrone | `tokio` | standard |
| Interface | `eframe` / `egui` avec backend `wgpu` | léger, multiplateforme, tout en Rust |
| Affichage vidéo | texture `wgpu` + shader NV12 vers RGB | affichage rapide, sans conversion CPU |
| Capture Linux | portail `xdg-desktop-portal` ScreenCast via `ashpd` + flux via `pipewire` | seule méthode propre sous Wayland / KDE |
| Capture Windows | Windows Graphics Capture via `windows-capture` | API moderne, capture fenêtre ou écran, jeux en plein écran fenêtré |
| Encodage / décodage vidéo | FFmpeg via `ffmpeg-next` (ou `rsmpeg`) | accès à tous les encodeurs matériels avec une seule API |
| Encodeurs matériels | Linux AMD : `h264_vaapi`. Windows : `h264_nvenc` (NVIDIA), `h264_amf` (AMD), `h264_qsv` (Intel). Secours : `libx264` ou `libopenh264` | la puce d'encodage dédiée ne ralentit presque pas le jeu |
| Décodeurs matériels | Linux : VAAPI. Windows : D3D11VA. Secours : logiciel | |
| Codec vidéo | H.264 par défaut, HEVC en option si les deux machines le gèrent | H.264 marche partout. Ma RX 5500 XT encode H.264 et HEVC, pas AV1 |
| Transport | WebRTC avec `str0m` (bibliothèque WebRTC Rust « sans IO ») | Rust pur, contrôle total, estimation de bande passante intégrée |
| Son : encodage | Opus via `opus` ou `audiopus` | standard WebRTC, faible latence |
| Son : capture Linux | `pipewire` (flux de sortie d'une application, ou moniteur de la sortie par défaut) | |
| Son : capture Windows | WASAPI via crate `windows` (capture « process loopback » d'un processus précis, ou loopback global) | permet de ne capturer que le jeu |
| Son : lecture | `cpal` | multiplateforme |
| Signalisation client | `tokio-tungstenite` (WebSocket sécurisé) | |
| Serveur | `axum` + WebSocket, derrière `caddy` (HTTPS automatique) | simple, léger |
| Relais | `coturn` avec identifiants temporaires (secret partagé) | standard |
| Config | `serde` + `toml`, emplacement via `directories` | |
| Erreurs | `anyhow` (application), `thiserror` (bibliothèques) | |
| Mise à jour auto (Windows) | `self_update` avec les releases GitHub | l'ami n'a jamais à réinstaller à la main |

---

## 5. Structure du dépôt

```
miroir/
  Cargo.toml                  workspace
  crates/
    miroir-core/              types communs, protocole de signalisation, config
    miroir-capture/           trait VideoSource + implémentations linux/ et windows/
    miroir-codec/             encodage et décodage FFmpeg, choix automatique de l'encodeur
    miroir-audio/             capture, encodage Opus, lecture
    miroir-net/               session WebRTC (str0m), client de signalisation
    miroir-app/               binaire client (interface egui, lecteur vidéo)
    miroir-server/            binaire serveur de signalisation
  deploy/
    docker-compose.yml        miroir-server + coturn + caddy
    Caddyfile
    turnserver.conf
  packaging/
    windows/                  script Inno Setup (MiroirSetup.exe) + DLL FFmpeg
    arch/PKGBUILD             paquet local pour mon PC
  .github/workflows/
    ci.yml                    build + tests Linux et Windows à chaque push
    release.yml               build des binaires et de l'installateur sur tag
  docs/
    test-manuel.md            procédures de test à la main
```

Interfaces principales (à affiner) :

```rust
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pts_us: u64,          // horodatage en microsecondes
    pub data: FrameData,
}

pub enum FrameData {
    CpuBgra(Vec<u8>),         // phase 1 : simple
    // ajoutés en phase d'optimisation :
    // DmaBuf(...)            Linux, zéro copie vers VAAPI
    // D3D11Texture(...)      Windows, zéro copie vers NVENC/AMF/QSV
}

pub trait VideoSource: Send {
    fn next_frame(&mut self) -> anyhow::Result<Option<VideoFrame>>;
    fn stop(&mut self);
}

pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &VideoFrame) -> anyhow::Result<Vec<EncodedPacket>>;
    fn request_keyframe(&mut self);
    fn set_bitrate(&mut self, bps: u32);
}
```

---

## 6. Réglages vidéo et audio

Vidéo (faible latence avant tout) :

- 1920x1080, 60 fps. Si la source est plus grande (1440p, 4K), réduire à 1080p. Si elle est plus petite, garder la taille d'origine.
- Pas d'images B, préréglage basse latence de chaque encodeur (NVENC : tune `ll` ou `ull`, AMF : usage `ultralowlatency`, VAAPI : sans B-frames, `async_depth` faible).
- Débit : 10 Mbit/s par défaut, réglable de 4 à 20 Mbit/s. Débit maximum plafonné, pas de pic.
- Image clé : intervalle long (par exemple toutes les 4 secondes) et image clé immédiate quand le récepteur la demande (PLI / FIR WebRTC) après une perte.
- Adaptation : le débit de l'encodeur suit l'estimation de bande passante de `str0m`, entre 3 Mbit/s et le maximum choisi. On garde 60 fps en priorité (c'est pour des jeux). En dessous de 4 Mbit/s, passer automatiquement en 720p60.
- Image fixe : PipeWire n'envoie pas d'image quand rien ne bouge. Renvoyer la dernière image au moins une fois par seconde pour que le récepteur ne croie pas à une coupure.
- Curseur de la souris : inclus, désactivable.

Son :

- Opus, 48 kHz stéréo, trames de 10 ms, 160 kbit/s, correction d'erreurs (FEC) activée.
- Lecture avec un petit tampon (40 à 60 ms) pour absorber les variations du réseau.
- Trois modes de son, au choix au moment de partager :
  1. « Son de l'application » (par défaut) : seulement le jeu ou le lecteur vidéo. Sous Windows, capture « process loopback » du processus de la fenêtre choisie (et de ses processus enfants). Sous Linux, liste des applications qui jouent du son (flux PipeWire de type `Stream/Output/Audio`) et on choisit la bonne.
  2. « Tout le son du PC » : avertissement à afficher, car le son de l'appel vocal sera aussi envoyé et l'autre s'entendra en écho.
  3. « Pas de son ».

---

## 7. Signalisation, comptes et connexion

Identité (pas de vrais comptes, on est deux) :

- Au premier lancement : écran « Ton pseudo » + « Code d'invitation ». Sauvegardés dans `config.toml` (`%APPDATA%\Miroir\` sous Windows, `~/.config/miroir/` sous Linux).
- Je crée les codes d'invitation sur le serveur avec une commande : `miroir-server invite create`. Le code donne un jeton permanent stocké par le client.
- L'adresse du serveur est intégrée à la compilation (valeur par défaut), modifiable dans la config au besoin. L'ami n'a jamais à la taper.

Protocole (JSON sur WebSocket sécurisé `wss://`) :

- Client vers serveur : `Hello { token, name }`, `ShareStarted { session_id, title }`, `ShareStopped { session_id }`, `Join { session_id }`, `Sdp { session_id, to, sdp }`, `Ice { session_id, to, candidate }`.
- Serveur vers client : `Welcome { peers, turn }` (identifiants TURN temporaires générés avec le secret de coturn), `PeerOnline / PeerOffline`, `ShareAvailable { from, session_id, title }`, `ShareEnded`, et relais des messages `Sdp` et `Ice`.
- Reconnexion automatique avec attente croissante si le serveur coupe.

Déroulé d'un partage :

1. A clique « Partager mon écran », choisit la source (sélecteur du portail KDE sous Linux, liste de fenêtres et écrans dans l'appli sous Windows) et le mode de son.
2. A annonce le partage. B reçoit une notification système + un bouton « Regarder » dans l'appli.
3. B clique « Regarder ». A crée l'offre WebRTC (vidéo + son en envoi seul), B répond (réception seule), échange des candidats ICE via le serveur.
4. Le flux part en direct. Si la connexion directe échoue, il passe par coturn.
5. Chacun peut arrêter. Quand A arrête, B voit « Partage terminé ».

---

## 8. Interface

Fenêtre principale (petite, simple) :

- État de connexion au serveur et de l'ami (en ligne / hors ligne / en train de partager).
- Gros bouton « Partager mon écran », et pendant un partage, « Arrêter le partage ».
- Quand l'ami partage : « Regarder ».
- Réglages : qualité (1080p60 / 720p60 / 1080p30), débit maximum, mode de son par défaut, encodeur (automatique ou forcé), inclure le curseur, lancer au démarrage.

Fenêtre de visionnage :

- Redimensionnable, image à l'échelle sans déformation (bandes noires si besoin).
- Double-clic ou F11 pour le plein écran, Échap pour en sortir.
- Volume du son reçu, réglable dans la fenêtre.
- F3 : surcouche de statistiques (fps reçus, débit, latence aller-retour, pertes, encodeur et décodeur utilisés, connexion directe ou relayée).

Icône dans la barre des tâches (zone de notification) avec menu : Partager, Arrêter, Quitter.

---

## 9. Phases de construction

### Phase 0 — Fondations
- Workspace Cargo, crates vides, `tracing`, config TOML.
- CI GitHub Actions : compilation et tests sur `ubuntu-latest` et `windows-latest`. Installer les dépendances FFmpeg et PipeWire dans la CI.
- Validation : `cargo build` et `cargo test` passent en local sous Linux et dans la CI sous Windows.

### Phase 1 — Capture Linux avec aperçu local
- Portail ScreenCast via `ashpd` (écran ou fenêtre, curseur intégré), flux PipeWire, images en BGRA côté CPU.
- Utiliser le « restore token » du portail (persist mode) pour que KDE se souvienne du choix et ne redemande pas à chaque fois.
- Aperçu dans une fenêtre egui/wgpu.
- Validation : je lance l'appli sur mon PC, je choisis un jeu ou une vidéo, je vois l'aperçu à 60 fps (compteur affiché).

### Phase 2 — Boucle locale encodage / décodage
- Encodage H.264 avec choix automatique de l'encodeur (essayer le matériel, sinon logiciel), conversion BGRA vers NV12 (d'abord avec `swscale`), décodage, affichage du résultat.
- Mesurer et afficher le temps d'encodage et de décodage par image.
- Validation : sur mon PC, l'image décodée s'affiche fluide en 1080p60 avec `h264_vaapi`, encodage + décodage sous 10 ms par image.

### Phase 3 — Capture et encodage Windows
- Capture via `windows-capture` (liste des fenêtres et écrans, sans bordure jaune sous Windows 11 si possible), encodage NVENC / AMF / QSV selon la carte, décodage D3D11VA.
- Même boucle locale qu'en phase 2.
- Fournir un `.exe` de test via la CI, à envoyer à l'ami.
- Validation : sur le PC de l'ami, boucle locale fluide en 1080p60, encodeur matériel détecté et affiché, logs récupérés.

### Phase 4 — Serveur de signalisation et déploiement
- `miroir-server` (axum, WebSocket, codes d'invitation, identifiants TURN temporaires).
- `deploy/docker-compose.yml` avec miroir-server, coturn et caddy, pour le VPS Hostinger. Ports à ouvrir : 80/tcp et 443/tcp (Caddy), 3478/tcp+udp, plage UDP de relais de coturn (par exemple 49160-49200). Les ouvrir à la fois dans le pare-feu du VPS (ufw) et dans le pare-feu Hostinger du panneau hPanel s'il est activé.
- Coturn doit annoncer l'IP publique du VPS (`external-ip`).
- Un nom de domaine (ou sous-domaine) pointant vers le VPS est nécessaire pour le HTTPS. Me demander lequel utiliser.
- Écrire `deploy/README.md` avec les commandes exactes à lancer sur le VPS, pas à pas.
- Validation : les deux clients se connectent au serveur et se voient « en ligne ».

### Phase 5 — Transport vidéo pair-à-pair
- Session WebRTC avec `str0m` : piste vidéo H.264, ICE avec STUN et TURN, demande d'image clé (PLI) branchée sur `request_keyframe`.
- Une session par direction. Partage simultané dans les deux sens.
- Validation : mon PC Linux partage vers la machine Windows et inversement, image fluide, statistiques F3 cohérentes. Test forcé en mode relais (TURN seul) pour vérifier le secours.

### Phase 6 — Son
- Capture par application (Windows : process loopback, Linux : flux PipeWire choisi), mode « tout le PC », mode sans son.
- Encodage Opus, piste audio WebRTC, lecture `cpal` avec tampon.
- Validation : le son du jeu arrive synchronisé avec l'image (décalage non perceptible), et le son de l'appel vocal n'est pas transmis en mode « application ».

### Phase 7 — Qualité réseau et robustesse
- Adaptation du débit selon l'estimation de bande passante, passage automatique en 720p60 si la connexion est trop faible.
- Récupération après perte : image clé à la demande, pas de figeage durable.
- Reconnexion automatique au serveur. Si la connexion pair-à-pair tombe, tentative de reconnexion du partage.
- Validation : avec une limitation réseau simulée (`tc netem` sous Linux : 2 % de pertes, 50 ms de délai, débit limité à 5 Mbit/s), l'image reste regardable et se rétablit seule. Session de 2 heures sans coupure.

### Phase 8 — Interface complète
- Tout le chapitre 8 : fenêtre principale, réglages, notifications, plein écran, statistiques F3, icône de la zone de notification, premier lancement (pseudo + code).
- Validation : l'ami, sans aide, arrive à regarder mon partage et à partager le sien.

### Phase 9 — Optimisation zéro copie (facultative mais conseillée)
- Linux : récupérer les images PipeWire en DMA-BUF et les passer directement à VAAPI (mise à l'échelle avec `scale_vaapi`), sans passer par le CPU.
- Windows : passer la texture D3D11 de la capture directement à l'encodeur matériel (mise à l'échelle sur GPU).
- Décodage vers texture GPU affichée directement par wgpu si possible.
- Validation : charge CPU du partage nettement réduite (mesurer avant / après), latence identique ou meilleure.

### Phase 10 — Installation et mises à jour
- Windows : `MiroirSetup.exe` avec Inno Setup, DLL FFmpeg incluses (build LGPL partagé), raccourci sur le bureau, lancement au démarrage en option. Mise à jour automatique via les releases GitHub.
- Note pour l'ami : l'exécutable n'étant pas signé, Windows SmartScreen affichera un avertissement la première fois (« Informations complémentaires » puis « Exécuter quand même »).
- Linux : `PKGBUILD` local (dépendances : `ffmpeg`, `pipewire`, `xdg-desktop-portal-kde`, pilotes VAAPI de Mesa).
- `release.yml` : sur un tag `vX.Y.Z`, build Windows et Linux, publication de la release.
- Validation : installation de zéro sur le PC de l'ami en moins de 2 minutes, puis mise à jour automatique d'une version à la suivante.

---

## 10. Points d'attention connus

- Jeux en plein écran exclusif sous Windows : la capture peut donner une image noire. Conseiller le mode « plein écran fenêtré » (borderless) dans les options du jeu.
- Jeux en plein écran sous Linux (Proton / XWayland) : capturer « l'écran entier » est souvent plus fiable que capturer la fenêtre.
- Linux : vérifier que le pilote VAAPI de Mesa est installé (`vainfo` doit lister `VAEntrypointEncSlice` pour H.264).
- Le portail ScreenCast ne fournit pas le son : la capture audio est toujours séparée (PipeWire directement).
- Débit montant : en dessous d'environ 10 Mbit/s d'upload réel, le 1080p60 sera dégradé quoi qu'on fasse.
- Anti-triche : certains jeux bloquent la capture. Rien à faire côté appli, à noter dans le README.

---

## 11. Risques et plans B

- Si `str0m` est trop difficile à intégrer : remplacer le transport par QUIC (`quinn`) avec tout le flux relayé par mon serveur. Plus simple (pas de traversée de box), un peu plus de latence, et le serveur doit avoir environ 20 Mbit/s de bande passante dans chaque sens.
- Si FFmpeg pose trop de problèmes sous Windows (DLL, build) : basculer sur GStreamer via `gstreamer-rs` (capture, encodage et décodage matériels disponibles sur les deux OS).
- Si l'encodeur matériel n'est pas disponible sur une machine : `libx264` en préréglage `ultrafast` / `zerolatency`, en sachant que ça coûtera des fps au jeu.

---

## 12. Procédure de test finale

1. Installation propre sur le PC de l'ami avec `MiroirSetup.exe`, premier lancement, pseudo + code.
2. Je partage un jeu en 1080p60 : l'ami regarde 15 minutes. Vérifier fps, débit, son, absence d'écho.
3. L'ami partage une vidéo YouTube en plein écran : je regarde 15 minutes.
4. Partage simultané dans les deux sens pendant 10 minutes.
5. Mesure de latence : afficher un chronomètre en millisecondes sur l'écran partagé et filmer côte à côte l'écran source et l'écran récepteur avec un téléphone. Objectif inférieur à 100 ms.
6. Couper le Wi-Fi ou débrancher le câble 10 secondes : le partage doit reprendre seul.
7. Session longue de 2 heures.
