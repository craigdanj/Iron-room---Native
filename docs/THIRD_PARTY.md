# Credits and dependency licences

The application is an independent Rust/Soniclaw port of Iron Room. No JUCE source or JUCE linkage is included.

- **Soniclaw 0.144.0:** MIT OR Apache-2.0; source and licence files retained under `vendor/soniclaw`.
- **Salamander Drumkit:** Alexander Holm / Rytmenpinne, CC BY-SA 3.0 Unported. Included recordings come unchanged from the existing Iron Room distribution, which trimmed/encoded the upstream recordings to MP3. Retain `assets/SAMPLE-LICENSE` and `assets/SAMPLE-README.md` with derived sample distributions. Upstream: https://github.com/studiorack/salamander-drumkit.
- **Original programmed grooves:** from Iron Room's existing MIT source; not commercial MIDI packs.
- **egui / eframe:** MIT OR Apache-2.0.
- **CPAL / midir:** Apache-2.0 / MIT respectively, through Soniclaw.
- **Symphonia:** MPL-2.0. This application uses the unmodified dependency to decode MP3/WAV/FLAC/AIFF; source is obtainable from the exact versions in Cargo.lock, https://github.com/pdeljanov/Symphonia.
- **rfd:** MIT.
- **hound:** Apache-2.0.
- **rtrb / base64 / serde / serde_json:** their upstream MIT/Apache licence choices, recorded by Cargo metadata.

Cargo.lock pins the transitive Rust dependencies. The platform compiler, system audio/window libraries and SDKs have their own terms. The MIT application licence does not replace any dependency or sample licence. This application is not affiliated with GetGood Drums or Native Instruments.
