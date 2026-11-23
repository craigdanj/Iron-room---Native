use crate::{
    content::Content,
    engine::{DrumProcessor, BLOCK},
    model::{meter, Project, Schedule},
    Result,
};
use soniclaw_core::*;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
};

/// Independent offline instance; same voices/effects as live audio. No full-file PCM allocation.
pub fn wav(
    content: Arc<Content>,
    project: &Project,
    path: &Path,
    rate: u32,
    cancel: &AtomicBool,
    progress: &AtomicU32,
) -> Result<u64> {
    if !(8000..=192000).contains(&rate) {
        return Err("Export rate must be 8–192 kHz".into());
    }
    let mut p = project.clone();
    p.looping = false;
    let schedule = Schedule::build(&p, &content)?;
    let body = (schedule.length * 15. / p.bpm * f64::from(rate)).round() as u64;
    let tail = ((p.room_decay as f64 * 2. + 1.).min(22.) * f64::from(rate)) as u64;
    let frames = body + tail;
    // Classic WAV has a 4 GiB RIFF limit. Reject before creating an output.
    if frames * 6 > u32::MAX as u64 - 128 {
        return Err("Mix exceeds classic WAV size; shorten the song or lower export rate".into());
    }
    let (mut processor, control) = DrumProcessor::new(content, &p)?;
    processor.prepare(ProcessSpec {
        sample_rate: rate as f64,
        max_block_frames: BLOCK,
        channels: 2,
    })?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let result = (|| {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(std::io::BufWriter::new(file), spec)?;
        control.play();
        let mut position = 0;
        let mut pcm = [0f32; BLOCK * 2];
        while position < frames {
            if cancel.load(Ordering::Acquire) {
                return Err("Export cancelled".into());
            }
            let n = (frames - position).min(BLOCK as u64) as usize;
            let context = ProcessContext {
                events: &[],
                transport: Transport {
                    sample_position: position,
                    tempo: p.bpm,
                    playing: true,
                },
            };
            processor.process(&mut AudioBuffer::new(&mut pcm[..n * 2], 2)?, &context);
            for s in &pcm[..n * 2] {
                if !s.is_finite() {
                    return Err("Nonfinite audio during export".into());
                }
                writer.write_sample((s.clamp(-1., 1.) * 8388607.).round() as i32)?;
            }
            position += n as u64;
            progress.store((position * 1000 / frames) as u32, Ordering::Relaxed);
        }
        writer.finalize()?;
        Ok(frames)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Type-0 SMF, GM channel 10, tempo and song signature changes, 480 PPQ.
pub fn midi(content: &Content, project: &Project, path: &Path) -> Result<usize> {
    let schedule = Schedule::build(project, content)?;
    let mut events: Vec<(u64, u8, Vec<u8>)> = Vec::new();
    let tempo = (60_000_000. / project.bpm).round() as u32;
    events.push((
        0,
        0,
        vec![
            0xff,
            0x51,
            3,
            (tempo >> 16) as u8,
            (tempo >> 8) as u8,
            tempo as u8,
        ],
    ));
    let mut at = 0.;
    let patterns: Vec<_> = if project.song_mode && !project.song.is_empty() {
        project
            .song
            .iter()
            .flat_map(|c| std::iter::repeat_n(&project.patterns[c.pattern], c.repeats as usize))
            .collect()
    } else {
        vec![project.pattern()]
    };
    for pattern in patterns {
        let (n, d) = meter(&pattern.signature).ok_or("Invalid meter")?;
        events.push((
            (at * 120f64).round() as u64,
            0,
            vec![0xff, 0x58, 4, n as u8, d.trailing_zeros() as u8, 24, 8],
        ));
        at += pattern.length();
    }
    for hit in &schedule.hits {
        let tick = (hit.step * 120.).round() as u64;
        let note = project.midi[hit.lane];
        events.push((tick, 2, vec![0x99, note, hit.velocity]));
        events.push((tick + 30, 1, vec![0x89, note, 0]));
    }
    events.sort_by_key(|(tick, order, _)| (*tick, *order));
    let mut track = Vec::new();
    let mut previous = 0;
    for (tick, _, message) in events {
        vlq(tick - previous, &mut track)?;
        track.extend(message);
        previous = tick;
    }
    let end = (schedule.length * 120.).round() as u64;
    vlq(end.saturating_sub(previous), &mut track)?;
    track.extend([0xff, 0x2f, 0]);
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    bytes.extend((track.len() as u32).to_be_bytes());
    bytes.extend(track);
    // Exercise Soniclaw's parser before publishing any MIDI file.
    soniclaw_midi::file::MidiClip::read(&bytes)?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes)?;
    Ok(schedule.hits.len())
}
fn vlq(mut n: u64, out: &mut Vec<u8>) -> Result<()> {
    if n > 0x0fff_ffff {
        return Err("MIDI delta exceeds SMF limit".into());
    }
    let mut b = [0u8; 4];
    let mut i = 3;
    b[i] = (n & 127) as u8;
    n >>= 7;
    while n > 0 {
        i -= 1;
        b[i] = ((n & 127) as u8) | 128;
        n >>= 7;
    }
    out.extend_from_slice(&b[i..]);
    Ok(())
}
