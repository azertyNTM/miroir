# Miroir

Read `PLAN.md` first: it is the spec (architecture, technical choices, phases, validation criteria).

- Work one phase at a time, in the order of PLAN.md chapter 9. At the end of a phase: stop, summarise, and tell Alexis exactly what to test by hand.
- UI text in French. Code and comments in English.
- `tracing` everywhere; log level via `MIROIR_LOG`.
- Check the latest version of a crate on crates.io and read its docs before adding it.
- Alexis has no Windows machine: Windows is tested through CI and on the friend's PC.

## Progress

- Phase 0 (foundations): done — workspace, config, logging, CI green on Linux and Windows (FFmpeg links on both).
- Phase 1 (Linux capture with local preview): done, validated by Alexis on his PC on 2026-11-22. Branch `phase-1-capture-linux`, not merged into `main` yet (his call).
- Phase 2 (local encode/decode loop): built on branch `phase-2-boucle-locale` (stacked on phase 1), waiting for Alexis to run `docs/test-manuel.md`.
- Next: phase 3 (Windows capture and encoding), once phase 2 is validated.

## Notes for later phases

- The PipeWire consumer can be exercised without the portal: `docs/test-manuel.md` explains how to publish a GStreamer test pattern and read it with the `pw_node` example.
- PipeWire does not report an error when the captured node disappears (the stream just pauses): the end of a capture comes from the portal session's `Closed` signal.
- Each `PortalSource` owns its tokio runtime and its own D-Bus connection. Never use `Screencast::new()` (ashpd's process-wide connection): it stops answering once the first runtime is dropped. `cargo test -p miroir-capture -- --ignored` checks this against the real portal.
- `PortalSource::stop` (and so its `Drop`) calls `block_on`: it panics if called from inside a tokio task. Keep sources on plain threads, or rework this when the app gets a shared runtime (phase 5).
- `VideoSource::next_frame` waits up to 100 ms and returns `Ok(None)` when nothing changed on screen: the sender must resend the last frame itself at least once per second (PLAN.md chapter 6, "image fixe"). Not done in phase 2 (no receiver yet): do it in phase 5.
- Keyframe on request (PLI) is not implemented either: phase 5.

- GitHub repo: `azertyNTM/miroir` (public). Commits use the GitHub noreply address set in the repo-local git config.
- Windows CI uses the BtbN LGPL shared FFmpeg build: no `libx264` there, the software fallback on Windows is `libopenh264`.
- Linux CI (Ubuntu) builds against an older FFmpeg than Arch and the Windows CI (9.0): if CI is red but local is green in phase 2, run the Linux job in an `archlinux` container.
- Logs still contain ANSI colour codes when redirected to a file: disable them when file logging lands (phase 3).

## Measurements behind phase 2 (Ryzen 5 2600X, RX 5500 XT, 1080p, 10 Mbit/s)

- **Maximum size is 2560x1440, not the 1920x1080 of PLAN.md chapter 6**: Alexis asked for it on 2026-11-22 (`encoded_size`). In 1440p encoding takes 10 to 11.5 ms and decoding 1.5 to 6 ms, so the spec's "encode + decode under 10 ms" cannot be met at that size; 60 fps holds because each side has its own thread (`pipeline` in `preview.rs`), as it will have its own PC. The bitrate is still 10 Mbit/s whatever the size.
- No test source reaches 60 fps in 1440p with a heavy picture or in 4K (the GStreamer patterns top out at 36 and 26 fps): those cases follow the source, 60 fps there is arithmetic, not a measurement.

- `h264_vaapi` takes 4.5 ms per frame in the GPU, whatever the options (the default preset is already the fastest; CAVLC instead of CABAC only saves 0.3 ms of decoding). With the colour conversion (1 ms) and the upload (1.2 ms), encoding is about 7 ms. Decoding is 1 to 3.5 ms depending on the picture. Encoding plus decoding is therefore 8 to 10.5 ms: the spec's 10 ms target is at the limit of this hardware, and is not met on every frame. Measure before adding anything per frame.
- Decoding is software on purpose: the GPU decodes in 1.5 ms but reading the picture back to CPU memory costs 3 ms more. Hardware decoding only pays once the picture stays on the GPU (phase 9). Phase 3 asks for D3D11VA: measure the read-back there before choosing.
- The encoder cuts each picture into 8 slices so the decoder can use 8 threads (`SLICES`): 4.5 ms with one slice, 2.5 ms with 8.
- swscale alone is too slow for 60 fps (4.6 ms for BGRA to NV12 in 1080p, 12 ms to reduce 1440p): `convert.rs` does BGRA to NV12 on several cores, and swscale only resizes afterwards, with threads.
- The VAAPI rate control spends the whole 10 Mbit/s even on a still picture, in CBR and in VBR. To look at in phase 7.
- Colours are BT.601 limited range on both sides and the stream does not say so: a shader or a hardware decoder added later must use the same matrix, or the encoder must start flagging it.
- The picture is still converted to RGBA on the CPU for an egui texture (2 ms per frame, shown as "affichage"). The NV12 shader of PLAN.md chapter 4 is not written yet. That conversion must stay on one thread: swscale's YUV to RGB writes past the end of each row, which leaves wrong pixels when rows are written in parallel (`whole_picture_is_right_at_awkward_widths`).
- An encoder is only chosen when it opens: if it fails on a later frame the capture stops, there is no retry with the next one. And a frame an encoder holds back (`Ok(None)`) only comes out with the next frame. Neither happens with VAAPI or libx264; check both with the phase 3 encoders (NVENC, AMF, QSV).
- `h264_vaapi` refuses pictures under 128 pixels a side on the RX 5500 XT: libx264 takes over for small windows.
- On Windows CI, libopenh264 warns that it cannot hold the bitrate without frame skipping. To look at in phase 3.
- FFmpeg's own messages go straight to stderr, not through `tracing`: route them when file logging lands (phase 3).
- Diagnostic variables: `MIROIR_ENCODER=<name>` forces an encoder, `MIROIR_PW_NODE=<id>` runs the whole loop on a PipeWire node without the portal (see `docs/test-manuel.md`). The 4K path could not be checked at 60 fps: the test pattern only reaches 26 fps at that size.
