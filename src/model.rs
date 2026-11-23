use crate::{
    content::{Content, Groove},
    Result,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
pub const LANES: usize = 17;
pub const CHANNELS: usize = 11;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Hit {
    pub lane: String,
    pub step: f64,
    pub velocity: u8,
    pub prob: f32,
    pub offset: f64,
}
impl Default for Hit {
    fn default() -> Self {
        Self {
            lane: "kick".into(),
            step: 0.,
            velocity: 100,
            prob: 1.,
            offset: 0.,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub bars: u32,
    pub signature: String,
    pub events: Vec<Hit>,
}
impl Pattern {
    pub fn length(&self) -> f64 {
        let (n, d) = meter(&self.signature).unwrap_or((4, 4));
        f64::from(n * 16 / d * self.bars)
    }
    pub fn from_groove(g: &Groove) -> Self {
        Self {
            name: g.name.clone(),
            bars: g.bars,
            signature: g.signature.clone(),
            events: g.events.clone(),
        }
    }
}
pub fn meter(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('/')?;
    let n: u32 = a.parse().ok()?;
    let d: u32 = b.parse().ok()?;
    ((1..=16).contains(&n) && [2, 4, 8, 16].contains(&d)).then_some((n, d))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub pattern: usize,
    pub repeats: u32,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Strip {
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub polarity: bool,
    pub tune: f32,
    pub decay: f32,
    pub hpf: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup: f32,
    pub room: f32,
}
impl Default for Strip {
    fn default() -> Self {
        Self {
            volume: 0.75,
            pan: 0.,
            mute: false,
            solo: false,
            polarity: false,
            tune: 0.,
            decay: 1.,
            hpf: 25.,
            low: 0.,
            mid: 0.,
            high: 0.,
            threshold: -18.,
            ratio: 1.,
            attack: 10.,
            release: 100.,
            makeup: 0.,
            room: 0.12,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomSample {
    pub velocity: u8,
    pub name: String,
    pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub format: String,
    pub version: u32,
    pub title: String,
    pub bpm: f64,
    pub swing: f64,
    pub humanize: f64,
    pub master: f32,
    pub room: f32,
    pub room_decay: f32,
    pub parallel: f32,
    pub channels: [Strip; CHANNELS],
    pub variants: [usize; LANES],
    pub midi: [u8; LANES],
    pub midi_channel: u8,
    pub pedal_closed_high: bool,
    pub patterns: Vec<Pattern>,
    pub selected: usize,
    pub song: Vec<Clip>,
    pub song_mode: bool,
    pub looping: bool,
    pub custom: Vec<Vec<CustomSample>>,
}
impl Default for Project {
    fn default() -> Self {
        Self {
            format: "iron-room-soniclaw".into(),
            version: 1,
            title: "Untitled session".into(),
            bpm: 112.,
            swing: 0.,
            humanize: 0.,
            master: 0.72,
            room: 0.16,
            room_decay: 1.2,
            parallel: 0.12,
            channels: [Strip::default(); CHANNELS],
            variants: [0; LANES],
            midi: [
                36, 38, 37, 40, 42, 46, 26, 44, 48, 43, 49, 57, 51, 53, 52, 55, 56,
            ],
            midi_channel: 0,
            pedal_closed_high: true,
            patterns: vec![Pattern {
                name: "Empty pattern".into(),
                bars: 2,
                signature: "4/4".into(),
                events: vec![],
            }],
            selected: 0,
            song: vec![],
            song_mode: false,
            looping: true,
            custom: vec![vec![]; LANES],
        }
    }
}
impl Project {
    pub fn factory(content: &Content) -> Self {
        let mut p = Self::default();
        for (s, c) in p.channels.iter_mut().zip(&content.library.channels) {
            s.volume = c.volume;
            s.pan = c.pan;
        }
        p.patterns[0] = Pattern::from_groove(&content.library.grooves[0]);
        p
    }
    pub fn pattern(&self) -> &Pattern {
        &self.patterns[self.selected]
    }
    pub fn pattern_mut(&mut self) -> &mut Pattern {
        &mut self.patterns[self.selected]
    }
    pub fn validate(&self, content: &Content) -> Result<()> {
        if self.format != "iron-room-soniclaw" || self.version != 1 {
            return Err("Unsupported project format/version".into());
        }
        if self.title.len() > 256
            || !(1..=128).contains(&self.patterns.len())
            || self.selected >= self.patterns.len()
            || self.song.len() > 128
            || self.custom.len() != LANES
            || self.midi_channel > 16
        {
            return Err("Invalid project dimensions".into());
        }
        for (v, lo, hi) in [
            (self.bpm, 30., 300.),
            (self.swing, 0., 0.65),
            (self.humanize, 0., 30.),
            (self.master as f64, 0., 1.5),
            (self.room as f64, 0., 1.),
            (self.parallel as f64, 0., 1.),
            (self.room_decay as f64, 0.1, 10.),
        ] {
            if !v.is_finite() || v < lo || v > hi {
                return Err("Project control is out of bounds".into());
            }
        }
        for (i, s) in self.channels.iter().enumerate() {
            for (v, lo, hi) in [
                (s.volume, 0., 1.5),
                (s.pan, -1., 1.),
                (s.tune, -12., 12.),
                (s.decay, 0.05, 1.),
                (s.hpf, 20., 2000.),
                (s.low, -18., 18.),
                (s.mid, -18., 18.),
                (s.high, -18., 18.),
                (s.threshold, -60., 0.),
                (s.ratio, 1., 20.),
                (s.attack, 0.1, 100.),
                (s.release, 10., 1000.),
                (s.makeup, 0., 18.),
                (s.room, 0., 1.),
            ] {
                if !v.is_finite() || v < lo || v > hi {
                    return Err(format!("Channel {i} control out of bounds").into());
                }
            }
        }
        for (i, v) in self.variants.iter().enumerate() {
            if *v >= content.library.lanes[i].variants.len() || self.midi[i] > 127 {
                return Err("Invalid articulation or MIDI mapping".into());
            }
        }
        let mut encoded = 0;
        for layer in &self.custom {
            if layer.len() > 64 {
                return Err("At most 64 custom samples per articulation".into());
            }
            for s in layer {
                encoded += s.data.len();
                if s.velocity == 0
                    || s.velocity > 127
                    || s.name.len() > 256
                    || s.data.len() > 44 * 1024 * 1024
                {
                    return Err("Invalid custom sample".into());
                }
            }
        }
        if encoded > 96 * 1024 * 1024 {
            return Err("Custom sample package exceeds 96 MiB encoded".into());
        }
        for pattern in &self.patterns {
            if meter(&pattern.signature).is_none()
                || !(1..=16).contains(&pattern.bars)
                || pattern.events.len() > 8192
                || pattern.name.len() > 256
            {
                return Err("Invalid pattern".into());
            }
            for h in &pattern.events {
                if content.lane_index(&h.lane).is_none()
                    || !h.step.is_finite()
                    || h.step < 0.
                    || h.step >= pattern.length()
                    || h.velocity == 0
                    || h.velocity > 127
                    || !h.prob.is_finite()
                    || !(0. ..=1.).contains(&h.prob)
                    || !h.offset.is_finite()
                    || h.offset.abs() > 100.
                {
                    return Err("Invalid drum hit".into());
                }
            }
        }
        for clip in &self.song {
            if clip.pattern >= self.patterns.len() || !(1..=32).contains(&clip.repeats) {
                return Err("Invalid song clip".into());
            }
        }
        Ok(())
    }
    pub fn save(&self, path: &Path, content: &Content) -> Result<()> {
        self.validate(content)?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > 128 * 1024 * 1024 {
            return Err("Project exceeds 128 MiB".into());
        }
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temp, bytes)?;
        if let Err(e) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e.into());
        }
        Ok(())
    }
    pub fn open(path: &Path, content: &Content) -> Result<Self> {
        if std::fs::metadata(path)?.len() > 128 * 1024 * 1024 {
            return Err("Project exceeds 128 MiB".into());
        }
        let p: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        p.validate(content)?;
        Ok(p)
    }
    pub fn kit(&mut self, index: usize) {
        self.variants.fill(0);
        match index {
            1 => {
                self.variants[0] = 1;
                self.variants[1] = 1;
                self.variants[3] = 1;
            }
            2 => {
                self.variants[10] = 1;
                self.variants[11] = 1;
            }
            3 => self.variants[5] = 5,
            4 => self.variants[12] = 1,
            _ => {}
        }
        for layer in &mut self.custom {
            layer.clear();
        }
    }
    pub fn mix_preset(&mut self, index: usize) {
        for c in &mut self.channels {
            c.low = 0.;
            c.mid = 0.;
            c.high = 0.;
            c.ratio = 1.;
            c.makeup = 0.;
            c.room = 0.12;
        }
        self.parallel = 0.12;
        self.room = 0.16;
        self.room_decay = 1.2;
        match index {
            1 => {
                self.parallel = 0.3;
                self.room = 0.12;
                for c in &mut self.channels[..2] {
                    c.ratio = 4.;
                    c.threshold = -22.;
                    c.low = 2.;
                    c.high = 2.;
                }
            }
            2 => {
                self.room = 0.45;
                self.room_decay = 2.8;
                self.parallel = 0.2;
                for c in &mut self.channels {
                    c.room = 0.35;
                }
            }
            3 => {
                self.room = 0.;
                self.parallel = 0.;
            }
            4 => {
                self.room = 0.22;
                for c in &mut self.channels {
                    c.low = 2.;
                    c.high = -3.;
                }
            }
            5 => {
                self.room = 0.08;
                for c in &mut self.channels {
                    c.high = 3.;
                    c.mid = -1.;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimedHit {
    pub step: f64,
    pub lane: usize,
    pub velocity: u8,
    pub probability: f32,
    pub index: usize,
}
pub struct Schedule {
    pub hits: Vec<TimedHit>,
    pub length: f64,
    pub bpm: f64,
    pub looping: bool,
    pub variants: [usize; LANES],
    pub midi: [u8; LANES],
    pub midi_channel: u8,
    pub pedal_closed_high: bool,
    pub channels: [Strip; CHANNELS],
    pub master: f32,
    pub room: f32,
    pub parallel: f32,
}
impl Schedule {
    pub fn build(p: &Project, c: &Content) -> Result<Box<Self>> {
        p.validate(c)?;
        let mut hits = Vec::new();
        let mut length = 0.;
        let clips = if p.song_mode && !p.song.is_empty() {
            p.song.clone()
        } else {
            vec![Clip {
                pattern: p.selected,
                repeats: 1,
            }]
        };
        for clip in clips {
            for _ in 0..clip.repeats {
                let pattern = &p.patterns[clip.pattern];
                for h in &pattern.events {
                    let mut step = h.step;
                    if (step / 2.).floor() as u64 % 2 == 1 {
                        step += p.swing * 2.;
                    }
                    let jitter = if p.humanize > 0. {
                        let hash = (hits.len() as u64 + 1).wrapping_mul(6364136223846793005);
                        (hash >> 32) as f64 / u32::MAX as f64 * 2. - 1.
                    } else {
                        0.
                    };
                    step += (h.offset + jitter * p.humanize) * p.bpm / 15000.;
                    hits.push(TimedHit {
                        step: length + step.clamp(0., pattern.length() - 0.00001),
                        lane: c.lane_index(&h.lane).unwrap(),
                        velocity: h.velocity,
                        probability: h.prob,
                        index: hits.len(),
                    });
                    if hits.len() > 65536 {
                        return Err("Song exceeds 65,536 hits".into());
                    }
                }
                length += pattern.length();
            }
        }
        if length * 15. / p.bpm > 4. * 3600. {
            return Err("Song exceeds four hours".into());
        }
        hits.sort_by(|a, b| a.step.total_cmp(&b.step));
        Ok(Box::new(Self {
            hits,
            length,
            bpm: p.bpm,
            looping: p.looping,
            variants: p.variants,
            midi: p.midi,
            midi_channel: p.midi_channel,
            pedal_closed_high: p.pedal_closed_high,
            channels: p.channels,
            master: p.master,
            room: p.room,
            parallel: p.parallel,
        }))
    }
}
