# Zwisper — Handoff

The project moved from a Linux cloud session to local development on the Mac. Read this first, then [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design and the [README](../README.md) for build commands.

## TL;DR

- **What it is:** a minimal, local-first dictation and meeting-notes app for macOS 14.4+ on Apple Silicon. It's written in Rust with GPUI and uses a ports-and-adapters design with exactly one curated model per job.
- **Done and tested on Linux:**
  - the core: domain types, interfaces (ports), services and command/event bus
  - storage, the note templates and the model downloader
  - the whole UI, running in `--demo` mode
- **Written but never compiled for macOS:** the macOS adapters and the macOS composition root. Expect compile errors there. **This is the first job.**
- **Unfinished:** the speech/notes engine adapters (`sherpa`, `llama`). That work was still running when this was handed off; its latest state is on the **`wip/engines`** branch.

## Decisions already made (don't relitigate)

| Topic | Decision |
|---|---|
| Platform | macOS first (Apple Silicon, 14.4+). Windows and Linux are later, as new adapters only |
| UI | **GPUI Kit** (`gpui-kit` =0.10.0, [gpui-kit.com](https://gpui-kit.com)), used under the crate name `gpui`. It bundles `gpui-pre` (weekly snapshots of Zed's gpui; the crates.io `gpui` hasn't been released since Oct 2025) and gpui-component. Changed 2026-11-17. Windows open through `gpui::open_window` (adds the kit's `Root`); the pill stays custom. Kit components are themed with our palette in `state.rs::apply_component_theme`. Upgrade by bumping the single `gpui-kit` pin (and the `gpui-pre-platform` pin in `crates/ui/Cargo.toml`, which only switches on runtime shaders) |
| Language | English only |
| Product philosophy | "Apple, not Android": one model per job and a fixed set of five templates. The only model choice is the Standard/Lite tier; no model picker, no cloud, no API keys |
| Models | Speech: Parakeet TDT 0.6B v2 int8 (sherpa-onnx). VAD: Silero v5. Speakers: pyannote seg-3.0 + WeSpeaker ResNet34 (sherpa-onnx). Notes: Qwen3-4B-Instruct-2507 Q4_K_M (Standard) or Qwen3-1.7B Q4_K_M (Lite), via llama.cpp. All pinned with SHA-256 in `crates/adapters/models-http/src/manifest.rs` |
| Tier | 16 GB RAM or more → Standard, otherwise Lite. Only Notes differs by tier |
| Distribution | Developer ID-signed `.dmg`, **not** sandboxed and not on the Mac App Store (global hotkeys and synthetic paste need that) |
| Templates | Summary (default), Action items, To-do list, 1:1, Follow-up email. No custom prompts |

## Repository map

```
crates/core/          domain, ports (traits), services, bus, decorators (Lazy, Swappable, Fallback), testkit contracts
crates/ui/            GPUI views — depends on core ONLY (enforced)
crates/app/           composition root: src/demo.rs (any OS), src/mac.rs + engines.rs + tray.rs (macOS)
crates/adapters/
  fakes/              test doubles for every port
  templates/          the 5 prompts + validators
  store-fs/           session folders (JSON + MD), WAV tracks, dictation log, TOML settings
  models-http/        manifest + resumable download + SHA-256
  sherpa/             ParakeetTranscriber, SileroVad, SherpaDiarizer          (see wip/engines)
  llama/              LlamaNoteWriter (Variant::Standard | Lite)              (see wip/engines)
  audio-macos/        MacAudio: cpal mic + Core Audio process tap             (never run)
  platform-macos/     hotkeys, Paste/Typing injectors, permissions, login item, window helpers (never run)
third_party/xattr/    patched copy (see Gotchas)
packaging/macos/      Info.plist, entitlements
scripts/bundle-macos.sh
```

**Dependency rule** (`crates/core/tests/architecture.rs`):
- `core` has no engine, UI or OS dependencies.
- Adapters depend only on `core`, never on each other.
- `ui` depends only on `core`.
- Only `app` names concrete adapters.

## How it fits together

- **Commands in, events out.** The UI (plus hotkeys and the tray) sends `Command`s to `Core`, which runs them on a controller thread. The UI subscribes to `Event`s, which reach GPUI through `AppState::attach` and an async channel.
- **`AppState`** (`crates/ui/src/state.rs`) is the view-model. It also owns window lifecycle.
- **Dictation:** `services/dictation.rs`. Tapping the key toggles recording; holding for ≥350 ms is push-to-talk. Then: trim silence → transcriber → `TextInjector`.
- **Meetings:** `services/meeting.rs`. Mic and system audio are recorded as separate tracks. Each runs through `Capture<TrackProcessor>`, which feeds the WAV writer and the VAD; VAD chunks go to the transcriber and come out as `LiveSegment` events.
- **After End:** `services/pipeline.rs` runs UnloadSpeech → DetectSpeakers (diarize, then `MaxOverlapAligner`) → RemoveEcho → WriteNotes, then saves.
- **Notes:** `services/notes.rs` handles long meetings with map-reduce, validates against the template, retries once, and stores results per template.
- **Engines** are `Swappable<dyn …>` in `core/src/engines.rs`. `SetTier` hot-swaps them through the `EngineFactory` (`app/src/engines.rs` wraps each one in `Lazy` with a 5-minute idle unload).

## Next steps, in order

1. **Get it compiling on the Mac.**

   ```bash
   cargo build -p zwisper
   ```

   Expect errors in `crates/app/src/{mac,tray,engines}.rs`, `crates/adapters/{audio-macos,platform-macos}` and the engine crates. The two macOS adapter crates did pass `cargo check --target aarch64-apple-darwin` on Linux. The app crate could not be checked there, because ring, GPUI's Metal shaders and llama.cpp need the macOS SDK.

2. **Merge the engine work.**
   - Look at `wip/engines`. It may be incomplete, and its real-model tests (`cargo test -p zwisper-sherpa -p zwisper-llama -- --ignored`) download about 4 GB.
   - Finish or fix it so `ParakeetTranscriber`, `SileroVad`, `SherpaDiarizer` and `LlamaNoteWriter` pass the contract suites in `crates/core/src/testkit.rs`.
   - Check that the constructor names match what `app/src/engines.rs` calls: `ParakeetTranscriber::load(&ModelPaths)`, `SileroVad::load`, `SherpaDiarizer::load`, `LlamaNoteWriter::load(&ModelPaths, Variant)`, `Variant::engine_id()`.
   - Decide how sherpa-onnx's native libraries link on macOS (static or bundled dylib); `scripts/bundle-macos.sh` copies `@rpath` dylibs into `Frameworks/`.
3. **Run the bundled app** and test real permissions.

   ```bash
   ./scripts/bundle-macos.sh
   ```

   ```bash
   open target/release/bundle/Zwisper.app
   ```

   macOS ties permissions to the signature, so test from the `.app`, not `cargo run`.
4. **Work through the `VERIFY ON MAC` list** below.
5. **Check end to end:**
   - ⌥Space pastes into Notes, Safari, Slack and VS Code in under 1 s.
   - A 3-person call is labelled correctly.
   - Each template produces a valid note.
6. **Polish:** see Known gaps.

## `VERIFY ON MAC` items (grep for the marker)

1. **tap.rs, denial error code:** which OSStatus a denied System Audio permission actually produces. Only `kAudioDevicePermissionsError` is mapped today.
2. **tap.rs, silence on denial:** a denial may just produce silence rather than an error. Consider an "all-zero audio" check that sends the user to Settings, and call `mark_system_audio_granted()` once real audio arrives.
3. **tap.rs, AirPlay/Bluetooth:** whether using the default output device as the aggregate's main sub-device breaks with AirPlay or Bluetooth output.
4. **tap.rs, buffer format:** whether IOProc buffers really arrive in the tap's format.
5. **tap.rs, teardown:** whether `AudioDeviceDestroyIOProcID` waits for an in-flight cycle before the state is freed.
6. **mic.rs:** whether a denied microphone opens fine and delivers silence. Check `Permissions::state(Microphone)` before recording.
7. **convert.rs:** that 24-bit audio is packed in 32 bits, low-aligned.
8. **window.rs:** that GPUI `WindowKind::PopUp` is a non-activating NSPanel. The source suggests it is (`gpui/src/platform/mac/window.rs`, `NSWindowStyleMaskNonactivatingPanel`), so the pill already uses `kind: PopUp, focus: false`. Confirm the pill never steals focus, or the paste lands in the wrong app.
9. **window.rs, private call:** that `_setPreventsActivation:` exists. It's private API and only used in the fallback path.
10. **window.rs, style toggle:** that setting the NonactivatingPanel style after creation takes effect.
11. **window.rs, class swap vs KVO:** that the class swap doesn't clash with KVO.
12. **permissions.rs:** that the `Privacy_AudioCapture` settings URL opens the right pane on 14.4 and 15.
13. **permissions.rs, threading:** that NSWorkspace `openURL` is fine off the main thread.
14. **inject.rs, layouts:** key code 9 is ⌘V only on QWERTY-like layouts. Dvorak needs UCKeyTranslate.
15. **inject.rs, threading:** that NSPasteboard is safe from the background restore thread.
16. **inject.rs, newlines:** how terminals and Electron apps handle "\n" from the typing fallback.

Also unverified: the tray icon (`tray-icon` 0.26, created inside `Application::run`), `MacHotkeys::new()` (must be on the main thread; it's called from `mac::compose` inside `Application::run`), and `set_activation_policy_accessory` toggling when windows open and close.

## Known gaps / TODO

- **Audio format:** stored as 16-bit WAV, about 115 MB per hour per track. The design calls for FLAC.
- **Shortcuts:** no shortcut editor in Settings (they're display-only; change them in `config.toml`). Hotkeys are bound only at startup, so changing settings doesn't rebind them.
- **Esc-to-cancel:** not global yet; the pill has a clickable Cancel. Bind Esc dynamically only while dictating.
- **Meeting window:** no ⌘M mini-pill and no audio playback.
- **Microphone picker:** cycles through devices on click instead of showing a real dropdown.
- **Real-time allocations:** the RT audio path allocates a `Vec` per callback (`Capture` and the adapters). This is fine for an MVP; use a ring buffer later.
- **Port changes suggested by the macOS adapter work, not yet done:**
  - a health or error channel on `AudioSource`, for device unplugged or silent-because-denied
  - stable device IDs instead of names
  - documenting thread requirements on `HotkeyService`
- **Packaging:** no auto-update and no DMG or notarization steps yet (`bundle-macos.sh` does ad-hoc or Developer ID signing only).

## Gotchas learned the hard way

- **GPUI re-entrancy:** never open a window inside `AppState::update`, because the new view reads `AppState` in its constructor and that panics ("cannot read … while it is already being updated"). All window opens go through `cx.defer`; keep it that way.
- **GPUI hover:** calling `.hover()` twice on one element panics ("hover style already set"). That's why `widgets::primary` doesn't wrap `capsule`.
- **xattr on Linux:** `xattr` 0.2.3, pulled in by `gpui_http_client`, doesn't build against current `libc` on Linux. It's patched in `third_party/xattr` through `[patch.crates-io]`. It's harmless on macOS, but remove the patch once upstream fixes it.
- **Linux screenshots:** GPUI on Xvfb only paints with a window manager running (`openbox`). Theme override: `ZWISPER_THEME=dark|light`.
- **Downloader proxy and certs:** `ureq` is built with `native-certs` and honours `HTTPS_PROXY`.
- **Model URLs:** pinned to Hugging Face commit SHAs. Silero comes from `raw.githubusercontent.com/snakers4/silero-vad/v5.1.2`. Self-host them before release.

## Testing

Fast tests: no models, no OS, run anywhere:

```bash
cargo test -p zwisper-core -p zwisper-templates -p zwisper-store-fs -p zwisper-ui
```

Real models, downloaded on first run:

```bash
cargo test -p zwisper-sherpa -p zwisper-llama -- --ignored
```

UI without models or audio:

```bash
cargo run -p zwisper -- --demo --open meeting
```

`--open` also takes `library`, `settings`, `onboarding`, `live` and `pill`.

The core has 27 tests (services on fakes, contracts, architecture rule, DSP, alignment and echo), all passing as of the handoff. CI (`.github/workflows/ci.yml`) has a Linux job and a `macos-14` job that builds and uploads an unsigned `.app`. The macOS job had not completed at handoff time.
