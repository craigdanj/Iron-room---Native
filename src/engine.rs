//! One audio owner. Soniclaw voices, resampling, EQ, compression, room and limiting.
//! Prepared schedules cross bounded SPSC queues; retired schedules are freed by UI.
use crate::{
    content::{decode, Content},
    model::*,
    Result,
};
use base64::Engine as _;
use soniclaw_core::*;
use soniclaw_dsp::{
    resample::ResampleQuality, Compressor, CompressorSettings, EqBand, EqShape, ParametricEq,
    PeakLimiter, Reverb, Sample,
};
use soniclaw_sampler::{MultiSampler, SampleBank, Zone};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc,
};

pub const BLOCK: usize = 512;
#[derive(Clone, Copy)]
pub struct Trigger {
    pub lane: usize,
    pub velocity: u8,
}
#[derive(Clone, Copy)]
pub struct RecordedHit {
    pub step: f64,
    pub lane: usize,
    pub velocity: u8,
}
pub struct Shared {
    pub running: AtomicBool,
    pub restart: AtomicBool,
    pub panic: AtomicBool,
    pub record: AtomicBool,
    pub position: AtomicU64,
    pub cycle: AtomicU64,
    pub peaks: [AtomicU32; CHANNELS],
    pub peak: AtomicU32,
    pub dropped: AtomicU64,
    pub voices: AtomicU64,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            panic: AtomicBool::new(false),
            record: AtomicBool::new(false),
            position: AtomicU64::new(0),
            cycle: AtomicU64::new(0),
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            peak: AtomicU32::new(0),
            dropped: AtomicU64::new(0),
            voices: AtomicU64::new(0),
        }
    }
}
pub struct Control {
    pub shared: Arc<Shared>,
    schedule: rtrb::Producer<Box<Schedule>>,
    retired: rtrb::Consumer<Box<Schedule>>,
    triggers: rtrb::Producer<Trigger>,
    pub recorded: rtrb::Consumer<RecordedHit>,
}
impl Control {
    pub fn collect(&mut self) {
        while self.retired.pop().is_ok() {}
    }
    pub fn publish(&mut self, s: Box<Schedule>) -> bool {
        self.collect();
        self.schedule.push(s).is_ok()
    }
    pub fn trigger(&mut self, lane: usize, velocity: u8) -> bool {
        self.triggers.push(Trigger { lane, velocity }).is_ok()
    }
    pub fn play(&self) {
        self.shared.running.store(true, Ordering::Release);
    }
    pub fn stop(&self) {
        self.shared.running.store(false, Ordering::Release);
        self.shared.panic.store(true, Ordering::Release);
        self.shared.restart.store(true, Ordering::Release);
    }
}
struct LaneVoice {
    sampler: MultiSampler<16>,
    events: Vec<Event>,
    pcm: Vec<f32>,
    channel: usize,
}
struct ChannelFx {
    eq: ParametricEq,
    compressor: Compressor,
    gain: f32,
    pan: f32,
}
pub struct DrumProcessor {
    lanes: Vec<LaneVoice>,
    channels: Vec<ChannelFx>,
    schedule: Box<Schedule>,
    incoming: rtrb::Consumer<Box<Schedule>>,
    retired: rtrb::Producer<Box<Schedule>>,
    triggers: rtrb::Consumer<Trigger>,
    recorded: rtrb::Producer<RecordedHit>,
    pub shared: Arc<Shared>,
    scratch: Vec<Vec<f32>>,
    room: Reverb,
    parallel: Compressor,
    limiter: PeakLimiter,
    room_pcm: Vec<f32>,
    parallel_pcm: Vec<f32>,
    spec: Option<ProcessSpec>,
    position: u64,
    cycle: u64,
    master_gain: f32,
    room_return: f32,
    parallel_return: f32,
    pedal: Option<u8>,
    prepared_project: Project,
}
impl DrumProcessor {
    pub fn new(content: Arc<Content>, project: &Project) -> Result<(Self, Control)> {
        let schedule = Schedule::build(project, &content)?;
        let (sp, sc) = rtrb::RingBuffer::new(4);
        let (rp, rc) = rtrb::RingBuffer::new(8);
        let (tp, tc) = rtrb::RingBuffer::new(256);
        let (ip, ic) = rtrb::RingBuffer::new(2048);
        let shared = Arc::new(Shared::default());
        let mut lanes = Vec::new();
        for (i, lane) in content.library.lanes.iter().enumerate() {
            let ch = content.channel_index(i);
            let strip = project.channels[ch];
            let mut samples = Vec::new();
            let mut zones = Vec::new();
            if !project.custom[i].is_empty() {
                let mut layers = project.custom[i].clone();
                layers.sort_by_key(|s| s.velocity);
                for item in &layers {
                    let bytes = base64::prelude::BASE64_STANDARD.decode(&item.data)?;
                    let sample = decode(&bytes)?;
                    let low = layers
                        .iter()
                        .filter(|s| s.velocity < item.velocity)
                        .map(|s| s.velocity)
                        .max()
                        .map_or(1, |v| v + 1);
                    let high = if item.velocity == layers.last().unwrap().velocity {
                        127
                    } else {
                        item.velocity
                    };
                    let n = samples.len();
                    samples.push(prepare_sample(&sample, 60, strip)?);
                    let mut z = Zone::single(n, 60);
                    z.low_velocity = low;
                    z.high_velocity = high;
                    z.round_robin_group = 0;
                    zones.push(z);
                }
            } else {
                for (variant, v) in lane.variants.iter().enumerate() {
                    let mut low = 1;
                    for (high, group) in &v.groups {
                        for source in &content.groups[group] {
                            let n = samples.len();
                            samples.push(prepare_sample(source, 60 + variant as u8, strip)?);
                            let mut z = Zone::single(n, 60 + variant as u8);
                            z.low_velocity = low;
                            z.high_velocity = *high;
                            z.round_robin_group = variant as u16;
                            zones.push(z);
                        }
                        low = high.saturating_add(1);
                    }
                }
            }
            // Hats/cymbals are gated internally so short release events can choke them.
            if (4..=7).contains(&i) || i >= 10 {
                for z in &mut zones {
                    z.one_shot = false;
                }
            }
            let bank = Arc::new(SampleBank::new(samples, zones, 256 * 1024 * 1024)?);
            let mut sampler = MultiSampler::new(bank);
            sampler.set_quality(ResampleQuality::SincFast);
            sampler.set_envelope(0., 5.)?;
            lanes.push(LaneVoice {
                sampler,
                events: Vec::with_capacity(1024),
                pcm: Vec::new(),
                channel: ch,
            });
        }
        let mut channels = Vec::new();
        for s in &project.channels {
            let eq = ParametricEq::new(vec![
                EqBand {
                    shape: EqShape::HighPass,
                    frequency: s.hpf,
                    q: 0.707,
                    gain_db: 0.,
                },
                EqBand {
                    shape: EqShape::LowShelf,
                    frequency: 100.,
                    q: 0.707,
                    gain_db: s.low,
                },
                EqBand {
                    shape: EqShape::Peak,
                    frequency: 1500.,
                    q: 0.8,
                    gain_db: s.mid,
                },
                EqBand {
                    shape: EqShape::HighShelf,
                    frequency: 8000.,
                    q: 0.707,
                    gain_db: s.high,
                },
            ])?;
            let compressor = Compressor::new(CompressorSettings {
                threshold_db: s.threshold,
                ratio: s.ratio,
                knee_db: 6.,
                attack_ms: s.attack,
                release_ms: s.release,
                makeup_db: s.makeup,
            })?;
            let audible = !s.mute && (!project.channels.iter().any(|c| c.solo) || s.solo);
            channels.push(ChannelFx {
                eq,
                compressor,
                gain: if audible { s.volume } else { 0. },
                pan: s.pan,
            });
        }
        let control = Control {
            shared: shared.clone(),
            schedule: sp,
            retired: rc,
            triggers: tp,
            recorded: ic,
        };
        let processor = Self {
            lanes,
            channels,
            schedule,
            incoming: sc,
            retired: rp,
            triggers: tc,
            recorded: ip,
            shared,
            scratch: vec![vec![]; CHANNELS],
            room: Reverb::new(project.room_decay, 1.)?,
            parallel: Compressor::new(CompressorSettings {
                threshold_db: -28.,
                ratio: 8.,
                attack_ms: 8.,
                release_ms: 120.,
                makeup_db: 6.,
                knee_db: 6.,
            })?,
            limiter: PeakLimiter::new(-0.5, 80.)?,
            room_pcm: vec![],
            parallel_pcm: vec![],
            spec: None,
            position: 0,
            cycle: 0,
            master_gain: project.master,
            room_return: project.room,
            parallel_return: project.parallel,
            pedal: None,
            prepared_project: project.clone(),
        };
        Ok((processor, control))
    }
    pub fn sample_voices(&self) -> usize {
        self.lanes.iter().map(|l| l.sampler.active_voices()).sum()
    }
    fn push(&mut self, lane: usize, event: Event) {
        if self.lanes[lane].events.len() < 1024 {
            self.lanes[lane].events.push(event);
        } else {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn fire(
        &mut self,
        lane: usize,
        velocity: u8,
        frame: usize,
        variant: Option<usize>,
        record: bool,
    ) {
        if lane >= LANES || velocity == 0 || velocity > 127 {
            return;
        }
        if lane == 4 || lane == 7 {
            for i in 4..=7 {
                for key in 60..67 {
                    self.push(
                        i,
                        Event {
                            frame,
                            message: MidiMessage::NoteOff {
                                channel: 9,
                                note: key,
                                velocity: 0,
                            },
                        },
                    );
                }
            }
        }
        let v = if self.prepared_project.custom[lane].is_empty() {
            variant.unwrap_or(self.schedule.variants[lane])
        } else {
            0
        };
        self.push(
            lane,
            Event {
                frame,
                message: MidiMessage::NoteOn {
                    channel: 9,
                    note: 60 + v as u8,
                    velocity,
                },
            },
        );
        if record
            && self.shared.record.load(Ordering::Relaxed)
            && self.shared.running.load(Ordering::Relaxed)
        {
            let rate = self.spec.unwrap().sample_rate;
            let step = (self.position + frame as u64) as f64 * self.schedule.bpm / (rate * 15.);
            if self
                .recorded
                .push(RecordedHit {
                    step: step % self.schedule.length,
                    lane,
                    velocity,
                })
                .is_err()
            {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    fn midi(&mut self, event: Event) {
        let (channel, note, velocity) = match event.message {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => (channel, note, velocity),
            MidiMessage::NoteOnId {
                channel,
                note,
                velocity,
                ..
            } => (channel, note, (velocity.clamp(0., 1.) * 127.).round() as u8),
            MidiMessage::ControlChange {
                channel,
                controller: 4,
                value,
            } => {
                if self.schedule.midi_channel == 0 || channel + 1 == self.schedule.midi_channel {
                    self.pedal = Some(if self.schedule.pedal_closed_high {
                        127 - value
                    } else {
                        value
                    });
                }
                return;
            }
            MidiMessage::PolyPressure { note, value, .. } if value > 0 => {
                if let Some(l) = self.schedule.midi.iter().position(|n| *n == note) {
                    for key in 60..67 {
                        self.push(
                            l,
                            Event {
                                frame: event.frame,
                                message: MidiMessage::NoteOff {
                                    channel: 9,
                                    note: key,
                                    velocity: 0,
                                },
                            },
                        );
                    }
                }
                return;
            }
            MidiMessage::AllNotesOff => {
                for l in &mut self.lanes {
                    l.sampler.reset();
                }
                return;
            }
            _ => return,
        };
        if self.schedule.midi_channel != 0 && channel + 1 != self.schedule.midi_channel {
            return;
        }
        if let Some(mut lane) = self.schedule.midi.iter().position(|n| *n == note) {
            let mut variant = None;
            if (4..=6).contains(&lane) {
                if let Some(open) = self.pedal {
                    if open < 16 {
                        lane = 4;
                    } else if open > 112 {
                        lane = 6;
                    } else {
                        lane = 5;
                        variant = Some(((open - 16) as usize * 7 / 97).min(6));
                    }
                }
            }
            self.fire(lane, velocity, event.frame, variant, true);
        }
    }
    fn scheduled(&mut self, frames: usize) {
        let rate = self.spec.unwrap().sample_rate;
        let loop_frames = (self.schedule.length * rate * 15. / self.schedule.bpm)
            .round()
            .max(1.) as u64;
        let mut cursor = 0usize;
        while cursor < frames {
            if self.position >= loop_frames {
                if self.schedule.looping {
                    self.position = 0;
                    self.cycle += 1;
                } else {
                    self.shared.running.store(false, Ordering::Release);
                    break;
                }
            }
            let count = (frames - cursor).min((loop_frames - self.position) as usize);
            let start_step = self.position as f64 * self.schedule.bpm / (rate * 15.);
            let tolerance = 0.51 * self.schedule.bpm / (rate * 15.);
            let begin = self
                .schedule
                .hits
                .partition_point(|h| h.step < start_step - tolerance);
            let mut i = begin;
            while i < self.schedule.hits.len() {
                let h = self.schedule.hits[i];
                let at = (h.step * rate * 15. / self.schedule.bpm).round() as u64;
                if at >= self.position + count as u64 {
                    break;
                }
                if at >= self.position {
                    let hash = (h.index as u64)
                        .wrapping_add(self.cycle.wrapping_mul(0x9e3779b97f4a7c15))
                        .wrapping_mul(0xbf58476d1ce4e5b9);
                    if h.probability >= 1.
                        || (hash >> 32) as f64 / (u32::MAX as f64) < h.probability as f64
                    {
                        self.fire(
                            h.lane,
                            h.velocity,
                            cursor + (at - self.position) as usize,
                            None,
                            false,
                        );
                    }
                }
                i += 1;
            }
            self.position += count as u64;
            cursor += count;
        }
    }
}
fn prepare_sample(source: &Sample, key: u8, strip: Strip) -> Result<Sample> {
    let mut pcm = source.data().to_vec();
    if strip.decay < 0.9999 {
        let frames = source.frames() as f32;
        let ch = source.channels();
        for (i, s) in pcm.iter_mut().enumerate() {
            *s *= (-8. * (1. / strip.decay - 1.) * (i / ch) as f32 / frames).exp();
        }
    }
    Ok(Sample::new(
        pcm,
        source.channels(),
        source.sample_rate() * 2f64.powf(strip.tune as f64 / 12.),
        key,
    )?)
}
impl AudioProcessor for DrumProcessor {
    fn prepare(&mut self, s: ProcessSpec) -> std::result::Result<(), Error> {
        s.validate()?;
        if s.channels != 2 || s.max_block_frames > 8192 {
            return Err(Error::InvalidSpec);
        }
        for l in &mut self.lanes {
            l.sampler.prepare(s)?;
            l.pcm = vec![0.; s.max_block_frames * 2];
        }
        for c in &mut self.channels {
            c.eq.prepare(s)?;
            c.compressor.prepare(s)?;
        }
        for pcm in &mut self.scratch {
            *pcm = vec![0.; s.max_block_frames * 2];
        }
        self.room.prepare(s)?;
        self.parallel.prepare(s)?;
        self.limiter.prepare(s)?;
        self.room_pcm = vec![0.; s.max_block_frames * 2];
        self.parallel_pcm = vec![0.; s.max_block_frames * 2];
        self.spec = Some(s);
        self.reset();
        Ok(())
    }
    fn reset(&mut self) {
        for l in &mut self.lanes {
            l.sampler.reset();
        }
        for c in &mut self.channels {
            c.eq.reset();
            c.compressor.reset();
        }
        self.room.reset();
        self.parallel.reset();
        self.limiter.reset();
        self.position = 0;
        self.cycle = 0;
        self.shared.position.store(0, Ordering::Relaxed);
    }
    fn process(&mut self, b: &mut AudioBuffer<'_>, ctx: &ProcessContext<'_>) {
        let Some(spec) = self.spec else {
            b.clear();
            return;
        };
        if b.channels() != 2
            || b.frames() > spec.max_block_frames
            || validate_events(ctx.events, b.frames()).is_err()
        {
            b.clear();
            return;
        }
        if !self.retired.is_full() {
            if let Ok(next) = self.incoming.pop() {
                let old = std::mem::replace(&mut self.schedule, next);
                let _ = self.retired.push(old);
            }
        }
        if self.shared.restart.swap(false, Ordering::AcqRel) {
            self.position = 0;
            self.cycle = 0;
        }
        if self.shared.panic.swap(false, Ordering::AcqRel) {
            for l in &mut self.lanes {
                l.sampler.reset();
            }
            self.room.reset();
            self.parallel.reset();
            self.limiter.reset();
        }
        for l in &mut self.lanes {
            l.events.clear();
        }
        for e in ctx.events {
            self.midi(*e);
        }
        for _ in 0..256 {
            match self.triggers.pop() {
                Ok(t) => self.fire(t.lane, t.velocity, 0, None, true),
                Err(_) => break,
            }
        }
        let running = self.shared.running.load(Ordering::Acquire);
        if running {
            self.scheduled(b.frames());
        }
        let samples = b.frames() * 2;
        for pcm in &mut self.scratch {
            pcm[..samples].fill(0.);
        }
        for l in &mut self.lanes {
            l.events.sort_unstable_by_key(|e| {
                (
                    e.frame,
                    if matches!(e.message, MidiMessage::NoteOff { .. }) {
                        0
                    } else {
                        1
                    },
                )
            });
            let context = ProcessContext {
                events: &l.events,
                transport: ctx.transport,
            };
            l.sampler.process(
                &mut AudioBuffer::new(&mut l.pcm[..samples], 2).unwrap(),
                &context,
            );
            for (out, x) in self.scratch[l.channel][..samples]
                .iter_mut()
                .zip(&l.pcm[..samples])
            {
                *out += *x;
            }
        }
        b.clear();
        self.room_pcm[..samples].fill(0.);
        let any_solo = self.schedule.channels.iter().any(|s| s.solo);
        for (i, (fx, pcm)) in self.channels.iter_mut().zip(&mut self.scratch).enumerate() {
            let s = self.schedule.channels[i];
            let mut buffer = AudioBuffer::new(&mut pcm[..samples], 2).unwrap();
            fx.eq.process(&mut buffer, ctx);
            fx.compressor.process(&mut buffer, ctx);
            let target = if s.mute || (any_solo && !s.solo) {
                0.
            } else {
                s.volume
            };
            let mut peak = 0f32;
            let a = 1. - (-1. / (spec.sample_rate as f32 * 0.005)).exp();
            for ((input, output), room) in pcm[..samples]
                .as_chunks::<2>()
                .0
                .iter()
                .zip(b.samples_mut().as_chunks_mut::<2>().0.iter_mut())
                .zip(self.room_pcm[..samples].as_chunks_mut::<2>().0.iter_mut())
            {
                fx.gain += (target - fx.gain) * a;
                fx.pan += (s.pan - fx.pan) * a;
                let angle = (fx.pan + 1.) * std::f32::consts::FRAC_PI_4;
                let g = fx.gain * if s.polarity { -1. } else { 1. };
                let left = input[0] * angle.cos() * std::f32::consts::SQRT_2 * g;
                let right = input[1] * angle.sin() * std::f32::consts::SQRT_2 * g;
                output[0] += left;
                output[1] += right;
                room[0] += left * s.room;
                room[1] += right * s.room;
                peak = peak.max(left.abs()).max(right.abs());
            }
            self.shared.peaks[i].store(peak.to_bits(), Ordering::Relaxed);
        }
        self.parallel_pcm[..samples].copy_from_slice(b.samples());
        self.parallel.process(
            &mut AudioBuffer::new(&mut self.parallel_pcm[..samples], 2).unwrap(),
            ctx,
        );
        self.room.process(
            &mut AudioBuffer::new(&mut self.room_pcm[..samples], 2).unwrap(),
            ctx,
        );
        let smoothing = 1. - (-1. / (spec.sample_rate as f32 * 0.005)).exp();
        for ((out, parallel), room) in b
            .samples_mut()
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(self.parallel_pcm[..samples].as_chunks::<2>().0.iter())
            .zip(self.room_pcm[..samples].as_chunks::<2>().0.iter())
        {
            self.master_gain += (self.schedule.master - self.master_gain) * smoothing;
            self.room_return += (self.schedule.room - self.room_return) * smoothing;
            self.parallel_return += (self.schedule.parallel - self.parallel_return) * smoothing;
            for channel in 0..2 {
                out[channel] = (out[channel]
                    + parallel[channel] * self.parallel_return
                    + room[channel] * self.room_return)
                    * self.master_gain;
            }
        }
        self.limiter.process(b, ctx);
        let peak = b.samples().iter().fold(0f32, |a, x| a.max(x.abs()));
        self.shared.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.shared
            .voices
            .store(self.sample_voices() as u64, Ordering::Relaxed);
        self.shared.position.store(
            (self.position as f64 * self.schedule.bpm / (spec.sample_rate * 15.)).to_bits(),
            Ordering::Relaxed,
        );
        self.shared.cycle.store(self.cycle, Ordering::Relaxed);
    }
}
