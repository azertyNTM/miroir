# Miroir

Partage d'écran pair-à-pair entre deux PC (Linux et Windows), en Rust : image 1080p 60 fps et son du jeu, dans les deux sens. Pas de chat, pas de voix, pas de fichiers.

Projet en cours de construction, phase par phase. Le plan complet est dans [PLAN.md](PLAN.md).

```bash
cargo build --workspace
cargo test --workspace
```

Dépendances système sous Arch : `ffmpeg`, `pipewire`, `clang`.
