# Zwisper — Architecture (v4)

> A minimal, local-only **English** dictation and meeting-notes app.
> Rust + GPUI. **macOS ships first** (Apple Silicon, macOS 14.4+); Windows and Linux follow on the same codebase.
> Revision 4: macOS-first plan (§11). Revision 3: English-only; models chosen for the most mature, most portable runtimes; two hardware tiers (Standard / Lite).

## 0. Philosophy

**Apple, not Android.** One way to do each thing. The app makes the choices so the user doesn't have to:

- **A curated model set.** There is no model zoo, no "bring your own API key" and no provider settings.
- **One hardware choice.** The only model choice is *Standard* or *Lite*. The app picks for you based on your hardware, and you can override it with one switch.
- **Fully on-device.** The network is used only to download models, once.
- **macOS first.** v1 is macOS on Apple Silicon only. The ports and adapters (§3) keep Windows and Linux one adapter set away, but no v1 time is spent on them.
- **Compatibility over novelty.** Every model runs on a mature, widely deployed runtime (sherpa-onnx, llama.cpp) on all three OSes, on the CPU if necessary. We don't chase models that need new or unusual runtime support.
- **A fixed set of note templates.** Each one is tested against both tiers' LLMs.
- **Settings fit on one screen.**

There are exactly **two supported combinations** (Standard and Lite). Both are tested end to end in CI, so mismatched models, APIs and configs can't happen.

## 1. What it does

1. **Dictate.** Press ⌥Space (or hold it to talk), speak, and the text is pasted into the focused app.
2. **Record meetings.** A small meeting window shows the live transcript next to a notes pad where you type your own notes.
3. **Label speakers.** When the meeting ends, the remote voices are separated into Speaker 1, Speaker 2 and so on. You can rename them, and a new name applies everywhere.
4. **Turn the transcript into notes** with one of the built-in templates: Summary, Action items, To-do list, 1:1, Follow-up email. Your own typed notes are fed in as ground truth.

**Out of scope:**
- languages other than English
- cloud speech-to-text or cloud LLMs
- custom models
- dictation "modes" or AI rewriting of dictation
- calendar integration, auto-detecting meetings
- accounts and sync
- plugins
- remembered voices across meetings (possible later)

## 2. The curated models

Nothing ships in the installer. The first time a feature needs a model, the app offers a single download for it, with a progress bar. Only two runtimes are involved: **sherpa-onnx** (one ONNX Runtime) for all audio models, and **llama.cpp** for the LLM.

| Pack | Job | **Standard** | **Lite** | Runtime | License |
|---|---|---|---|---|---|
| **Speech** (required) | Speech-to-text | **Parakeet TDT 0.6B v2** (English), int8, ~660 MB | *same model* | `sherpa-onnx` | CC-BY-4.0 |
| | Voice activity detection | Silero VAD v5, ~2 MB | *same* | `sherpa-onnx` | MIT |
| **Speakers** | Diarization | pyannote **segmentation-3.0** (~6 MB) + English speaker embedding (WeSpeaker ResNet34 VoxCeleb, ~26 MB) | *same* | `sherpa-onnx` | MIT / Apache-2.0 |
| **Notes** | Rewriting transcripts | **Qwen3-4B-Instruct-2507**, Q4_K_M, ~2.5 GB | **Qwen3-1.7B**, Q4_K_M, thinking off, ~1.1 GB | `llama-cpp-2` | Apache-2.0 |

**Why these models**
- **Parakeet TDT 0.6B v2** is the English-only Parakeet. It is the most accurate small open English model on the Open ASR leaderboard: 6.05% average word error rate, against 7.4% for Whisper large-v3.
  - It produces punctuation, casing and word-level timestamps; speaker labelling depends on the word timestamps.
  - It runs at many times realtime on an ordinary 4-core laptop CPU, so it needs no GPU. That's why it doesn't need a Lite variant, which also saves us a whole test matrix.
  - Superwhisper, OpenWhispr and Handy all ship it.
- **Speaker detection** uses sherpa-onnx's built-in diarizer: pyannote segmentation-3.0, plus a speaker-embedding model, plus clustering.
  - It runs on the same ONNX Runtime and the same crate as speech, so there is one runtime for all audio AI.
  - Its models total about 30 MB and it runs on CPU on every OS.
  - The number of speakers is worked out automatically from a distance threshold, which we tune once against our test meetings.
  - The pyannote community-1 pipeline via `speakrs` is more accurate, but that crate is young and brings a second ONNX Runtime. It is a later upgrade (§15), not part of v1.
- **Qwen3-4B-Instruct-2507** (Standard) is a plain transformer that llama.cpp has supported for a long time. It has no thinking mode to manage and a 256K context.
  - It is the most compatible strong model under 3 GB.
  - With a q8 KV cache at a 16k-token context, it needs about 4 GB of RAM.
- **Qwen3-1.7B** (Lite) is the same family, so the same prompt templates behave the same way. Thinking is turned off, it has a 32K context and it needs about 2 GB of RAM.

**Tier selection**
- On first launch, a machine with **16 GB or more of unified memory** gets *Standard*; an 8 GB M-series Mac gets *Lite*.
- Settings has one switch: **Notes model: Standard (2.5 GB) / Lite (1.1 GB)**.
- Speech and Speakers are the same in both tiers. So the tier only changes the Notes download and the notes quality.
- Switching tiers downloads the other Notes model and offers to delete the old one.

**How models are delivered**
- **The model list is compiled into the app** as a manifest: pack, files, version, URL, SHA-256 and size. It cannot be edited at runtime.
- **Models are hosted on our own release bucket** (a GitHub Releases repo or a CDN), with a pinned Hugging Face URL as a fallback mirror. Self-hosting means upstream renames or gated repos can't break downloads, and the licenses allow it with attribution.
- **Downloads resume after interruption** and are verified against the SHA-256 checksum. They go into a staging folder and are moved into place atomically.
- **A model update means a new app version** with a new manifest. There are no silent model updates.
- **An About → Acknowledgements screen** gives the credits CC-BY requires (NVIDIA), plus credits for pyannote, WeSpeaker, Silero and Qwen.

**When models are loaded**

| State | Loaded | Approx. RAM |
|---|---|---|
| Idle | nothing | < 40 MB |
| Dictating | Parakeet + VAD (kept warm for 5 min after use) | ~0.9 GB |
| Meeting in progress | Parakeet + VAD | ~0.9 GB |
| After the meeting | Parakeet is unloaded → speaker detection runs → that is unloaded → Qwen loads, generates, unloads | peaks around 4 GB (Standard) / 2 GB (Lite) |

Only one heavy model is in memory at a time. The `Lazy` and `Exclusive` decorators (§3.5) enforce this.

---

## 3. System design: ports, adapters and strategies

**Goal: nothing is coupled to anything it doesn't need to know about.** Every engine, OS service and storage mechanism sits behind a trait (a *port*). Concrete implementations (*adapters*) are interchangeable *strategies*, and they are chosen in exactly one place: the **composition root**. The business logic and the UI never name a concrete model, library or OS API.

### 3.1 Layers and the dependency rule

```
                    ┌───────────────────────────────────────────────┐
                    │  app (composition root + GPUI shell)           │  picks strategies,
                    │  the ONLY crate that names concrete adapters   │  wires them together
                    └───────┬───────────────────────────┬───────────┘
                            │                           │
              ┌─────────────▼─────────────┐   ┌─────────▼──────────────────────────────┐
              │ ui  (GPUI views)           │   │ adapters/*  (strategies)                │
              │ depends on: core only      │   │ sherpa · llama · cpal · coreaudio ·     │
              │ talks via Commands/Events  │   │ wasapi · pipewire · fs-store · mac/win/ │
              └─────────────┬─────────────┘   │ linux platform · fakes (tests)          │
                            │                  │ each depends on: core only               │
                            │                  └─────────┬──────────────────────────────┘
                            ▼                            ▼
              ┌─────────────────────────────────────────────────────────────────┐
              │ core                                                              │
              │  domain types · port traits · services (use-cases) · event bus    │
              │  depends on: std + serde + a handful of pure crates. NO engines,  │
              │  NO GPUI, NO OS APIs.                                             │
              └─────────────────────────────────────────────────────────────────┘
```

**Rules, enforced by a CI check on `cargo metadata`:**
1. `core` depends on no adapter, no UI crate and no ML or OS crate.
2. Adapters depend only on `core`, never on each other.
3. `ui` depends only on `core`. It never imports an adapter or a model library.
4. Only `app` depends on both `ui` and the adapters.

### 3.2 Ports (traits in `core::ports`)

All ports are synchronous, `Send` and object-safe, used as `Box<dyn Port>` or `Arc<dyn Port>`. Threading is a separate concern (§3.5), so adapters stay simple.

| Port | Responsibility | v1 strategies (adapters) | Test double |
|---|---|---|---|
| `AudioSource` | Produce 16 kHz mono frames from a mic or system output | `CpalMic`, `CoreAudioTap`, `WasapiLoopback`, `PipeWireMonitor` | `WavFileSource` |
| `VoiceActivity` | Split frames into speech chunks | `SileroVad` (sherpa) | `EnergyVad` |
| `Transcriber` | Audio → segments with word timestamps | `ParakeetTranscriber` (sherpa) | `ScriptedTranscriber` |
| `Diarizer` | Audio → speaker turns | `SherpaDiarizer` | `ScriptedDiarizer` |
| `SpeakerAligner` | Words + turns → attributed paragraphs | `MaxOverlapAligner` | n/a (pure) |
| `EchoFilter` | Drop mic text duplicated from system audio | `TextSimilarityEcho` | `NoEcho` |
| `NoteWriter` | Prompt → streamed Markdown | `LlamaNoteWriter` (Qwen, Standard or Lite) | `CannedNoteWriter` |
| `TemplateCatalog` | Template set + prompts + output validation | `BuiltinTemplates` | n/a |
| `TextInjector` | Put text into the focused app | `PasteInjector` (with `KeystrokeInjector` as fallback, combined with `Fallback<A,B>`) | `RecordingInjector` |
| `HotkeyService` | Global shortcuts → `Command`s | `GlobalHotkey`, `WaylandPortalHotkey` | `ManualHotkey` |
| `SessionRepository` | Load and save sessions and dictations | `FsSessionRepo` (folders + JSON/MD) | `InMemoryRepo` |
| `AudioRecorder` | Persist tracks | `FlacRecorder` | `NullRecorder` |
| `ModelStore` | Manifest, download, verify, locate model files | `HttpModelStore` | `LocalDirModelStore` |
| `Permissions` | Query and request OS permissions | `MacPermissions`, `WinPermissions`, `LinuxPermissions` | `AllGranted` |
| `Clock` | Time | `SystemClock` | `FakeClock` |

Example:

```rust
// core::ports
pub trait Transcriber: Send {
    fn id(&self) -> EngineId;                       // for logs + session metadata
    fn transcribe(&mut self, audio: &AudioBuf, ctx: &TranscribeCtx) -> Result<Vec<Segment>, AsrError>;
}

pub trait NoteWriter: Send {
    fn id(&self) -> EngineId;
    fn generate(&mut self, req: &NoteRequest, out: &mut dyn TokenSink, cancel: &CancelToken)
        -> Result<(), NoteError>;
}
```

**Domain types** (`Segment`, `Word`, `SpeakerTurn`, `AudioBuf`, `NoteRequest`, `Session`, …) live in `core::domain`. They are plain data, so a strategy can be replaced without touching anything else.

### 3.3 Services (use cases in `core::services`)

Services hold the business logic. They depend **only on ports**, which are injected through their constructors:

- `DictationService(AudioSource, VoiceActivity, Transcriber, TextInjector, SessionRepository)`
- `MeetingService(AudioSource×2, VoiceActivity, Transcriber, AudioRecorder, SessionRepository)`
- `PostProcessPipeline`, an ordered list of `PipelineStep` strategies:
  - `FlushAsr`
  - `Diarize(Diarizer)`
  - `Align(SpeakerAligner)`
  - `DeEcho(EchoFilter)`
  - `WriteNotes(NoteWriter, TemplateCatalog)`
  - `Persist(SessionRepository)`

  Steps can be added, removed or reordered without touching the others.
- `NotesService(NoteWriter, TemplateCatalog, SessionRepository)`, which handles regenerating and switching templates.
- `ModelService(ModelStore)`, which handles downloads, tier switching and acknowledgements.

Services never know which OS, model or UI they are running under. Each one has its own unit tests that use the test doubles.

### 3.4 Messaging: Commands in, Events out

The UI and the services never call each other directly.

```rust
enum Command { StartDictation, StopDictation, CancelDictation, StartMeeting, PauseMeeting, EndMeeting,
               RenameSpeaker{..}, ReassignSegment{..}, GenerateNotes{template}, SetTier(Tier), DownloadPack(Pack), .. }

enum Event   { DictationState(..), Level(f32), LiveSegment(..), PipelineProgress(..), NotesDelta(..),
               SessionSaved(..), ModelProgress(..), PermissionChanged(..), Error(UserFacingError) }
```

- The `core::bus` module provides a typed `CommandBus` and an `EventBus` built on `crossbeam-channel`, with fan-out to multiple subscribers.
- The UI sends `Command`s and subscribes to `Event`s. The GPUI side drains events into view-model entities.
- Other front-ends become trivial adapters over the same bus: the tray, the global hotkey, a future CLI (`zwisper --dictate`) or an IPC socket for Wayland.
- The UI itself is a swappable presentation layer: replacing GPUI would mean rewriting `ui` only.

### 3.5 Cross-cutting behaviour as decorators, not in adapters

Policies wrap any port implementation, so adapters only contain the raw engine binding:

- **`Worker<P>`** runs a port on its own dedicated thread behind a channel. The threading model is therefore a strategy too: one shared thread, one thread per engine, or inline in tests.
- **`Lazy<P>`** loads the engine on first use and unloads it after an idle timeout, which is the memory policy from §2.
- **`Exclusive`** is a memory gate that makes sure only one heavy model is resident at a time.
- **`Timed<P>` / `Traced<P>`** add latency metrics and `tracing` spans.
- **`Fallback<A,B>`** tries A and then B (used for text injection).

```rust
let transcriber: Box<dyn Transcriber> =
    Box::new(Traced::new(Lazy::new(idle_5min, move || ParakeetTranscriber::load(&paths.speech))));
```

### 3.6 Composition root and hot-swapping

`app::compose` is the single place that names concrete types. It builds an `EngineSet` from the current **Profile** (tier × OS):

```rust
pub struct EngineSet {
    pub transcriber: Swappable<dyn Transcriber>,
    pub diarizer:    Swappable<dyn Diarizer>,
    pub notes:       Swappable<dyn NoteWriter>,
    pub injector:    Arc<dyn TextInjector>,
    pub capture:     Arc<dyn AudioSourceFactory>,
    /* … */
}

pub fn compose(profile: &Profile, models: &dyn ModelStore) -> EngineSet { /* match on profile → strategies */ }
```

- **`Swappable<T>`** is a thin wrapper around `arc_swap::ArcSwap`. A swap happens atomically *between* jobs: a running job keeps the instance it started with, and the next job picks up the new one. That is how switching Standard ↔ Lite, or replacing a model after an update, works at runtime with no restart.
- **Strategy selection is data-driven.** A `Profile` is a small struct: tier, OS capabilities, and which packs are installed. Adding a strategy means adding an adapter crate and one match arm in `compose`.
- **Cargo features** compile each adapter in or out (`--features sherpa,llama,coreaudio`). A build only contains the strategies its platform uses.
- **Hot-swap ≠ user-facing choice.** The architecture *can* swap anything at runtime. The *product* still exposes only the Standard/Lite switch (§0). Every other swap is for us: experiments, upgrades, tests and fallbacks.

### 3.7 Contract tests

`core::testkit` holds one reusable test suite per port. Every adapter must pass its port's suite, so a replacement can be dropped in with confidence. For example, the `Transcriber` contract checks:
- segments come out in order
- every word sits inside its segment
- timestamps never go backwards
- silence produces empty output
- the fixture clip scores under its word-error-rate threshold

The fakes pass the same suites, so service tests running on fakes match production behaviour.

### 3.8 Threading summary

**Rule:** the UI thread never blocks.
- Real-time audio callbacks only push into lock-free ring buffers.
- Each heavy port runs inside a `Worker`.
- Services run on a small set of controller threads.
- Results reach the UI only as `Event`s.

### 3.9 Workspace

```
zwisper/
├── Cargo.toml                    # workspace; gpui pinned to one zed rev
└── crates/
    ├── core/                     # domain, ports, services, bus, decorators, testkit  (no engines, no UI, no OS)
    ├── ui/                       # GPUI views + view-models; depends on core only
    ├── app/                      # composition root, profiles, main(); the only crate naming adapters
    └── adapters/
        ├── sherpa/               # ParakeetTranscriber, SileroVad, SherpaDiarizer
        ├── llama/                # LlamaNoteWriter (Qwen Standard/Lite)
        ├── templates/            # BuiltinTemplates (prompts + validators)
        ├── audio-cpal/           # CpalMic (+ PipeWire/WASAPI where cpal suffices)
        ├── audio-macos/          # CoreAudioTap
        ├── audio-windows/        # WasapiLoopback   (placeholder until M7)
        ├── audio-linux/          # PipeWireMonitor  (placeholder until M7)
        ├── store-fs/             # FsSessionRepo, FlacRecorder
        ├── models-http/          # HttpModelStore (manifest, download, verify)
        ├── platform-macos/       # hotkey, injector, permissions, tray, login item
        ├── platform-windows/     # (placeholder until M7)
        ├── platform-linux/       # (placeholder until M7)
        └── fakes/                # every test double, used by core tests and UI previews
```

Swapping Parakeet for another speech model is one new crate under `adapters/` plus one line in `compose`. Nothing else changes: not the services, the UI, storage or the other engines.

---

## 4. Audio

- **Mic:** `cpal` on all platforms.
- **System audio**, for meetings only:
  - **macOS (v1):** a Core Audio process tap (`CATapDescription` + `AudioHardwareCreateProcessTap`) on all output, added to a private aggregate device. It needs only the *System Audio Recording* permission and no screen-recording prompt. There is no ScreenCaptureKit fallback, because the minimum OS is 14.4.
  - Windows (later): WASAPI loopback.
  - Linux (later): PipeWire monitor of the default sink.
- **Pipeline:** OS callback → ring buffer → downmix → resample to 16 kHz mono f32 → three outputs at once:
  - a FLAC writer that flushes every 2 s, so a crash loses seconds, not the meeting
  - a level meter for the waveform, throttled to 30 Hz
  - Silero VAD, which cuts speech chunks at pauses of 500 ms or more, up to 20 s each
- **Separate tracks.** Mic and system audio are recorded and transcribed separately and never mixed:
  - the mic track is labelled "You" automatically
  - diarization only has to separate the remote voices
  - echo can be detected (§7)
- **In-person meetings:** if there's no system audio, the mic track alone is diarized.

## 5. Speech (`Transcriber` → `ParakeetTranscriber`)

- `ParakeetTranscriber` wraps sherpa-onnx's `OfflineRecognizer` with the English Parakeet TDT 0.6B v2 model. It is wrapped in `Worker` + `Lazy` + `Traced` (§3.5). A job is `{session, track, t0_ms, samples}`, and the result is `Vec<Segment>`, where each segment carries word timestamps.
- **Dictation:** the whole utterance (after silence trimming) goes through as one job. Parakeet's speed on CPU keeps a 10 s clip well under 1 s.
- **Meetings:** each VAD chunk becomes a job, so the live transcript runs a few seconds behind speech. These live segments are final; nothing is transcribed twice.
- **Acceleration:** CPU by default, which keeps it portable and predictable. The CoreML (mac) and CUDA (Windows/Linux) execution providers are compile-time options to evaluate later, not user settings.

## 6. Dictation

```
⌥Space down → pill appears, mic starts (in memory, no disk)
⌥Space up / second press / Esc
   ├─ Esc → cancel (no transcription)
   └─ stop → trim silence → Parakeet → inject text → pill fades out
```

- **Hotkey:** one shortcut. Tapping it toggles recording; holding it works as push-to-talk. The shortcut can be changed, but there is only ever one.
- **Text injection:**
  1. Save the clipboard.
  2. Put the text on the clipboard.
  3. Send ⌘V or Ctrl+V.
  4. Restore the clipboard after 150 ms.

  If pasting fails, it falls back to typing keystrokes (`enigo`). Empty results are dropped quietly.
- **No LLM is involved.** Parakeet's own punctuation and casing are good enough, and dictation stays instant.
- **Dictation keeps working during a meeting** and is kept out of the meeting recording.
- **Optional history:** each dictation is appended to `dictations/YYYY-MM-DD.md`. This is on by default and can be cleared.

## 7. Meetings

**Lifecycle:** Start (menu bar or ⌥⇧M) → meeting window opens → live transcript plus your notes → Pause/Resume → **End** → post-processing → finished note.

**Post-processing** is the `PostProcessPipeline` (§3.3), a list of `PipelineStep`s that runs in the background, with progress shown in the window:
1. **Flush and unload ASR.** Transcribe any remaining speech and unload Parakeet.
2. **Detect speakers.** Run the `Diarizer` (`SherpaDiarizer`) on the system track (or the mic track in in-person mode). The result is a list of speaker turns.
3. **Align.** Give each word the speaker turn it overlaps most, then group runs of words by the same speaker into paragraphs. Mic words are always "You".
4. **Remove echo.** Drop a mic paragraph if it overlaps in time with a system paragraph *and* their texts are at least 80% similar. This catches remote voices picked up from speakers.
5. **Write notes.** Run the default template (Summary) with the `NoteWriter` (Qwen; Standard or Lite). The input is the transcript, the speaker names and your private notes, which are marked as authoritative facts.
6. **Save** `session.json`, `transcript.md` and `notes.md`.

**Speakers:**
- Remote speakers appear as Speaker 1…N, each with a fixed colour.
- Click a name to rename it everywhere. Giving two speakers the same name merges them.
- A segment can be reassigned to a different speaker with right-click → *Speaker…*.

## 8. Notes (`NoteWriter` → `LlamaNoteWriter`)

`LlamaNoteWriter` loads whichever Qwen GGUF the current profile names: **Qwen3-4B-Instruct-2507** (Standard) or **Qwen3-1.7B** (Lite). Both use the same Qwen chat template, so the template prompts are shared.

**Fixed templates** (`BuiltinTemplates`), compiled in and tested against **both** tiers:

| Template | Output shape |
|---|---|
| **Summary** (default) | `## Summary` paragraph, `## Decisions` bullets, `## Action items` `- [ ] Owner: task` |
| **Action items** | `- [ ] Owner: task (due if stated)` only |
| **To-do list** | Your own to-dos only, `- [ ]` |
| **1:1** | Updates, Wins, Blockers, Feedback, Follow-ups |
| **Follow-up email** | Subject line + short email body |

- **Generation settings:**
  - Standard: the 2507 instruct model, which has no thinking mode
  - Lite: Qwen3-1.7B with `enable_thinking=false`
  - instruct sampling (temperature 0.7, top-p 0.8)
  - context window of 32k (Standard) or 24k (Lite)
  - q8 KV cache
  - output streams token by token into the notes pane
- **Output format:** each template asks for strict Markdown with fixed headings. The renderer turns `- [ ]` lines into checkboxes. If a required heading is missing, the app retries once with a lower temperature.
- **Long meetings:** if the input is over the tier's budget (~24k tokens for Standard, roughly 2 hours; ~16k for Lite, roughly 80 minutes), the transcript is summarised in time-ordered chunks and the chunk summaries are combined.
- **Results are kept per template,** so switching templates is instant once generated. *Regenerate* replaces only the current template's result. Editing a result in place is allowed and stored.
- **No custom prompts in v1.** A single free-form "Ask about this meeting…" box is the most this should ever add.

## 9. UI specification

**Visual language** (modelled on Superwhisper; our own implementation):
- **Surfaces:** opaque charcoal in dark mode (`#2B2B2B` window, `#333` raised, `#3A3A3A` inset), with the light theme generated from the same tokens. Follows the system appearance.
- **Text:** primary white at 92%, secondary at 60%, tertiary at 40%. System UI font: SF Pro, Segoe UI Variable, or Inter on Linux. 13–15 pt body, bold headings.
- **Shape:** window corner radius 20, cards 12, and capsules for buttons, players and keycap chips. Hairline 1 px outlines at 8% white.
- **Colour is only used to carry meaning:**
  - Speaker names: You = lime, then blue, purple, orange, pink, teal.
  - Recording dot: red.
  - Everything else is monochrome.
- **Icons:** thin line icons in one icon set.
- **Motion:** 150–200 ms ease-out fades and resizes, and a live waveform. Nothing bounces.
- **Density:** generous padding. Keyboard hints are shown inline as keycap chips.

### 9.1 Dictation pill
A floating, borderless, always-on-top window about 420×110 pt, centred near the bottom of the active screen.

```
╭──────────────────────────────────────────────────────╮
│                                                      │
│  ·····················∿∿∿∿∿∿······················   │  ← live waveform (dotted when idle)
│ ╭──────────────────────────────────────────────────╮ │
│ │ 🎙 Dictation          Stop [⌥][Space]  Cancel [esc]│ │
│ ╰──────────────────────────────────────────────────╯ │
╰──────────────────────────────────────────────────────╯
```

**States:**
1. **Listening:** the waveform moves.
2. **Transcribing:** the waveform settles into a soft shimmer, usually for under a second.
3. **Done:** the pill fades out.
4. **Error:** the pill turns red briefly, for example "No microphone access", with a link to fix it.

There is no timer and there are no modes. There is no mini mode in v1.

### 9.2 Meeting window
A normal window, about 900×600 by default and resizable, that remembers its frame.

```
╭──────────────────────────────────────────────────────────────────────────────╮
│ ● ●            Weekly product sync   [📄 Summary ⌃⌄]          [Copy] [···] [▥] │
├─────────────────────────────────────────────┬────────────────────────────────┤
│ Weekly product sync                         │                    [🔍] [👥]   │
│                                             │ You                            │
│ Summary                                     │ Okay, let's start with the     │
│ The team agreed to ship on Thursday.        │ launch checklist…              │
│                                             │ Speaker 1                 0:06 │
│ Decisions                                   │ Release notes are drafted…     │
│ • Ship Thursday                             │ Speaker 2                 0:11 │
│                                             │ I can get you that by EOD…     │
│ Action items                                │                                │
│ ◯ Priya: send announcement draft Wed        │                                │
│                                             │                                │
│ My notes                                    │                                │
│ ask Jordan about export format_             │                                │
│                                             │                                │
│        ╭────────────────────────────╮       │                                │
│        │ ● 12:34   ❚❚ Pause   ■ End  │       │                                │
│        ╰────────────────────────────╯       │                                │
╰─────────────────────────────────────────────┴────────────────────────────────╯
```

- **While recording:**
  - The left pane is just the title and **My notes**, a plain-text editor with the cursor already in it.
  - The right pane is the live transcript, auto-scrolling. Scrolling up pauses auto-scroll and shows a "Jump to live" pill.
  - The floating control capsule shows the red dot, the elapsed time, Pause and End. End asks for confirmation.
- **⌘M collapses the window into a tiny recording pill** (red dot plus timer) that stays on top. Clicking it restores the window.
- **After End:**
  - The left pane shows a "Writing notes…" shimmer, then the generated note streams in above **My notes**, which stays below and is dimmed.
  - The capsule turns into a playback player (▶, time, 1×). Clicking a transcript timestamp seeks the player there.
- **Toolbar:**
  - centred title, editable
  - the template picker (document icon with ⌃⌄), listing the five templates
  - **Copy**, which copies the note as Markdown
  - **···**: Copy transcript, Show in folder, Regenerate, Delete
  - **▥**, which hides the transcript and leaves a single centred reading column
- **Transcript header:** **🔍** searches within the transcript. **👥** opens *Speakers*: one tinted card per speaker with a sample line, ▶ to hear a sample, and an editable name.

### 9.3 Library
A single window that opens from the menu bar.
- **Header:** "Meetings", a search field, and a **+** button that starts a meeting.
- **Body:** meetings grouped by day (*Today*, *Yesterday*, dates). Each meeting is a rounded card with a bold title, the time on the right, and the summary's first line in grey underneath. A "Recording now" card is pinned at the top while recording.
- **A segmented control at the top** switches between **Meetings** and **Dictations**. Dictations is a plain day-grouped list with a copy button on each item.
- **Opening a meeting** opens it in the meeting window.
- There is no sidebar, no stats dashboard and no tabs beyond these two.

### 9.4 Menu bar / tray
Dictate `⌥Space` · Start meeting `⌥⇧M` · Open Library · Settings… · Quit.
The icon changes to a filled red dot while a meeting is recording.

### 9.5 Settings (one screen, grouped cards)
- **Shortcuts:** Dictate, Start meeting.
- **Microphone:** a picker (System default by default).
- **Meetings:** "Record system audio" (on by default).
- **Models:**
  - one switch: **Notes model: Standard / Lite**, with a one-line hint ("Lite uses less memory; notes are a little less polished")
  - three rows (Speech, Speakers, Notes), each with a status (Not downloaded / Downloading 43% / Ready), its size, and Download or Remove
- **Storage:** library folder, with a Change button and Show in Finder; "Keep dictation history" toggle.
- **General:** Launch at login.
- **About:** version and acknowledgements.

### 9.6 Onboarding (first launch, 3 steps)
1. **Welcome**, followed by the Speech model download. The download continues in the background while the next steps run.
2. **Permissions,** with a live status for each: Microphone, Accessibility (for pasting), and System audio (for meetings).
3. **Try it:** "Press ⌥Space and say something", with a sample text field.

The Speakers and Notes packs are offered the first time a meeting ends ("Download speaker labels and notes, 2.5 GB?", or 1.1 GB on Lite), so dictation-only users never download them.

---

## 10. Storage

```
~/Zwisper/
├── Meetings/2026-11-17 1030 Weekly product sync/
│   ├── session.json        # source of truth
│   ├── transcript.md       # rendered for humans; regenerated on change
│   ├── notes.md            # current template's note
│   └── audio/{mic.flac, system.flac}
└── Dictations/2026-11-17.md
```

```rust
struct Session {
    version: u32, id: Ulid, title: String, created: DateTime, duration_ms: u64,
    tracks: Vec<Track>,                       // { source: Mic|System, file }
    speakers: Vec<Speaker>,                   // { id, name, color_idx, is_you }
    segments: Vec<Segment>,                   // { id, track, speaker_id, start_ms, end_ms, text, words, edited }
    my_notes: String,
    notes: BTreeMap<Template, NoteResult>,    // { markdown, generated_at, model_version, edited }
}
```

- **Writes** are atomic: write to a temp file, then rename.
- **The library** is built by scanning folders, with an in-memory cache. Search is a substring scan over the Markdown files, with no index.
- **Config** is `config.toml` in the OS config directory. It holds no secrets, because there are none.
- **Models** are stored in the OS data directory, e.g. `…/Zwisper/models/{speech,speakers,notes}/<version>/`.

## 11. Platform layer — macOS first

**v1 target:** Apple Silicon (M1 or later) on macOS 14.4 or later.
- **Apple Silicon only:** all Macs sold since 2023 are Apple Silicon, Apple is phasing out Intel, and Metal makes llama.cpp fast. It also leaves one CPU architecture to test.
- **macOS 14.4 or later:** that is when Core Audio process taps arrived, which avoids the screen-recording permission.

Only the `adapters/*-macos` crates are built in v1. The Windows and Linux adapter crates exist as empty placeholders, so the workspace layout and the composition-root match arms are ready for them.

| Port | macOS adapter (v1) | Notes |
|---|---|---|
| `HotkeyService` | `global-hotkey` (Carbon `RegisterEventHotKey`) | Push-to-talk needs key-up events: Carbon provides them; verify in M0 |
| `TextInjector` | Pasteboard + `CGEventPost` ⌘V; `CGEvent` unicode typing as fallback | Needs **Accessibility**. Restores the pasteboard, including non-text items |
| `AudioSource` (mic) | `cpal` (CoreAudio) | Needs **Microphone** |
| `AudioSource` (system) | `CoreAudioTap` via `objc2-core-audio` | Needs **System Audio Recording** (`NSAudioCaptureUsageDescription`) |
| `Permissions` | `AVCaptureDevice.authorizationStatus`, `AXIsProcessTrusted`, a tap-creation probe | Onboarding shows live status and deep-links into System Settings → Privacy & Security |
| Tray | `tray-icon` (`NSStatusItem`) | Template image, so it follows the menu bar's appearance |
| Login item | `SMAppService.mainApp` via `objc2-service-management` | A single toggle |
| Floating pill | GPUI window, then the underlying `NSWindow` adjusted through `objc2` to behave like a **non-activating `NSPanel`** (floating level, joins all Spaces, ignores ⌘Tab) | It must **never take focus**, or the paste lands in the wrong app. This is an M0 spike |
| App activation | `LSUIElement` = menu-bar app, no Dock icon. Switches to `.regular` while the Library or meeting window is open, so it shows up in ⌘Tab, then back again | |

**Acceleration on Mac:**
- llama.cpp uses **Metal**. At Q4, the Standard model should comfortably beat reading speed on M1-class hardware.
- Parakeet and diarization run on CPU through sherpa-onnx. That is fast enough on M-series. The CoreML execution provider is an optional later optimisation, swapped in behind the same `Transcriber` port.

**Later ports** (same `core`, `ui` and services; new adapters only):

| | Windows | Linux |
|---|---|---|
| Hotkey | `global-hotkey` | `global-hotkey` (X11) / GlobalShortcuts portal (Wayland) |
| Injection | `SendInput` paste | XTest / RemoteDesktop portal, `wtype` fallback |
| System audio | WASAPI loopback | PipeWire monitor |
| LLM acceleration | Vulkan / CUDA | Vulkan |

## 12. Dependencies

Dependencies are grouped by the crate that owns them. `core` stays nearly dependency-free.

| Crate | Dependencies |
|---|---|
| `core` | `serde`, `thiserror`, `crossbeam-channel`, `arc-swap`, `ulid`, `chrono`, `strsim`, `tracing` |
| `ui` | `gpui` (pinned zed rev), `gpui-component` |
| `adapters/sherpa` | `sherpa-onnx` (Parakeet, Silero VAD, diarizer; one ONNX Runtime) |
| `adapters/llama` | `llama-cpp-2` (Metal / Vulkan features) |
| `adapters/audio-*` | `cpal`, `ringbuf`, `rubato`; `objc2-core-audio` (mac), `wasapi` (win), `pipewire` (linux) |
| `adapters/store-fs` | `flacenc`, `claxon`, `serde_json`, `toml`, `directories` |
| `adapters/models-http` | `ureq`, `sha2` |
| `adapters/platform-*` | `global-hotkey`, `tray-icon`, `arboard`, `enigo`, `ashpd` (linux), `auto-launch` |
| `app` | all of the above, behind cargo features, plus `anyhow` and `tracing-subscriber` |

There is no async runtime: GPUI's executor plus plain threads is enough. There is **one ONNX Runtime** (bundled by sherpa-onnx) and **one llama.cpp**.

## 13. Build and distribution

- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip`, `panic = "abort"`.
- **Target installer size:** under 40 MB. Most of that is ONNX Runtime and llama.cpp; models are downloaded separately.
- **macOS (v1):**
  - an `arm64` `.app` with `LSUIElement`, `LSMinimumSystemVersion` 14.4, `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription`
  - hardened runtime, signed with a **Developer ID** certificate and notarized (`notarytool`), shipped as a `.dmg`
  - not sandboxed and not on the Mac App Store in v1: the Accessibility-based paste and the global hotkeys work much more simply outside the sandbox
  - auto-update through a Sparkle-style appcast. An `Updater` port with a Sparkle adapter is decided in M6
- **Windows / Linux (later):** signed MSI; AppImage and a Nix flake.
- **CI** (GitHub Actions on `macos-14` arm64 runners in v1; Windows and Linux runners added later, with only `core` tests running there until then) runs:
  - a **dependency-rule check** (§3.1) over `cargo metadata`
  - **contract tests**: every adapter against its port's suite (§3.7)
  - **service tests** on fakes: dictation, meeting and the pipeline, with no models and no OS
  - unit tests for alignment, echo dedupe, the manifest/verifier and template output parsing
  - a gated "golden" test that runs the real models on a 2-minute, three-speaker fixture recording and checks word error rate, diarization error and that every template's required headings are present. It runs once per tier (Standard and Lite), the only two supported combinations

## 14. Milestones

| # | Deliverable | Exit criteria |
|---|---|---|
| **M0 — Spikes** | (a) sherpa-onnx (Parakeet v2, VAD, diarizer) builds and runs on all 3 OSes. (b) `llama-cpp-2` runs both Qwen tiers. (c) Parakeet CPU speed on a base M1 with 8 GB. (e) A non-activating GPUI pill window plus Core Audio tap capture on macOS 14.4. (d) Diarization threshold tuned on fixture meetings | All five pass, or the fallback is chosen (§15) |
| **M1 — Core** | `core`: domain types, ports, bus, decorators, testkit, fakes. Services running fully on fakes | Dictation and meeting flows pass headless on fakes; dependency-rule check is green |
| M1.5 | Skeleton: GPUI shell, tray, settings, `models-http`, onboarding, composition root | Speech pack downloads and verifies |
| M2 | **Dictation** end to end on macOS | ⌥Space → text pasted into Notes, Safari, Slack and VS Code in under 1 s on an M1 |
| M3 | **Meeting recording:** both tracks, FLAC, live transcript, meeting window, my-notes pad | A 1-hour meeting recorded reliably, with live text |
| M4 | **Speakers:** diarization, alignment, echo dedupe, renaming | A 3-person call is labelled correctly |
| M5 | **Notes:** five templates, streaming, per-template results | Every template passes the golden test |
| M6 | **macOS release:** signing, notarization, DMG, auto-update, onboarding polish | Public v1 for Apple Silicon |
| M7+ | Windows, then Linux adapters | The same contract and golden tests pass on each OS |

## 15. Risks and pre-chosen fallbacks

Each fallback is decided now so it doesn't turn into a settings option later. Thanks to §3, every fallback is an adapter swap in `compose`, with no ripple through the rest of the app.

| Risk | Fallback (replaces the primary; never offered alongside it) |
|---|---|
| sherpa-onnx diarization is not accurate enough | Add `adapters/speakrs` (pyannote community-1, PLDA/VBx clustering), but only once it can share sherpa's ONNX Runtime |
| Parakeet is too slow on low-end CPUs | Lite gets **Moonshine base** (English, sherpa-onnx). Its segment-level timestamps are aligned using VAD chunk bounds |
| Qwen3-1.7B notes are too weak | Lite uses **Qwen3-4B-Instruct-2507** at a smaller quant (Q3_K_M), so there is still one model family |
| Users need other languages | Out of scope. If it changes, add a Whisper `Transcriber` adapter and an English/multilingual profile |
| GPUI API churn | Pin the rev; the UI is isolated in `ui` and depends only on `core` |
| Wayland limits on hotkeys and injection | `WaylandPortalHotkey` / portal injector adapters. A compositor-bound shortcut can call `zwisper --dictate`, which goes through the IPC front-end onto the same command bus |
| Echo when using speakers instead of headphones | `TextSimilarityEcho` now; a WebRTC echo-cancellation `EchoFilter` later |
| Upstream model URLs change or get gated | Self-host the pinned files; the manifest checks SHA-256 |
| Over-abstraction slows delivery | Ports exist only at real seams (engines, OS, storage, UI). Inside an adapter or service, plain code |

## 16. Open decisions

1. **App name.**
2. Confirm the template set: Summary, Action items, To-do list, 1:1, Follow-up email.
