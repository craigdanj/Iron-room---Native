use iron_room_soniclaw::{
    content::{Content, Library},
    engine::DrumProcessor,
    export,
    model::*,
};
use soniclaw_core::*;
use soniclaw_dsp::Sample;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, OnceLock,
    },
};

fn factory() -> Arc<Content> {
    static C: OnceLock<Arc<Content>> = OnceLock::new();
    C.get_or_init(|| {
        Content::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")).unwrap()
    })
    .clone()
}
fn fixture() -> Arc<Content> {
    let library: Library = serde_json::from_str(include_str!("../assets/library.json")).unwrap();
    let mut groups = BTreeMap::new();
    for lane in &library.lanes {
        for variant in &lane.variants {
            for (_, name) in &variant.groups {
                groups.entry(name.clone()).or_insert_with(|| {
                    (0..3)
                        .map(|rr| {
                            let data: Vec<_> = (0..4096)
                                .flat_map(|i| {
                                    let x = if i < 8 {
                                        0.
                                    } else {
                                        ((i as f32 * (0.08 + rr as f32 * 0.005)).sin()
                                            * (-i as f32 / 800.).exp())
                                            * 0.2
                                    };
                                    [x, x * 0.8]
                                })
                                .collect();
                            Sample::new(data, 2, 44100., 60).unwrap()
                        })
                        .collect::<Vec<_>>()
                });
            }
        }
    }
    Arc::new(Content {
        root: PathBuf::new(),
        library,
        groups,
        sample_count: 201,
        pcm_bytes: 0,
    })
}
fn context() -> ProcessContext<'static> {
    ProcessContext {
        events: &[],
        transport: Transport::default(),
    }
}
fn render(c: Arc<Content>, p: &Project, frames: usize, parts: &[usize]) -> Vec<f32> {
    let (mut processor, control) = DrumProcessor::new(c, p).unwrap();
    processor
        .prepare(ProcessSpec {
            sample_rate: 44100.,
            max_block_frames: 1024,
            channels: 2,
        })
        .unwrap();
    control.play();
    let mut out = vec![0.; frames * 2];
    let (mut offset, mut part) = (0, 0);
    while offset < frames {
        let n = parts[part % parts.len()].min(frames - offset);
        processor.process(
            &mut AudioBuffer::new(&mut out[offset * 2..(offset + n) * 2], 2).unwrap(),
            &context(),
        );
        offset += n;
        part += 1;
    }
    out
}
fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("iron-room-test-{}-{name}", std::process::id()))
}

#[test]
fn factory_content_decodes_all_original_recordings() {
    let c = factory();
    assert_eq!(c.sample_count, 201);
    assert_eq!(c.library.grooves.len(), 2375);
    assert_eq!(c.library.lanes.len(), 17);
    assert_eq!(c.library.channels.len(), 11);
    assert!(c.pcm_bytes > 10_000_000);
    for groove in &c.library.grooves {
        let mut p = Project::factory(&c);
        p.patterns[0] = Pattern::from_groove(groove);
        p.validate(&c).unwrap();
    }
    for k in 0..5 {
        let mut p = Project::factory(&c);
        p.kit(k);
        p.validate(&c).unwrap();
    }
}
#[test]
fn fractional_grooves_are_partition_invariant_through_loop_boundaries() {
    let c = fixture();
    let mut p = Project::factory(&c);
    p.bpm = 300.;
    p.patterns[0].bars = 1;
    p.patterns[0].signature = "3/4".into();
    p.patterns[0].events = vec![
        Hit {
            step: 0.,
            ..Default::default()
        },
        Hit {
            lane: "snare".into(),
            step: 8. / 3.,
            prob: 0.6,
            ..Default::default()
        },
        Hit {
            lane: "closed".into(),
            step: 11.999,
            ..Default::default()
        },
    ];
    p.swing = 0.27;
    let a = render(c.clone(), &p, 60000, &[512]);
    let b = render(c, &p, 60000, &[1, 7, 65, 513, 255, 1024]);
    assert_eq!(a, b);
    assert!(a.iter().any(|x| x.abs() > 0.01));
}
#[test]
fn prepared_audio_and_schedule_retirement_allocate_nothing() {
    let c = fixture();
    let p = Project::factory(&c);
    let (mut processor, mut control) = DrumProcessor::new(c.clone(), &p).unwrap();
    processor
        .prepare(ProcessSpec {
            sample_rate: 44100.,
            max_block_frames: 512,
            channels: 2,
        })
        .unwrap();
    let mut pcm = [0.; 1024];
    control.play();
    processor.process(&mut AudioBuffer::new(&mut pcm, 2).unwrap(), &context());
    control.publish(Schedule::build(&p, &c).unwrap());
    control.trigger(1, 127);
    let result = allocation_counter::measure(|| {
        for _ in 0..16 {
            processor.process(&mut AudioBuffer::new(&mut pcm, 2).unwrap(), &context());
        }
    });
    assert_eq!(result.count_total, 0);
    assert_eq!(result.count_current, 0);
    control.collect();
}
#[test]
fn mute_and_solo_are_applied_before_room_and_parallel() {
    let c = fixture();
    let mut p = Project::factory(&c);
    for strip in &mut p.channels {
        strip.mute = true;
    }
    let out = render(c.clone(), &p, 12000, &[127]);
    assert!(out.iter().all(|x| *x == 0.));
    p.channels[1].mute = false;
    p.channels[0].solo = true;
    let out = render(c, &p, 12000, &[512]);
    assert!(out.iter().all(|x| *x == 0.));
}
#[test]
fn host_midi_offsets_and_hat_choking_reach_soniclaw_voices() {
    let c = fixture();
    let mut p = Project::factory(&c);
    p.patterns[0].events.clear();
    p.parallel = 0.;
    p.room = 0.;
    let (mut processor, control) = DrumProcessor::new(c, &p).unwrap();
    processor
        .prepare(ProcessSpec {
            sample_rate: 44100.,
            max_block_frames: 512,
            channels: 2,
        })
        .unwrap();
    let events = [
        Event {
            frame: 100,
            message: MidiMessage::NoteOn {
                channel: 9,
                note: 26,
                velocity: 127,
            },
        },
        Event {
            frame: 350,
            message: MidiMessage::NoteOn {
                channel: 9,
                note: 42,
                velocity: 127,
            },
        },
    ];
    let mut pcm = [0.; 1024];
    processor.process(
        &mut AudioBuffer::new(&mut pcm, 2).unwrap(),
        &ProcessContext {
            events: &events,
            transport: Transport::default(),
        },
    );
    assert!(pcm[..200].iter().all(|x| *x == 0.));
    assert!(pcm[220..700].iter().any(|x| x.abs() > 0.01));
    assert!(control.shared.dropped.load(Ordering::Relaxed) == 0);
    assert_eq!(processor.sample_voices(), 2);
    processor.process(&mut AudioBuffer::new(&mut pcm, 2).unwrap(), &context());
    assert_eq!(
        processor.sample_voices(),
        1,
        "The open hat must finish its short choke release"
    );
}
#[test]
fn project_portability_and_validation_preserve_previous_file() {
    let c = fixture();
    let mut p = Project::factory(&c);
    let path = temp("project.ironroom-s");
    p.save(&path, &c).unwrap();
    let original = std::fs::read(&path).unwrap();
    let restored = Project::open(&path, &c).unwrap();
    assert_eq!(restored.pattern().events.len(), p.pattern().events.len());
    p.patterns[0].events[0].step = -1.;
    assert!(p.save(&path, &c).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let _ = std::fs::remove_file(path);
}
#[test]
fn song_arrangement_and_midi_keep_odd_meter_and_fractional_timing() {
    let c = fixture();
    let mut p = Project::factory(&c);
    p.patterns[0].signature = "7/8".into();
    p.patterns[0].bars = 1;
    p.patterns[0].events = vec![Hit {
        step: 8. / 3.,
        ..Default::default()
    }];
    p.patterns.push(Pattern {
        name: "Three".into(),
        bars: 1,
        signature: "3/4".into(),
        events: vec![Hit {
            lane: "snare".into(),
            step: 4.,
            ..Default::default()
        }],
    });
    p.song = vec![
        Clip {
            pattern: 0,
            repeats: 2,
        },
        Clip {
            pattern: 1,
            repeats: 1,
        },
    ];
    p.song_mode = true;
    let s = Schedule::build(&p, &c).unwrap();
    assert_eq!(s.length, 40.);
    assert_eq!(s.hits.len(), 3);
    let path = temp("groove.mid");
    let _ = std::fs::remove_file(&path);
    export::midi(&c, &p, &path).unwrap();
    let clip = soniclaw_midi::file::MidiClip::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(clip.events.iter().any(|e| e.tick == 320));
    assert_eq!(clip.events.len(), 6);
    let _ = std::fs::remove_file(path);
}
#[test]
fn wav_export_is_finite_has_expected_duration_and_refuses_overwrite() {
    let c = fixture();
    let mut p = Project::factory(&c);
    p.patterns[0].bars = 1;
    p.patterns[0].signature = "3/4".into();
    p.patterns[0].events = vec![Hit::default()];
    p.bpm = 300.;
    p.room_decay = 0.1;
    let path = temp("mix.wav");
    let _ = std::fs::remove_file(&path);
    let frames = export::wav(
        c.clone(),
        &p,
        &path,
        44100,
        &AtomicBool::new(false),
        &AtomicU32::new(0),
    )
    .unwrap();
    assert_eq!(frames, 26460 + 52920);
    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.duration() as u64, frames);
    let pcm: Vec<_> = reader.samples::<i32>().map(|s| s.unwrap()).collect();
    assert!(pcm.iter().any(|s| s.abs() > 1000));
    assert!(pcm.iter().all(|s| s.abs() < 8_388_607));
    assert!(export::wav(
        c,
        &p,
        &path,
        44100,
        &AtomicBool::new(false),
        &AtomicU32::new(0)
    )
    .is_err());
    let _ = std::fs::remove_file(path);
}
#[test]
fn cancelled_export_removes_only_its_new_output() {
    let c = fixture();
    let p = Project::factory(&c);
    let path = temp("cancel.wav");
    let _ = std::fs::remove_file(&path);
    assert!(export::wav(
        c,
        &p,
        &path,
        44100,
        &AtomicBool::new(true),
        &AtomicU32::new(0)
    )
    .is_err());
    assert!(!path.exists());
}
#[test]
fn portable_custom_velocity_round_robins_use_embedded_audio() {
    use base64::Engine as _;
    let c = fixture();
    let mut p = Project::factory(&c);
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 44100,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..2048 {
            w.write_sample(if i < 200 { 2000i16 } else { 0 }).unwrap();
        }
        w.finalize().unwrap();
    }
    p.custom[0] = vec![CustomSample {
        velocity: 64,
        name: "kick_v64_rr1.wav".into(),
        data: base64::prelude::BASE64_STANDARD.encode(bytes.into_inner()),
    }];
    p.patterns[0].events = vec![Hit {
        velocity: 127,
        ..Default::default()
    }];
    p.validate(&c).unwrap();
    let a = render(c, &p, 3000, &[512]);
    assert!(a.iter().any(|x| x.abs() > 0.001));
}
