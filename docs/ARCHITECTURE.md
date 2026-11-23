# Architecture and lifecycle

## Soniclaw integration

The application directly uses `soniclaw-core` processor/buffer/MIDI contracts, `soniclaw-sampler::MultiSampler` for fixed voice playback and round robins, `soniclaw-dsp` sinc interpolation/EQ/compressors/reverb/limiter, `soniclaw-engine` for device hosting, `soniclaw-device::OutputHost` for native audio, and `soniclaw-midi` input/SMF validation. The UI is native egui/eframe, not HTML. Symphonia decodes the original MP3s and imported files off the audio callback.

`content` validates and decodes the factory manifest. `model` owns projects, validation and flattened schedules. `engine` implements the shared live/offline processor and bounded control queues. `export` writes streamed WAV and SMF. `app` owns UI, history, lifecycle, device selection and workers. `main` dispatches desktop/headless operations.

The copied framework is Soniclaw **0.144.0**, from `soniclaw-rust-audio-framework-v0.144.0-source.zip`, SHA-256 `1372e74a4bdaa1f5c67778a17020c33378e785a8aabd887904cfe155e148e3af`. DSP algorithms are not modified. Packaging may omit unused examples and rendered fixtures; see the vendoring receipt.

## Threads and ownership

- Content decoding, custom-file validation, sample-bank construction and exports run before playback or on application workers.
- The UI opens/closes CPAL streams and MIDI connections. A new prepared instrument replaces the old stream while stopped; it does not destroy a live processor from another thread.
- One callback owns all sample voices and effects. Prepared slices, fixed voice slots and bounded events are used during processing.
- One-producer/one-consumer queues carry prepared schedules, audition triggers and captured MIDI hits. The callback returns old boxed schedules to a retirement queue, and the UI frees them. A full schedule queue causes UI retries rather than callback allocation.
- Each articulation has 16 voices plus Soniclaw's bounded retiring slot per voice. The app has at most 272 active voice slots, not an unlimited pool. All banks remain owned throughout playback. The original recording alternatives are immutable.
- Levels/pan/mute/solo move toward new targets over approximately 5 ms. Sound reconstruction is explicit for structural controls; historical processor states and tails are not transferred across reconstruction.
- MIDI capture records callback positions; the UI later inserts the returned hits into editable patterns. Hardware alignment/latency needs actual device testing.

## Persistence

The format discriminator is `iron-room-soniclaw`, schema 1; files use `.ironroom-s`. Factory references use stable articulation IDs. Custom samples store encoded file bytes and upper velocity bounds. Saves validate before writing a temporary sibling and renaming it. Invalid saves leave an existing named project unchanged. Only one instance should write a given project/autosave. Undo uses bounded project snapshots; it is not disk-backed.

The previous JUCE and browser formats are intentionally distinct and fail validation here. No older file is silently reinterpreted or migrated. Project title/name fields are UTF-8. Assets are resolved beneath the factory root; escaped/symlinked external factory references reject. User imports are explicit and embedded on save.

## Bounds and failure behaviour

128 patterns, 128 song clips, 16 bars/pattern, 32 repeats/clip, 8,192 hits/pattern and 65,536 flattened song hits. Duration <= four hours; project JSON <=128 MiB; custom base64 <=96 MiB total. Factory decoded PCM <=512 MiB; prepared per-articulation bank <=256 MiB. Sample decoding supports mono/stereo at 8–192 kHz, <=30 seconds/file. MIDI/articulation indices and all audio controls are validated. Additional allocations exist during preparation/history/decoding; these limits are not a single process-wide memory cap.

Each lane event buffer has 1,024 entries/callback; audition and record queues hold 256/2,048 entries. Overflow increments the app's dropped counter. Device MIDI uses its own bounded queue and telemetry. An unsupported output layout or device error leaves the UI available for editing/export. Invalid processor block shapes clear the output. WAV export refuses occupied filenames; cancellation removes only its newly created output. RIFF sizes over 4 GiB reject before file creation. MIDI export uses a bounded four-byte SMF delta representation.

## Current limits

- Standalone only; no VST3/AU/CLAP export or DAW host synchronization in this release.
- macOS/Windows build recipes and CI are provided; execution, installers, signing/notarization and physical-device acceptance must be recorded separately.
- Stereo device outputs; no audio-interface settings beyond output selection and no individual DAW channel outputs.
- Five kit configurations reuse one acoustic sample pool. The room is algorithmic; separate physical mic recordings cannot be reconstructed from these assets.
- Mixer EQ/compression/tuning/decay and room decay require Apply sound edits; playback restarts from its beginning. There is no seamless engine-state transfer.
- The limiter bounds sample peaks; it is not a certified true-peak mastering limiter.
- The editor is a first native implementation: no original JUCE project import, dedicated velocity lane, drag MIDI to DAW, separate kit snapshots or per-note articulation override.
- Undo snapshots cover most discrete edits; continuously dragged scalar knobs do not create a separate undo step for every intermediate value.
- Clock scheduling is deterministic for the same project/seed; hardware MIDI arrival and user audition are external inputs.
- Fast playing can exhaust the bounded per-articulation voice pool. Sinc playback and dense effects still require profiling on target CPUs.
