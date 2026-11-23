use crate::Result;
use serde::Deserialize;
use soniclaw_dsp::Sample;
use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};

#[derive(Clone, Debug, Deserialize)]
pub struct Variant {
    pub label: String,
    pub groups: Vec<(u8, String)>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Lane {
    pub id: String,
    pub name: String,
    pub ch: String,
    pub note: u8,
    pub key: String,
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub pan: f32,
    pub volume: f32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Groove {
    pub id: String,
    pub name: String,
    pub bars: u32,
    pub signature: String,
    pub events: Vec<crate::model::Hit>,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub feel: String,
    #[serde(default)]
    pub bpm: Option<f64>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Library {
    pub lanes: Vec<Lane>,
    pub channels: Vec<Channel>,
    pub grooves: Vec<Groove>,
}

pub struct Content {
    pub root: PathBuf,
    pub library: Library,
    pub groups: BTreeMap<String, Vec<Sample>>,
    pub sample_count: usize,
    pub pcm_bytes: usize,
}
impl Content {
    pub fn load(root: &Path) -> Result<Arc<Self>> {
        let library: Library = serde_json::from_slice(&std::fs::read(root.join("library.json"))?)?;
        if library.lanes.len() != 17 || library.channels.len() != 11 || library.grooves.is_empty() {
            return Err("Invalid Iron Room factory library".into());
        }
        let manifest: BTreeMap<String, Vec<String>> =
            serde_json::from_slice(&std::fs::read(root.join("samples.json"))?)?;
        let mut groups = BTreeMap::new();
        let mut count = 0;
        let mut pcm_bytes = 0;
        let canonical_root = root.canonicalize()?;
        for (group, files) in manifest {
            let mut samples = Vec::new();
            for file in files {
                let path = root.join(file).canonicalize()?;
                if !path.starts_with(&canonical_root) {
                    return Err("Sample escapes asset folder".into());
                }
                let bytes = std::fs::read(path)?;
                let sample = decode(&bytes)?;
                pcm_bytes += sample.data().len() * 4;
                if pcm_bytes > 512 * 1024 * 1024 {
                    return Err("Factory PCM budget exceeded".into());
                }
                samples.push(sample);
                count += 1;
            }
            if samples.is_empty() {
                return Err("Empty sample group".into());
            }
            groups.insert(group, samples);
        }
        for lane in &library.lanes {
            for variant in &lane.variants {
                for (_, group) in &variant.groups {
                    if !groups.contains_key(group) {
                        return Err(format!("Missing sample group {group}").into());
                    }
                }
            }
        }
        Ok(Arc::new(Self {
            root: root.to_owned(),
            library,
            groups,
            sample_count: count,
            pcm_bytes,
        }))
    }
    pub fn lane_index(&self, id: &str) -> Option<usize> {
        self.library.lanes.iter().position(|l| l.id == id)
    }
    pub fn channel_index(&self, lane: usize) -> usize {
        self.library
            .channels
            .iter()
            .position(|c| c.id == self.library.lanes[lane].ch)
            .unwrap()
    }
}

/// Decode only on a loading/control worker. The original encoded recordings stay unchanged.
pub fn decode(bytes: &[u8]) -> Result<Sample> {
    if bytes.is_empty() || bytes.len() > 32 * 1024 * 1024 {
        return Err("Sample must be 1 byte to 32 MiB".into());
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let probed = symphonia::default::get_probe().format(
        &Hint::new(),
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("No audio track")?;
    let id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut pcm = Vec::new();
    let mut spec = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != id {
            continue;
        }
        let audio = decoder.decode(&packet)?;
        let s = *audio.spec();
        if !(1..=2).contains(&s.channels.count()) || !(8000..=192000).contains(&s.rate) {
            return Err("Use mono/stereo audio at 8–192 kHz".into());
        }
        if spec.is_some_and(|old| old != s) {
            return Err("Sample changes audio format".into());
        }
        spec = Some(s);
        let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, s);
        samples.copy_interleaved_ref(audio);
        pcm.extend_from_slice(samples.samples());
        if pcm.len() > s.rate as usize * s.channels.count() * 30 {
            return Err("Sample exceeds 30 seconds".into());
        }
    }
    let s = spec.ok_or("No decoded audio")?;
    Ok(Sample::new(pcm, s.channels.count(), s.rate as f64, 60)?)
}

pub fn find_assets(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.join("library.json").is_file() {
            return Ok(path);
        }
        return Err("Asset folder has no library.json".into());
    }
    if let Some(path) = std::env::var_os("IRON_ROOM_ASSETS") {
        return find_assets(Some(path.into()));
    }
    let executable = std::env::current_exe()?;
    let parent = executable.parent().ok_or("Executable has no parent")?;
    for path in [
        parent.join("assets"),
        parent.join("../Resources/assets"),
        PathBuf::from("assets"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"),
    ] {
        if path.join("library.json").is_file() {
            return Ok(path);
        }
    }
    Err("Factory assets not found. Keep assets beside the executable or use --assets PATH.".into())
}
