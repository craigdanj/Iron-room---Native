# Iron Room · Soniclaw Edition

A native Rust desktop acoustic drum sampler built with **Soniclaw 0.144.0**.
This is a separate implementation of Iron Room; no JUCE or browser runtime is used.

## Included

- The existing Salamander content: **201 recordings, 17 articulations, 11 mixer channels**.
- **2,375 original grooves**, searchable by name, family, feel and meter. Fractional triplet timing and odd meters are retained.
- Five acoustic-kit configurations drawn from the same recordings, recorded velocity layers, round robins, seven intermediate hi-hat positions, short-release hi-hat/cymbal choking.
- Native drum pads and computer-key performance; hardware MIDI input, editable note mapping, input-channel filtering, CC4 hi-hat position and MIDI overdub.
- Drum editor with draw/select/erase, Shift selection, marquee, grouped dragging, velocity/probability edits, straight/triplet snap, quantize, ghost notes, flams, rolls and clipboard.
- Multiple patterns and a song arrangement of repeatable, reorderable pattern clips.
- An 11-channel stereo mixer with live smoothed level/pan/mute/solo, polarity, room sends, meters, high-pass, low/mid/high EQ, compression, tuning and decay.
- Shared stereo room, parallel compression, master gain and a sample-peak output limiter; six starting mix presets.
- Portable `.ironroom-s` projects, undo/redo and opt-in recovery of factory-only autosaves.
- User WAV/MP3/FLAC/AIFF imports, velocity layers/round robins, encoded recordings embedded in explicit project saves.
- Background **24-bit stereo WAV** and **480-PPQ MIDI** export; the WAV renderer uses the same Soniclaw processor as live playback.
- Headless content inspection and rendering, platform build scripts and macOS/Windows/Linux CI configuration.

This release is a **standalone application**. It does not yet provide VST3/AU/CLAP builds or import the previous JUCE `.ironroom` or browser session formats. See [known limits](docs/ARCHITECTURE.md#current-limits) and [validation](docs/VALIDATION.md).

## Build and run

Install Rust **1.100 or later** and the native compiler tools for your platform. Soniclaw source is included under `vendor/soniclaw`; first builds still need internet access to download locked Cargo dependencies. Build from this directory:

```sh
cargo build --release --locked
cargo run --release --locked
```

**macOS:** install Xcode Command Line Tools (`xcode-select --install`), then run `python3 scripts/package.py`. This produces `dist/Iron Room.app`, including the factory assets. The app is unsigned; Developer ID signing and notarization are not included.

**Windows:** install the Rust MSVC toolchain and Visual Studio's **Desktop development with C++** workload. Run `python scripts/package.py`; keep `assets` beside `iron-room.exe` inside the generated portable folder. ASIO integration is not included; native output uses Soniclaw/CPAL's configured platform backend.

**Linux:** install `build-essential pkg-config libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev`. A working X11/XWayland desktop and OpenGL driver are required for this app's native UI. Run `python3 scripts/package.py` to create a portable folder. System GUI/audio libraries are not bundled.

## Start making a groove

1. Open the app and click **Play**, or audition the pads in **Kit**.
2. Search the left browser and double-click a groove to load it into the selected pattern. Use **+ Pattern** first to preserve another pattern. Undo restores replaced grooves.
3. In **Drum editor**, click an empty cell to draw a hit, click a hit to select it, drag selected hits to move them, or right-click to erase. Shift-click toggles selection; Select mode supports marquee. The straight/triplet snap affects new edits, not imported groove timing.
4. Choose **+ Song clip** to add the pattern to the song. In **Song**, set repeats and reorder clips. Switch the transport to **Song** to play it.
5. Faders, pan, mute/solo, sends, master, room return and parallel blend update live. Adjust EQ/compression/tuning/decay in **Channel FX**, then choose **Apply sound edits**. Preparation temporarily stops audio and restarts playback from the beginning when it was playing.
6. Save an `.ironroom-s` project. Export WAV or MIDI using the header buttons. Exports refuse to overwrite existing files; choose a new filename.

### Performance and editing keys

`B` kick · `F` snare · `D` cross-stick · `G` wires off · `H/J/K/L` closed/intermediate/open/pedal hat · `R/T` toms · `Q/W` crashes · `E/Y` ride/bell · `U/I/O` china/splash/cowbell. Hold Shift for softer hits. `X` chokes/reset voices and room; Space plays/stops. Shortcuts are suspended while a text control owns keyboard input.

Cmd/Ctrl S saves; Cmd/Ctrl Shift S saves as; Cmd/Ctrl Z undoes; Cmd/Ctrl Shift Z redoes. In Drum editor: Cmd/Ctrl A selects all, C copies, V pastes relative to beat two, and Delete removes selected hits. Clipboard paste retains relative timing. The current release does not expose a separate velocity-lane editor or cut/nudge shortcuts.

### Your own recordings

Select an articulation, then **Import recordings…**. Naming files `snare_v40_rr1.wav`, `snare_v40_rr2.wav`, `snare_v90_rr1.wav`, `snare_v127_rr1.wav` sets upper velocity boundaries and round-robin alternatives. No `_v` token means velocity 127. The highest boundary also covers velocity 127. Import replaces the selected articulation's custom set; **Restore factory** returns to the included sounds. Apply after restoring. Factory kit selections clear all custom sets; Undo or an explicit project save retains them.

Imports support mono/stereo, 8–192 kHz, at most 30 seconds/file, 32 MiB/file, 64 recordings/articulation and 64 MiB per import. Project limits apply across all custom sets. Explicit saves contain the encoded bytes, so the source files can be moved afterward. Custom-sample sessions require explicit saving; periodic autosave currently covers factory-only projects to avoid repeatedly writing large packages.

### Audio and MIDI

Open **Audio / MIDI** to choose output/input and reconnect. The device's default sample rate and hardware buffer configuration are used; internal processing is split into blocks no larger than 512 frames. Stereo output is required. These 512 frames are a software processing bound, not a measured round-trip latency or guaranteed hardware buffer size.

Input channel 0 accepts all channels. Note mapping follows the original Iron Room defaults. CC4 selects hi-hat openness; its direction is configurable. Polyphonic pressure releases the mapped cymbal. Start playback and enable **REC** to overdub triggers into the current pattern or the pattern referred to by a song clip. Repeated clips share their source pattern. Hardware timestamps use Soniclaw's callback clock; physical e-drum calibration remains target-device work.

## Headless rendering

```sh
cargo run --release --locked -- --inspect
cargo run --release --locked -- --list-grooves
cargo run --release --locked -- --groove 0 --render demo.wav --midi demo.mid
cargo run --release --locked -- --project session.ironroom-s --rate 48000 --render mix.wav
```

Use `--assets PATH` or `IRON_ROOM_ASSETS` to select an asset folder. Normal packages resolve assets beside the executable or inside the macOS app. `--no-audio` opens the editor without requesting a device. A headless-only build is available with `cargo build --release --locked --no-default-features`.

WAV includes one complete pattern/song pass plus up to 22 seconds of effect tails, based on room decay. It evaluates hit probability using a deterministic seed. MIDI exports all programmed hits, tempo and signature changes on GM channel 10; it carries neither samples nor mixer processing. Loop repeats continue indefinitely only during live playback, never during export.

## Architecture and development

Read [architecture and limits](docs/ARCHITECTURE.md), [validation](docs/VALIDATION.md), [release notes](docs/releases/v0.1.0.md) and [third-party attribution](docs/THIRD_PARTY.md).

```sh
cargo fmt --check
cargo test --locked -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
```

## Licence

Application code: MIT. Soniclaw: MIT OR Apache-2.0; its licences remain in the vendored source. The Salamander recordings are by **Alexander Holm / Rytmenpinne**, distributed under **CC BY-SA 3.0 Unported**; this app reuses the Iron Room MP3s unchanged. Preserve [assets/SAMPLE-LICENSE](assets/SAMPLE-LICENSE) and [assets/SAMPLE-README.md](assets/SAMPLE-README.md) when redistributing. These are overhead recordings, not newly recorded separate close/room microphones. No commercial GGD or Kontakt recordings are included.
