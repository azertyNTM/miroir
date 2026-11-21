# Miroir

Read `PLAN.md` first: it is the spec (architecture, technical choices, phases, validation criteria).

- Work one phase at a time, in the order of PLAN.md chapter 9. At the end of a phase: stop, summarise, and tell Alexis exactly what to test by hand.
- UI text in French. Code and comments in English.
- `tracing` everywhere; log level via `MIROIR_LOG`.
- Check the latest version of a crate on crates.io and read its docs before adding it.
- Alexis has no Windows machine: Windows is tested through CI and on the friend's PC.

## Progress

- Phase 0 (foundations): done — workspace, config, logging, CI green on Linux and Windows (FFmpeg links on both).
- Phase 1 (Linux capture with local preview): built on branch `phase-1-capture-linux`, waiting for Alexis to run `docs/test-manuel.md` on his PC (the portal dialog and the picture cannot be checked automatically).
- Next: phase 2 (local encode/decode loop), once phase 1 is validated.

## Notes for later phases

- The PipeWire consumer can be exercised without the portal: `docs/test-manuel.md` explains how to publish a GStreamer test pattern and read it with the `pw_node` example.
- PipeWire does not report an error when the captured node disappears (the stream just pauses): the end of a capture comes from the portal session's `Closed` signal.
- `VideoSource::next_frame` waits up to 100 ms and returns `Ok(None)` when nothing changed on screen: phase 2 must resend the last frame itself (PLAN.md chapter 6, "image fixe").

- GitHub repo: `azertyNTM/miroir` (public). Commits use the GitHub noreply address set in the repo-local git config.
- Windows CI uses the BtbN LGPL shared FFmpeg build: no `libx264` there, the software fallback on Windows is `libopenh264`.
- Linux CI (Ubuntu) builds against an older FFmpeg than Arch and the Windows CI (9.0): if CI is red but local is green in phase 2, run the Linux job in an `archlinux` container.
- Logs still contain ANSI colour codes when redirected to a file: disable them when file logging lands (phase 3).
