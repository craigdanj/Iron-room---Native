# Executed validation — 0.1.0

Executed on Linux x86_64 on 2026-11-21 with Rust 1.100.0. This report separates application checks from physical-device and target-platform acceptance.

## Passed

- `cargo check` for the complete native desktop application.
- `cargo build --release --locked` for the native Linux executable.
- `cargo fmt --check` and `cargo clippy --all-targets --locked -- -D warnings`.
- `cargo test --no-default-features --locked -- --test-threads=1`: **10 integration tests passed**, plus library/binary/doc-test harnesses. The final full run completed in 92.55 seconds. The strengthened hi-hat choke assertion was then rerun separately and passed.
- All **201 original recordings decoded**, with **302,819,328 decoded PCM bytes**; all 2,375 grooves and five kit configurations passed model validation.
- Fractional groove positions, probability and swing produced exactly identical samples across unequal callback partitions, including loop crossings.
- Prepared processing and schedule retirement: **zero measured allocations and zero net deallocations** during the exercised callbacks.
- Mute/solo before room and parallel sends, exact input-MIDI onset offsets, open-hat short release, portable custom samples, project round-trip and failed-save preservation.
- Song expansion and MIDI round-trip through Soniclaw's parser retained odd-meter structure and a fractional hit at tick 320.
- 24-bit WAV frame count, finite bounded audio, refusal to overwrite an existing output, and cancellation cleanup.
- Native eframe/Glow UI under Xvfb/software OpenGL: 120 frames, real XTest add-note input and Cmd/Ctrl Z / Shift Z, resulting in 27 hits from the original 26. Project validation passed and the app closed normally.
- Native Soniclaw/CPAL callback under ALSA **virtual null output**: connected, peak **0.9379318953**, zero stream errors and zero processing errors. This used no physical audio device.
- Actual factory-content demo: **338,940 stereo frames at 44.1 kHz / 24-bit**, plus a **26-hit MIDI** export. Files are in `demo/`.

Exact logs and machine-readable receipts are in [validation/](validation/). The demo WAV uses one Rock Pocket pass plus room tails. WAV and MIDI were produced by the packaged application code, not by synthesized replacement samples or an external renderer.

## Not established

macOS/Windows compilation or runtime, external DAW operation, physical audio/MIDI input, perceived latency, unplug/reconnect stress, live listening on speakers, signing/notarization, installers and large-session CPU admission. The platform workflow is provided but was not run in the user's GitHub account. The sample-peak limiter and mathematical tests are not mastering certification or proof of perceptual equivalence to the JUCE edition.
