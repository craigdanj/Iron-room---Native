use crate::{
    content::Content,
    engine::{Control, DrumProcessor},
    model::*,
    Result,
};
use base64::Engine as _;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use soniclaw_device::{event_channel, OutputHost, Telemetry};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

const ORANGE: Color32 = Color32::from_rgb(244, 137, 77);
const TEAL: Color32 = Color32::from_rgb(92, 195, 177);
const MUTED: Color32 = Color32::from_rgb(148, 153, 160);
const PANEL: Color32 = Color32::from_rgb(28, 31, 36);
const KIT_NAMES: [&str; 5] = [
    "Studio birch",
    "Dry birch",
    "Bright rock",
    "Loose hats",
    "Dark ride",
];
const MIX_NAMES: [&str; 6] = [
    "Studio natural",
    "Modern punch",
    "Big room",
    "Dry & intimate",
    "Vintage warmth",
    "Crisp funk",
];
type DragState = (f64, usize, Vec<(usize, f64, String)>);
type ImportResult = std::result::Result<(usize, Vec<CustomSample>), String>;
type SoundResult = std::result::Result<(DrumProcessor, Control), String>;
struct Audio {
    host: OutputHost,
    control: Control,
    _midi: Option<soniclaw_midi::native::Connection>,
}
struct ExportJob {
    rx: mpsc::Receiver<std::result::Result<String, String>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

pub struct IronRoom {
    content: Arc<Content>,
    project: Project,
    audio: Option<Audio>,
    sound: Option<mpsc::Receiver<SoundResult>>,
    sound_dirty: bool,
    schedule_dirty: bool,
    device: Option<usize>,
    midi: Option<usize>,
    devices: Vec<String>,
    ports: Vec<String>,
    tab: usize,
    query: String,
    family: String,
    groove: Option<usize>,
    selected_lane: usize,
    selected_channel: usize,
    selected_hits: Vec<usize>,
    snap: f64,
    tool: usize,
    drag: Option<DragState>,
    marquee: Option<Pos2>,
    clipboard: Vec<Hit>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    status: String,
    project_path: Option<PathBuf>,
    settings: bool,
    job: Option<ExportJob>,
    import: Option<mpsc::Receiver<ImportResult>>,
    no_audio: bool,
    autosave: PathBuf,
    autosave_at: Instant,
    autosave_dirty: bool,
    peaks: [f32; CHANNELS],
    master_peak: f32,
    kit_name: usize,
    mix_name: usize,
    playing_after_prepare: bool,
    ui_smoke: Option<PathBuf>,
    smoke_frames: u32,
}
impl IronRoom {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        content: Arc<Content>,
        project: Project,
        no_audio: bool,
        ui_smoke: Option<PathBuf>,
    ) -> Self {
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Dark)).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(20, 22, 26);
        style.visuals.window_fill = PANEL;
        style.visuals.selection.bg_fill = Color32::from_rgb(124, 65, 38);
        style.visuals.selection.stroke = Stroke::new(1., ORANGE);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(41, 45, 51);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(60, 64, 71);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(110, 65, 42);
        style.spacing.item_spacing = Vec2::new(8., 7.);
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_style_of(egui::Theme::Dark, style);
        let autosave = data_dir().join("autosave.ironroom-s");
        let mut app = Self {
            content,
            project,
            audio: None,
            sound: None,
            sound_dirty: false,
            schedule_dirty: false,
            device: None,
            midi: None,
            devices: OutputHost::devices().unwrap_or_default(),
            ports: soniclaw_midi::native::input_ports().unwrap_or_default(),
            tab: 0,
            query: String::new(),
            family: "All styles".into(),
            groove: Some(0),
            selected_lane: 0,
            selected_channel: 0,
            selected_hits: vec![],
            snap: 1.,
            tool: 0,
            drag: None,
            marquee: None,
            clipboard: vec![],
            undo: vec![],
            redo: vec![],
            status: "Loading acoustic engine…".into(),
            project_path: None,
            settings: false,
            job: None,
            import: None,
            no_audio,
            autosave,
            autosave_at: Instant::now(),
            autosave_dirty: false,
            peaks: [0.; CHANNELS],
            master_peak: 0.,
            kit_name: 0,
            mix_name: 0,
            playing_after_prepare: false,
            ui_smoke,
            smoke_frames: 0,
        };
        if no_audio {
            app.status = "Offline editing · WAV and MIDI export available".into();
        } else {
            app.prepare_sound();
        }
        app
    }
    fn checkpoint(&mut self) {
        self.undo.push(self.project.clone());
        trim_history(&mut self.undo);
        self.redo.clear();
    }
    fn changed(&mut self) {
        self.schedule_dirty = true;
        self.autosave_dirty = true;
    }
    fn sound_changed(&mut self) {
        self.sound_dirty = true;
        self.changed();
    }
    fn prepare_sound(&mut self) {
        if self.sound.is_some() {
            return;
        }
        self.playing_after_prepare = self
            .audio
            .as_ref()
            .is_some_and(|a| a.control.shared.running.load(Ordering::Relaxed));
        self.sound_dirty = false;
        let content = self.content.clone();
        let project = self.project.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(DrumProcessor::new(content, &project).map_err(|e| e.to_string()));
        });
        self.sound = Some(rx);
        self.status = "Preparing sample voices and mixer…".into();
    }
    fn accept_sound(&mut self) {
        let result = self.sound.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.sound = None;
            match result.and_then(|(processor, control)| {
                // Keep the old audio owner alive until replacement construction succeeds.
                self.audio = None;
                let telemetry = Arc::new(Telemetry::default());
                let (mut sender, receiver) = event_channel(512, telemetry.clone());
                let midi = self
                    .midi
                    .map(|port| {
                        soniclaw_midi::native::connect_input(port, move |message| {
                            sender.send_timed(message);
                        })
                    })
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let mut engine = soniclaw_engine::Engine::default();
                engine.push(processor);
                let host = OutputHost::start(engine, self.device, 512, vec![receiver], telemetry)
                    .map_err(|e| e.to_string())?;
                Ok(Audio {
                    host,
                    control,
                    _midi: midi,
                })
            }) {
                Ok(audio) => {
                    if self.playing_after_prepare {
                        audio.control.play();
                    }
                    self.status = format!(
                        "{} · {} Hz",
                        audio.host.device_name, audio.host.spec.sample_rate
                    );
                    self.audio = Some(audio);
                    self.schedule_dirty = true;
                }
                Err(error) => {
                    self.status =
                        format!("Audio unavailable: {error}. Edit and export remain available.")
                }
            }
        }
    }
    fn tick(&mut self) {
        self.accept_sound();
        if let Some(audio) = &mut self.audio {
            audio.control.collect();
            if self.schedule_dirty {
                match Schedule::build(&self.project, &self.content) {
                    Ok(s) => {
                        if audio.control.publish(s) {
                            self.schedule_dirty = false;
                        }
                    }
                    Err(e) => {
                        self.status = e.to_string();
                        self.schedule_dirty = false;
                    }
                }
            }
            for (i, v) in self.peaks.iter_mut().enumerate() {
                *v = f32::from_bits(audio.control.shared.peaks[i].load(Ordering::Relaxed))
                    .max(*v * 0.88);
            }
            self.master_peak = f32::from_bits(audio.control.shared.peak.load(Ordering::Relaxed))
                .max(self.master_peak * 0.9);
        }
        let mut recorded = Vec::new();
        if let Some(audio) = &mut self.audio {
            while let Ok(hit) = audio.control.recorded.pop() {
                recorded.push(hit);
            }
        }
        if !recorded.is_empty() {
            self.checkpoint();
            for h in recorded {
                let mut step = h.step;
                let mut pattern = self.project.selected;
                if self.project.song_mode && !self.project.song.is_empty() {
                    'song: for clip in &self.project.song {
                        let length = self.project.patterns[clip.pattern].length();
                        for _ in 0..clip.repeats {
                            if step < length {
                                pattern = clip.pattern;
                                break 'song;
                            }
                            step -= length;
                        }
                    }
                }
                if step < self.project.patterns[pattern].length()
                    && self.project.patterns[pattern].events.len() < 8192
                {
                    self.project.patterns[pattern].events.push(Hit {
                        lane: self.content.library.lanes[h.lane].id.clone(),
                        step,
                        velocity: h.velocity,
                        ..Default::default()
                    });
                }
            }
            self.changed();
        }
        if let Some(result) = self.job.as_ref().and_then(|j| j.rx.try_recv().ok()) {
            self.status = result.unwrap_or_else(|e| e);
            self.job = None;
        }
        if let Some(result) = self.import.as_ref().and_then(|j| j.try_recv().ok()) {
            self.import = None;
            match result {
                Ok((lane, samples)) => {
                    self.checkpoint();
                    self.project.custom[lane] = samples;
                    self.sound_changed();
                    if !self.no_audio {
                        self.prepare_sound();
                    }
                    self.status =
                        "Custom recordings imported; save the project to keep a portable copy."
                            .into();
                }
                Err(e) => self.status = e,
            }
        }
        if self.autosave_dirty && self.autosave_at.elapsed() > Duration::from_secs(15) {
            if self.project.custom.iter().all(Vec::is_empty) {
                if let Some(parent) = self.autosave.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if self.project.save(&self.autosave, &self.content).is_ok() {
                    self.autosave_dirty = false;
                }
            }
            self.autosave_at = Instant::now();
        }
    }
    fn audition(&mut self, lane: usize, soft: bool) {
        self.selected_lane = lane;
        self.selected_channel = self.content.channel_index(lane);
        if let Some(audio) = &mut self.audio {
            if !audio.control.trigger(lane, if soft { 55 } else { 108 }) {
                self.status = "Audition queue full".into();
            }
        }
    }
    fn play_stop(&mut self) {
        if let Some(audio) = &self.audio {
            if audio.control.shared.running.load(Ordering::Relaxed) {
                audio.control.stop();
            } else {
                audio.control.play();
            }
        } else if !self.no_audio {
            self.prepare_sound();
            self.playing_after_prepare = true;
        }
    }
    fn undo(&mut self, redo: bool) {
        let next = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(next) = next {
            if redo {
                self.undo.push(self.project.clone());
                trim_history(&mut self.undo);
            } else {
                self.redo.push(self.project.clone());
                trim_history(&mut self.redo);
            }
            self.project = next;
            self.selected_hits.clear();
            self.sound_changed();
        }
    }
    fn save(&mut self, save_as: bool) {
        let path = if save_as || self.project_path.is_none() {
            rfd::FileDialog::new()
                .add_filter("Iron Room Soniclaw project", &["ironroom-s"])
                .set_file_name("session.ironroom-s")
                .save_file()
        } else {
            self.project_path.clone()
        };
        if let Some(path) = path {
            match self.project.save(&path, &self.content) {
                Ok(()) => {
                    self.status = format!("Saved {}", path.display());
                    self.project_path = Some(path);
                    self.autosave_dirty = false;
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }
    fn open(&mut self, path: Option<PathBuf>) {
        let path = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("Iron Room Soniclaw project", &["ironroom-s"])
                .pick_file()
        });
        if let Some(path) = path {
            match Project::open(&path, &self.content) {
                Ok(p) => {
                    self.checkpoint();
                    self.project = p;
                    self.project_path = Some(path);
                    self.selected_hits.clear();
                    self.sound_changed();
                    if !self.no_audio {
                        self.prepare_sound();
                    }
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }
    fn export(&mut self, wav: bool) {
        if self.job.is_some() {
            return;
        }
        let path = if wav {
            rfd::FileDialog::new()
                .add_filter("24-bit WAV", &["wav"])
                .set_file_name("iron-room-mix.wav")
                .save_file()
        } else {
            rfd::FileDialog::new()
                .add_filter("Standard MIDI", &["mid"])
                .set_file_name("iron-room-groove.mid")
                .save_file()
        };
        let Some(path) = path else {
            return;
        };
        let content = self.content.clone();
        let project = self.project.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0));
        let c = cancel.clone();
        let p = progress.clone();
        std::thread::spawn(move || {
            let result = if wav {
                crate::export::wav(content, &project, &path, 44100, &c, &p)
                    .map(|n| format!("Exported {} · {n} stereo frames", path.display()))
            } else {
                crate::export::midi(&content, &project, &path)
                    .map(|n| format!("Exported {} · {n} drum hits", path.display()))
            };
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        self.job = Some(ExportJob {
            rx,
            cancel,
            progress,
        });
        self.status = "Exporting on a background worker…".into();
    }
    fn import_samples(&mut self) {
        if self.import.is_some() {
            return;
        }
        let Some(paths) = rfd::FileDialog::new()
            .add_filter("Audio samples", &["wav", "mp3", "flac", "aiff", "aif"])
            .pick_files()
        else {
            return;
        };
        let lane = self.selected_lane;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<_> {
                if paths.len() > 64 {
                    return Err("Choose at most 64 recordings".into());
                }
                let mut out = Vec::new();
                let mut total = 0;
                for path in paths {
                    let metadata = std::fs::metadata(&path)?;
                    if metadata.len() > 32 * 1024 * 1024 {
                        return Err("Sample exceeds 32 MiB".into());
                    }
                    total += metadata.len();
                    if total > 64 * 1024 * 1024 {
                        return Err("Import exceeds 64 MiB".into());
                    }
                    let bytes = std::fs::read(&path)?;
                    crate::content::decode(&bytes)?;
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    let velocity = name
                        .split('_')
                        .find_map(|s| {
                            s.strip_prefix('v')
                                .and_then(|x| x.split('.').next()?.parse::<u8>().ok())
                        })
                        .unwrap_or(127);
                    if velocity == 0 || velocity > 127 {
                        return Err("Velocity filename must use v1…v127".into());
                    }
                    out.push(CustomSample {
                        velocity,
                        name,
                        data: base64::prelude::BASE64_STANDARD.encode(bytes),
                    });
                }
                Ok((lane, out))
            })();
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        self.import = Some(rx);
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("IRON ROOM").size(26.).strong().color(ORANGE));
            ui.label(RichText::new("ACOUSTIC DRUM STUDIO").size(10.).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Audio / MIDI").clicked() {
                    self.settings = true;
                }
                if ui.button("Export MIDI").clicked() {
                    self.export(false);
                }
                if ui.button("Export WAV").clicked() {
                    self.export(true);
                }
                if ui.button("Save as").clicked() {
                    self.save(true);
                }
                if ui.button("Save").clicked() {
                    self.save(false);
                }
                if ui.button("Open").clicked() {
                    self.open(None);
                }
                if ui.button("New").clicked() {
                    self.checkpoint();
                    self.project = Project::factory(&self.content);
                    self.project.patterns[0].events.clear();
                    self.project_path = None;
                    self.sound_changed();
                }
            });
        });
        ui.add_space(5.);
        ui.horizontal(|ui| {
            let playing = self
                .audio
                .as_ref()
                .is_some_and(|a| a.control.shared.running.load(Ordering::Relaxed));
            if ui
                .add_sized(
                    [88., 34.],
                    egui::Button::new(if playing { "■  STOP" } else { "▶  PLAY" }).fill(
                        if playing {
                            Color32::from_rgb(90, 47, 32)
                        } else {
                            Color32::from_rgb(49, 90, 78)
                        },
                    ),
                )
                .clicked()
            {
                self.play_stop();
            }
            let mut recording = self
                .audio
                .as_ref()
                .is_some_and(|a| a.control.shared.record.load(Ordering::Relaxed));
            if ui
                .toggle_value(
                    &mut recording,
                    RichText::new("● REC").color(Color32::from_rgb(230, 102, 97)),
                )
                .changed()
            {
                if let Some(a) = &self.audio {
                    a.control.shared.record.store(recording, Ordering::Relaxed);
                }
            }
            ui.label("BPM");
            if ui
                .add(
                    egui::DragValue::new(&mut self.project.bpm)
                        .range(30. ..=300.)
                        .speed(0.2),
                )
                .changed()
            {
                self.changed();
            }
            if ui.checkbox(&mut self.project.looping, "Loop").changed() {
                self.changed();
            }
            if ui
                .selectable_value(&mut self.project.song_mode, false, "Pattern")
                .changed()
            {
                self.changed();
            }
            if ui
                .selectable_value(&mut self.project.song_mode, true, "Song")
                .changed()
            {
                self.changed();
            }
            ui.separator();
            ui.label("Swing");
            if ui
                .add(egui::Slider::new(&mut self.project.swing, 0. ..=0.65).show_value(false))
                .changed()
            {
                self.changed();
            }
            ui.label(format!("{}%", (self.project.swing * 100.) as u32));
            ui.label("Feel");
            if ui
                .add(
                    egui::DragValue::new(&mut self.project.humanize)
                        .range(0. ..=30.)
                        .suffix(" ms"),
                )
                .changed()
            {
                self.changed();
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("↶"))
                .clicked()
            {
                self.undo(false);
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new("↷"))
                .clicked()
            {
                self.undo(true);
            }
            if self.sound_dirty
                && ui
                    .add(
                        egui::Button::new("Apply sound edits").fill(Color32::from_rgb(104, 60, 34)),
                    )
                    .clicked()
            {
                if self.no_audio {
                    self.sound_dirty = false;
                } else {
                    self.prepare_sound();
                }
            }
        });
        ui.add_space(3.);
    }
    fn browser(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("GROOVE LIBRARY").color(ORANGE).strong());
        ui.small("2,375 original patterns");
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Search grooves…")
                .desired_width(f32::INFINITY),
        );
        let mut families: Vec<_> = self
            .content
            .library
            .grooves
            .iter()
            .map(|g| g.family.as_str())
            .collect();
        families.sort();
        families.dedup();
        egui::ComboBox::from_id_salt("family")
            .selected_text(&self.family)
            .width(205.)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.family, "All styles".into(), "All styles");
                for f in families {
                    ui.selectable_value(&mut self.family, f.into(), f);
                }
            });
        let query = self.query.to_lowercase();
        let filtered: Vec<usize> = self
            .content
            .library
            .grooves
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                (self.family == "All styles" || g.family == self.family)
                    && (query.is_empty()
                        || format!("{} {} {} {}", g.name, g.family, g.feel, g.signature)
                            .to_lowercase()
                            .contains(&query))
            })
            .map(|(i, _)| i)
            .collect();
        ui.small(format!("{} results", filtered.len()));
        let height = (ui.available_height() - 90.).max(80.);
        egui::ScrollArea::vertical().max_height(height).show_rows(
            ui,
            39.,
            filtered.len(),
            |ui, rows| {
                for row in rows {
                    let i = filtered[row];
                    let g = &self.content.library.grooves[i];
                    let label = format!(
                        "{}\n{} · {} · {} bars",
                        g.name, g.family, g.signature, g.bars
                    );
                    let r = ui.add_sized(
                        [ui.available_width(), 37.],
                        egui::Button::new(label).selected(self.groove == Some(i)),
                    );
                    if r.clicked() {
                        self.groove = Some(i);
                    }
                    if r.double_clicked() {
                        self.load_groove(i);
                    }
                }
            },
        );
        ui.separator();
        if ui
            .add_enabled(
                self.groove.is_some(),
                egui::Button::new("Load into pattern")
                    .min_size(Vec2::new(ui.available_width(), 30.)),
            )
            .clicked()
        {
            if let Some(i) = self.groove {
                self.load_groove(i);
            }
        }
        ui.small("Double-click a groove to load.\nOdd meters and triplets are retained.");
    }
    fn load_groove(&mut self, index: usize) {
        self.checkpoint();
        self.project.patterns[self.project.selected] =
            Pattern::from_groove(&self.content.library.grooves[index]);
        self.selected_hits.clear();
        self.changed();
        self.tab = 1;
    }
    fn kit(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("The live room");
            ui.label(RichText::new("SALAMANDER / ACOUSTIC").color(MUTED));
            egui::ComboBox::from_id_salt("kit")
                .selected_text(KIT_NAMES[self.kit_name])
                .show_ui(ui, |ui| {
                    for (i, name) in KIT_NAMES.iter().enumerate() {
                        if ui.selectable_label(self.kit_name == i, *name).clicked() {
                            self.checkpoint();
                            self.kit_name = i;
                            self.project.kit(i);
                            self.sound_changed();
                            if !self.no_audio {
                                self.prepare_sound();
                            }
                        }
                    }
                });
        });
        ui.label("Click a pad or play the letter below it. Hold Shift for a softer hit.");
        ui.add_space(12.);
        let width = ((ui.available_width() - 24.) / 4.).max(100.);
        egui::Grid::new("pads")
            .num_columns(4)
            .spacing([8., 8.])
            .show(ui, |ui| {
                for i in 0..LANES {
                    let lane = &self.content.library.lanes[i];
                    let selected = self.selected_lane == i;
                    let ch = self.content.channel_index(i);
                    let pulse = self.peaks[ch].min(1.);
                    let fill = if selected {
                        Color32::from_rgb(91, 55, 36)
                    } else if pulse > 0.15 {
                        Color32::from_rgb(39, 53, 49)
                    } else {
                        PANEL
                    };
                    let label = format!(
                        "{}\n\n[ {} ]   MIDI {}",
                        lane.name,
                        lane.key.to_uppercase(),
                        self.project.midi[i]
                    );
                    if ui
                        .add_sized(
                            [width, 82.],
                            egui::Button::new(RichText::new(label).size(13.))
                                .fill(fill)
                                .stroke(Stroke::new(
                                    if selected { 1.5 } else { 0.5 },
                                    if selected {
                                        ORANGE
                                    } else {
                                        Color32::from_gray(57)
                                    },
                                )),
                        )
                        .clicked()
                    {
                        self.audition(i, ui.input(|i| i.modifiers.shift));
                    }
                    if i % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(12.);
        ui.separator();
        self.articulation(ui);
    }
    fn articulation(&mut self, ui: &mut egui::Ui) {
        let i = self.selected_lane;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.content.library.lanes[i].name).strong());
            let variants = self.content.library.lanes[i].variants.clone();
            egui::ComboBox::from_id_salt("variant")
                .selected_text(&variants[self.project.variants[i]].label)
                .show_ui(ui, |ui| {
                    for (v, item) in variants.iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.project.variants[i], v, &item.label)
                            .changed()
                        {
                            self.changed();
                        }
                    }
                });
            if ui.button("Import recordings…").clicked() {
                self.import_samples();
            }
            if !self.project.custom[i].is_empty() {
                ui.label(format!(
                    "{} custom recordings",
                    self.project.custom[i].len()
                ));
                if ui.button("Restore factory").clicked() {
                    self.checkpoint();
                    self.project.custom[i].clear();
                    self.sound_changed();
                }
            }
        });
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let names: Vec<_> = self
                .project
                .patterns
                .iter()
                .map(|p| p.name.clone())
                .collect();
            egui::ComboBox::from_id_salt("pattern")
                .selected_text(&names[self.project.selected])
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.project.selected, i, n)
                            .changed()
                        {
                            self.selected_hits.clear();
                            self.changed();
                        }
                    }
                });
            if ui.button("+ Pattern").clicked() && self.project.patterns.len() < 128 {
                self.checkpoint();
                self.project.patterns.push(Pattern {
                    name: format!("Pattern {}", self.project.patterns.len() + 1),
                    bars: 2,
                    signature: "4/4".into(),
                    events: vec![],
                });
                self.project.selected = self.project.patterns.len() - 1;
                self.selected_hits.clear();
                self.changed();
            }
            if ui.button("Duplicate").clicked() && self.project.patterns.len() < 128 {
                self.checkpoint();
                let mut p = self.project.pattern().clone();
                p.name.push_str(" copy");
                self.project.patterns.push(p);
                self.project.selected = self.project.patterns.len() - 1;
                self.changed();
            }
            if ui.button("+ Song clip").clicked() && self.project.song.len() < 128 {
                self.checkpoint();
                self.project.song.push(Clip {
                    pattern: self.project.selected,
                    repeats: 1,
                });
                self.changed();
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.project.pattern_mut().name)
                    .desired_width(150.),
            );
            ui.label("Bars");
            let old = self.project.pattern().bars;
            if ui
                .add(egui::DragValue::new(&mut self.project.pattern_mut().bars).range(1..=16))
                .changed()
            {
                let length = self.project.pattern().length();
                self.project
                    .pattern_mut()
                    .events
                    .retain(|h| h.step < length);
                self.changed();
                if old != self.project.pattern().bars {
                    self.selected_hits.clear();
                }
            }
            let signature = self.project.pattern().signature.clone();
            egui::ComboBox::from_id_salt("meter")
                .selected_text(&signature)
                .show_ui(ui, |ui| {
                    for s in [
                        "4/4", "3/4", "5/4", "5/8", "6/8", "7/8", "9/8", "11/8", "12/8",
                    ] {
                        if ui.selectable_label(signature == s, s).clicked() {
                            self.checkpoint();
                            self.project.pattern_mut().signature = s.into();
                            let length = self.project.pattern().length();
                            self.project
                                .pattern_mut()
                                .events
                                .retain(|h| h.step < length);
                            self.selected_hits.clear();
                            self.changed();
                        }
                    }
                });
            ui.separator();
            ui.selectable_value(&mut self.tool, 0, "Draw");
            ui.selectable_value(&mut self.tool, 1, "Select");
            ui.selectable_value(&mut self.tool, 2, "Erase");
            egui::ComboBox::from_id_salt("snap")
                .selected_text(if self.snap == 1. {
                    "1/16"
                } else if self.snap == 0.5 {
                    "1/32"
                } else {
                    "1/16T"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.snap, 1., "1/16");
                    ui.selectable_value(&mut self.snap, 0.5, "1/32");
                    ui.selectable_value(&mut self.snap, 1. / 3., "1/16 triplet");
                });
        });
        let length = self.project.pattern().length();
        let col = 26.;
        let row = 25.;
        let left = 130.;
        let top = 25.;
        let position = self.audio.as_ref().map_or(0., |a| {
            f64::from_bits(a.control.shared.position.load(Ordering::Relaxed)) % length
        });
        egui::ScrollArea::both()
            .id_salt("drum-grid")
            .max_height(475.)
            .show(ui, |ui| {
                let (size, response) = ui.allocate_exact_size(
                    Vec2::new(
                        left + length as f32 * col + 8.,
                        top + LANES as f32 * row + 8.,
                    ),
                    Sense::click_and_drag(),
                );
                let painter = ui.painter_at(size);
                painter.rect_filled(size, 4., Color32::from_rgb(23, 26, 30));
                for i in 0..LANES {
                    let y = size.top() + top + i as f32 * row;
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(size.left(), y),
                            Vec2::new(size.width(), row - 1.),
                        ),
                        0.,
                        if i % 2 == 0 {
                            Color32::from_rgb(29, 32, 37)
                        } else {
                            Color32::from_rgb(25, 28, 32)
                        },
                    );
                    painter.text(
                        Pos2::new(size.left() + 8., y + row / 2.),
                        egui::Align2::LEFT_CENTER,
                        &self.content.library.lanes[i].name,
                        egui::FontId::proportional(11.),
                        if i == self.selected_lane {
                            ORANGE
                        } else {
                            MUTED
                        },
                    );
                }
                for s in 0..=length as usize {
                    let x = size.left() + left + s as f32 * col;
                    let major = s % 4 == 0;
                    painter.line_segment(
                        [Pos2::new(x, size.top() + top), Pos2::new(x, size.bottom())],
                        Stroke::new(
                            if major { 1. } else { 0.5 },
                            if major {
                                Color32::from_gray(69)
                            } else {
                                Color32::from_gray(43)
                            },
                        ),
                    );
                    if major {
                        painter.text(
                            Pos2::new(x + 4., size.top() + 11.),
                            egui::Align2::LEFT_CENTER,
                            format!("{}", s / 4 + 1),
                            egui::FontId::monospace(10.),
                            MUTED,
                        );
                    }
                }
                let mut under = None;
                let pointer = response.interact_pointer_pos();
                for (i, h) in self.project.pattern().events.iter().enumerate() {
                    let lane = self.content.lane_index(&h.lane).unwrap();
                    let x = size.left() + left + h.step as f32 * col;
                    let y = size.top() + top + lane as f32 * row;
                    let r = Rect::from_min_size(
                        Pos2::new(x + 1., y + 3.),
                        Vec2::new((col * self.snap as f32 - 2.).max(7.), row - 6.),
                    );
                    let fill = if self.selected_hits.contains(&i) {
                        TEAL
                    } else {
                        Color32::from_rgb(
                            110 + h.velocity,
                            70 + (h.velocity as f32 * 0.45) as u8,
                            45,
                        )
                    };
                    painter.rect_filled(r, 2., fill);
                    if pointer.is_some_and(|p| r.contains(p)) {
                        under = Some(i);
                    }
                }
                if self
                    .audio
                    .as_ref()
                    .is_some_and(|a| a.control.shared.running.load(Ordering::Relaxed))
                {
                    let x = size.left() + left + position as f32 * col;
                    painter.line_segment(
                        [Pos2::new(x, size.top()), Pos2::new(x, size.bottom())],
                        Stroke::new(1.5, TEAL),
                    );
                }
                let snap = self.snap;
                let locate = |p: Pos2| -> Option<(usize, f64)> {
                    let x = p.x - size.left() - left;
                    let y = p.y - size.top() - top;
                    if x < 0. || y < 0. || y >= row * LANES as f32 {
                        return None;
                    }
                    Some((
                        (y / row) as usize,
                        (f64::from(x / col) / snap).round() * snap,
                    ))
                };
                if response.drag_started() {
                    if let Some(pos) = pointer {
                        if let Some((lane, step)) = locate(pos) {
                            if let Some(hit) = under {
                                if !self.selected_hits.contains(&hit) {
                                    self.selected_hits = vec![hit];
                                }
                                self.checkpoint();
                                let originals = self
                                    .selected_hits
                                    .iter()
                                    .map(|i| {
                                        (
                                            *i,
                                            self.project.pattern().events[*i].step,
                                            self.project.pattern().events[*i].lane.clone(),
                                        )
                                    })
                                    .collect();
                                self.drag = Some((step, lane, originals));
                            } else if self.tool == 1 {
                                self.marquee = Some(pos);
                            }
                        }
                    }
                }
                if response.dragged() {
                    if let Some(pos) = pointer {
                        if let Some((lane, step)) = locate(pos) {
                            if let Some((start, origin, originals)) = &self.drag {
                                let delta = step - *start;
                                let shift = lane as isize - *origin as isize;
                                let min = originals
                                    .iter()
                                    .map(|(_, s, _)| *s)
                                    .fold(f64::INFINITY, f64::min);
                                let max = originals.iter().map(|(_, s, _)| *s).fold(0., f64::max);
                                let delta = delta.clamp(-min, (length - 0.00001 - max).max(-min));
                                for (index, old, id) in originals {
                                    let old_lane = self.content.lane_index(id).unwrap();
                                    let target = (old_lane as isize + shift)
                                        .clamp(0, LANES as isize - 1)
                                        as usize;
                                    let name = self.content.library.lanes[target].id.clone();
                                    let hit = &mut self.project.pattern_mut().events[*index];
                                    hit.step = *old + delta;
                                    hit.lane = name;
                                }
                                self.changed();
                            }
                        }
                        if let Some(start) = self.marquee {
                            let r = Rect::from_two_pos(start, pos);
                            painter.rect_stroke(
                                r,
                                0.,
                                Stroke::new(1., TEAL),
                                egui::StrokeKind::Inside,
                            );
                            self.selected_hits = self
                                .project
                                .pattern()
                                .events
                                .iter()
                                .enumerate()
                                .filter_map(|(i, h)| {
                                    let lane = self.content.lane_index(&h.lane).unwrap();
                                    let p = Pos2::new(
                                        size.left() + left + h.step as f32 * col,
                                        size.top() + top + (lane as f32 + 0.5) * row,
                                    );
                                    r.contains(p).then_some(i)
                                })
                                .collect();
                        }
                    }
                }
                if response.drag_stopped() {
                    self.drag = None;
                    self.marquee = None;
                }
                if response.clicked() {
                    if let Some(pos) = pointer {
                        if pos.x < size.left() + left {
                            let lane = ((pos.y - size.top() - top) / row).floor() as isize;
                            if (0..LANES as isize).contains(&lane) {
                                self.audition(lane as usize, false);
                            }
                        } else if let Some((lane, step)) = locate(pos) {
                            self.selected_lane = lane;
                            if let Some(index) = under {
                                if self.tool == 2 {
                                    self.checkpoint();
                                    self.project.pattern_mut().events.remove(index);
                                    self.selected_hits.clear();
                                    self.changed();
                                } else if ui.input(|i| i.modifiers.shift) {
                                    if self.selected_hits.contains(&index) {
                                        self.selected_hits.retain(|n| *n != index);
                                    } else {
                                        self.selected_hits.push(index);
                                    }
                                } else {
                                    self.selected_hits = vec![index];
                                }
                            } else if self.tool == 0
                                && step < length
                                && self.project.pattern().events.len() < 8192
                            {
                                self.checkpoint();
                                let id = self.content.library.lanes[lane].id.clone();
                                let index = self.project.pattern().events.len();
                                self.project.pattern_mut().events.push(Hit {
                                    lane: id,
                                    step,
                                    velocity: 100,
                                    ..Default::default()
                                });
                                self.selected_hits = vec![index];
                                self.audition(lane, false);
                                self.changed();
                            } else {
                                self.selected_hits.clear();
                            }
                        }
                    }
                }
                if response.secondary_clicked() {
                    if let Some(i) = under {
                        self.checkpoint();
                        self.project.pattern_mut().events.remove(i);
                        self.selected_hits.clear();
                        self.changed();
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.small(format!(
                "{} hits · {} selected",
                self.project.pattern().events.len(),
                self.selected_hits.len()
            ));
            if ui.button("Quantize").clicked() {
                self.checkpoint();
                let snap = self.snap;
                let selected = self.selected_hits.clone();
                for (i, h) in self.project.pattern_mut().events.iter_mut().enumerate() {
                    if selected.is_empty() || selected.contains(&i) {
                        h.step = (h.step / snap).round() * snap;
                        h.step = h.step.min(length - 0.00001);
                    }
                }
                self.changed();
            }
            if ui.button("Ghost").clicked() {
                self.set_velocity(42);
            }
            if ui.button("Flam").clicked() {
                self.add_repeats(2, 0.025 * self.project.bpm / 15.);
            }
            if ui.button("Roll").clicked() {
                self.add_repeats(4, 0.5);
            }
            if ui.button("Delete").clicked() {
                self.delete_selection();
            }
            if let Some(&i) = self.selected_hits.first() {
                if i < self.project.pattern().events.len() {
                    let mut velocity = self.project.pattern().events[i].velocity;
                    if ui
                        .add(egui::Slider::new(&mut velocity, 1..=127).text("Velocity"))
                        .changed()
                    {
                        self.set_velocity(velocity);
                    }
                    let mut probability = self.project.pattern().events[i].prob;
                    if ui
                        .add(
                            egui::DragValue::new(&mut probability)
                                .range(0. ..=1.)
                                .speed(0.01)
                                .prefix("Chance "),
                        )
                        .changed()
                    {
                        for j in self.selected_hits.clone() {
                            self.project.pattern_mut().events[j].prob = probability;
                        }
                        self.changed();
                    }
                }
            }
        });
    }
    fn set_velocity(&mut self, v: u8) {
        for i in self.selected_hits.clone() {
            if let Some(h) = self.project.pattern_mut().events.get_mut(i) {
                h.velocity = v;
            }
        }
        self.changed();
    }
    fn delete_selection(&mut self) {
        if self.selected_hits.is_empty() {
            return;
        }
        self.checkpoint();
        let selected = self.selected_hits.clone();
        let mut i = 0;
        self.project.pattern_mut().events.retain(|_| {
            let keep = !selected.contains(&i);
            i += 1;
            keep
        });
        self.selected_hits.clear();
        self.changed();
    }
    fn add_repeats(&mut self, count: usize, spacing: f64) {
        self.checkpoint();
        let selected: Vec<_> = self
            .selected_hits
            .iter()
            .filter_map(|i| self.project.pattern().events.get(*i).cloned())
            .collect();
        let length = self.project.pattern().length();
        for h in selected {
            for n in 1..count {
                let mut hit = h.clone();
                hit.step += n as f64 * spacing;
                if hit.step < length && self.project.pattern().events.len() < 8192 {
                    self.project.pattern_mut().events.push(hit);
                }
            }
        }
        self.changed();
    }
    fn song(&mut self, ui: &mut egui::Ui) {
        ui.heading("Song arrangement");
        ui.label("Add clips from the drum editor. Every clip refers to its source pattern.");
        let mut action = None;
        for i in 0..self.project.song.len() {
            let clip = self.project.song[i].clone();
            let name = self.project.patterns[clip.pattern].name.clone();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{:02}", i + 1)).color(ORANGE));
                ui.label(name);
                let mut repeats = clip.repeats;
                if ui
                    .add(
                        egui::DragValue::new(&mut repeats)
                            .range(1..=32)
                            .prefix("Repeat × "),
                    )
                    .changed()
                {
                    self.checkpoint();
                    self.project.song[i].repeats = repeats;
                    self.changed();
                }
                if ui.button("Edit").clicked() {
                    self.project.selected = clip.pattern;
                    self.tab = 1;
                    self.selected_hits.clear();
                    self.changed();
                }
                if ui.add_enabled(i > 0, egui::Button::new("↑")).clicked() {
                    action = Some((i, 0));
                }
                if ui
                    .add_enabled(i + 1 < self.project.song.len(), egui::Button::new("↓"))
                    .clicked()
                {
                    action = Some((i, 1));
                }
                if ui.button("Remove").clicked() {
                    action = Some((i, 2));
                }
            });
        }
        if let Some((i, a)) = action {
            self.checkpoint();
            match a {
                0 => self.project.song.swap(i, i - 1),
                1 => self.project.song.swap(i, i + 1),
                _ => {
                    self.project.song.remove(i);
                }
            }
            self.changed();
        }
        if self.project.song.is_empty() {
            ui.add_space(35.);
            ui.label("Your song starts with a pattern. Open Drum editor and choose + Song clip.");
        }
    }
    fn mixer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("MIXER").color(ORANGE).strong());
            egui::ComboBox::from_id_salt("mixpreset")
                .selected_text(MIX_NAMES[self.mix_name])
                .show_ui(ui, |ui| {
                    for (i, name) in MIX_NAMES.iter().enumerate() {
                        if ui.selectable_label(self.mix_name == i, *name).clicked() {
                            self.checkpoint();
                            self.mix_name = i;
                            self.project.mix_preset(i);
                            self.sound_changed();
                        }
                    }
                });
            ui.label("Room");
            if ui
                .add(egui::Slider::new(&mut self.project.room, 0. ..=1.).show_value(false))
                .changed()
            {
                self.changed();
            }
            ui.label("Parallel");
            if ui
                .add(egui::Slider::new(&mut self.project.parallel, 0. ..=1.).show_value(false))
                .changed()
            {
                self.changed();
            }
            ui.label("Master");
            if ui
                .add(egui::Slider::new(&mut self.project.master, 0. ..=1.5).show_value(false))
                .changed()
            {
                self.changed();
            }
        });
        let width = ((ui.available_width() - 10. * 8.) / 11.).max(62.);
        egui::ScrollArea::horizontal()
            .id_salt("mixerscroll")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..CHANNELS {
                        let name = self.content.library.channels[i].name.clone();
                        let selected = self.selected_channel == i;
                        egui::Frame::new()
                            .fill(if selected {
                                Color32::from_rgb(47, 40, 35)
                            } else {
                                PANEL
                            })
                            .inner_margin(7.)
                            .show(ui, |ui| {
                                ui.set_width(width - 14.);
                                ui.vertical_centered(|ui| {
                                    if ui
                                        .selectable_label(selected, RichText::new(name).size(11.))
                                        .clicked()
                                    {
                                        self.selected_channel = i;
                                    }
                                    ui.horizontal(|ui| {
                                        if ui
                                            .toggle_value(&mut self.project.channels[i].mute, "M")
                                            .changed()
                                        {
                                            self.changed();
                                        }
                                        if ui
                                            .toggle_value(&mut self.project.channels[i].solo, "S")
                                            .changed()
                                        {
                                            self.changed();
                                        }
                                    });
                                    let pan = ui.add(
                                        egui::Slider::new(
                                            &mut self.project.channels[i].pan,
                                            -1. ..=1.,
                                        )
                                        .show_value(false),
                                    );
                                    if pan.changed() {
                                        self.changed();
                                    }
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut self.project.channels[i].volume,
                                                    0. ..=1.5,
                                                )
                                                .vertical()
                                                .show_value(false),
                                            )
                                            .changed()
                                        {
                                            self.changed();
                                        }
                                        meter_ui(ui, self.peaks[i], 9., 82.);
                                    });
                                    let gain = self.project.channels[i].volume;
                                    ui.small(if gain <= 0. {
                                        "−∞ dB".into()
                                    } else {
                                        format!("{:.1} dB", 20. * gain.log10())
                                    });
                                });
                            });
                    }
                });
            });
    }
    fn channel_fx(&mut self, ui: &mut egui::Ui) {
        let i = self.selected_channel;
        ui.heading(format!(
            "{} · channel processing",
            self.content.library.channels[i].name
        ));
        ui.label("Levels, pan, mute, solo and sends update live. Apply sound edits to update EQ, compression, tuning or decay.");
        let before = self.project.channels[i];
        egui::Grid::new("fx")
            .num_columns(4)
            .spacing([20., 12.])
            .show(ui, |ui| {
                let s = &mut self.project.channels[i];
                ui.label("High-pass");
                ui.add(
                    egui::Slider::new(&mut s.hpf, 20. ..=2000.)
                        .logarithmic(true)
                        .suffix(" Hz"),
                );
                ui.label("Low shelf");
                ui.add(egui::Slider::new(&mut s.low, -18. ..=18.).suffix(" dB"));
                ui.end_row();
                ui.label("Mid EQ · 1.5 kHz");
                ui.add(egui::Slider::new(&mut s.mid, -18. ..=18.).suffix(" dB"));
                ui.label("High shelf");
                ui.add(egui::Slider::new(&mut s.high, -18. ..=18.).suffix(" dB"));
                ui.end_row();
                ui.label("Tuning");
                ui.add(egui::Slider::new(&mut s.tune, -12. ..=12.).suffix(" st"));
                ui.label("Decay");
                ui.add(egui::Slider::new(&mut s.decay, 0.05..=1.));
                ui.end_row();
                ui.label("Threshold");
                ui.add(egui::Slider::new(&mut s.threshold, -60. ..=0.).suffix(" dB"));
                ui.label("Ratio");
                ui.add(egui::Slider::new(&mut s.ratio, 1. ..=20.));
                ui.end_row();
                ui.label("Attack");
                ui.add(egui::Slider::new(&mut s.attack, 0.1..=100.).suffix(" ms"));
                ui.label("Release");
                ui.add(egui::Slider::new(&mut s.release, 10. ..=1000.).suffix(" ms"));
                ui.end_row();
                ui.label("Makeup");
                ui.add(egui::Slider::new(&mut s.makeup, 0. ..=18.).suffix(" dB"));
                ui.label("Room send");
                ui.add(egui::Slider::new(&mut s.room, 0. ..=1.));
                ui.end_row();
                ui.checkbox(&mut s.polarity, "Invert polarity");
                ui.end_row();
            });
        if before != self.project.channels[i] {
            self.sound_changed();
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Room decay");
            if ui
                .add(egui::Slider::new(&mut self.project.room_decay, 0.1..=10.).suffix(" s"))
                .changed()
            {
                self.sound_changed();
            }
            if ui.button("Apply sound edits").clicked() && !self.no_audio {
                self.prepare_sound();
            }
        });
    }
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings;
        egui::Window::new("Audio / MIDI / Recovery").open(&mut open).default_width(520.).show(ctx,|ui|{
            ui.heading("Audio output");let label=self.device.and_then(|i|self.devices.get(i)).map_or("System default",String::as_str).to_string();
            egui::ComboBox::from_id_salt("device").selected_text(label).show_ui(ui,|ui|{ui.selectable_value(&mut self.device,None,"System default");for(i,name)in self.devices.iter().enumerate(){ui.selectable_value(&mut self.device,Some(i),name);}});
            ui.heading("MIDI input");let label=self.midi.and_then(|i|self.ports.get(i)).map_or("None",String::as_str).to_string();
            egui::ComboBox::from_id_salt("midiport").selected_text(label).show_ui(ui,|ui|{ui.selectable_value(&mut self.midi,None,"None");for(i,name)in self.ports.iter().enumerate(){ui.selectable_value(&mut self.midi,Some(i),name);}});
            if ui.button("Refresh devices").clicked(){self.devices=OutputHost::devices().unwrap_or_default();self.ports=soniclaw_midi::native::input_ports().unwrap_or_default();}
            ui.label("The device's default sample rate is negotiated by Soniclaw. Internal blocks are bounded to 512 frames.");
            if ui.button("Connect / restart audio").clicked(){self.no_audio=false;self.prepare_sound();}
            ui.separator();ui.label("MIDI channel · 0 accepts all");if ui.add(egui::DragValue::new(&mut self.project.midi_channel).range(0..=16)).changed(){self.changed();}
            if ui.checkbox(&mut self.project.pedal_closed_high,"CC4: high values close the hi-hat").changed(){self.changed();}
            egui::ScrollArea::vertical().max_height(240.).show(ui,|ui|{for i in 0..LANES{ui.horizontal(|ui|{ui.label(&self.content.library.lanes[i].name);if ui.add(egui::DragValue::new(&mut self.project.midi[i]).range(0..=127)).changed(){self.changed();}});}});
            ui.separator();
            if ui.add_enabled(self.autosave.is_file(),egui::Button::new("Restore latest autosave")).clicked(){self.open(Some(self.autosave.clone()));}
            ui.small("Factory-only edits autosave every 15 seconds. Save projects containing custom recordings explicitly.");
            ui.small("Hardware latency, macOS and Windows audio require testing on the target machine.");
        });
        self.settings = open;
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let command = ctx.input(|i| i.modifiers.command);
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.play_stop();
        }
        if command {
            if ctx.input(|i| i.key_pressed(egui::Key::S)) {
                self.save(ctx.input(|i| i.modifiers.shift));
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Z)) {
                self.undo(ctx.input(|i| i.modifiers.shift));
            }
            if self.tab == 1 {
                if ctx.input(|i| i.key_pressed(egui::Key::A)) {
                    self.selected_hits = (0..self.project.pattern().events.len()).collect();
                }
                if ctx.input(|i| i.key_pressed(egui::Key::C)) {
                    self.clipboard = self
                        .selected_hits
                        .iter()
                        .filter_map(|i| self.project.pattern().events.get(*i).cloned())
                        .collect();
                }
                if ctx.input(|i| i.key_pressed(egui::Key::V)) && !self.clipboard.is_empty() {
                    self.checkpoint();
                    let offset = self
                        .clipboard
                        .iter()
                        .map(|h| h.step)
                        .fold(f64::INFINITY, f64::min);
                    let length = self.project.pattern().length();
                    self.selected_hits.clear();
                    for mut h in self.clipboard.clone() {
                        h.step = (h.step - offset + 4.).min(length - 0.00001);
                        if self.project.pattern().events.len() < 8192 {
                            self.selected_hits.push(self.project.pattern().events.len());
                            self.project.pattern_mut().events.push(h);
                        }
                    }
                    self.changed();
                }
            }
            return;
        }
        if self.tab == 1
            && ctx
                .input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            self.delete_selection();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::X)) {
            if let Some(a) = &self.audio {
                a.control.shared.panic.store(true, Ordering::Release);
            }
        }
        let keys = [
            egui::Key::B,
            egui::Key::F,
            egui::Key::D,
            egui::Key::G,
            egui::Key::H,
            egui::Key::J,
            egui::Key::K,
            egui::Key::L,
            egui::Key::R,
            egui::Key::T,
            egui::Key::Q,
            egui::Key::W,
            egui::Key::E,
            egui::Key::Y,
            egui::Key::U,
            egui::Key::I,
            egui::Key::O,
        ];
        for (i, key) in keys.iter().enumerate() {
            if ctx.input(|input| input.key_pressed(*key)) {
                self.audition(i, ctx.input(|i| i.modifiers.shift));
            }
        }
    }
}
impl eframe::App for IronRoom {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(33));
        self.tick();
        self.shortcuts(&ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "ironroom-s") {
                self.open(Some(path.to_path_buf()));
            }
        }
        egui::Panel::top("transport").show(root, |ui| self.header(ui));
        egui::Panel::bottom("footer").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.small(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small("SONICLAW 0.144 · NATIVE RUST");
                    meter_ui(ui, self.master_peak, 90., 8.);
                });
            });
            if let Some(job) = &self.job {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::ProgressBar::new(job.progress.load(Ordering::Relaxed) as f32 / 1000.)
                            .desired_width(260.),
                    );
                    if ui.button("Cancel export").clicked() {
                        job.cancel.store(true, Ordering::Release);
                    }
                });
            }
        });
        egui::Panel::bottom("mixer")
            .resizable(true)
            .default_size(213.)
            .min_size(195.)
            .show(root, |ui| self.mixer(ui));
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(245.)
            .min_size(210.)
            .show(root, |ui| self.browser(ui));
        egui::CentralPanel::default().show(root, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in ["Kit", "Drum editor", "Song", "Channel FX"]
                    .iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut self.tab, i, *name);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(format!(
                        "{} samples · 17 articulations",
                        self.content.sample_count
                    ));
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("maincontent")
                .show(ui, |ui| match self.tab {
                    0 => self.kit(ui),
                    1 => self.editor(ui),
                    2 => self.song(ui),
                    _ => self.channel_fx(ui),
                });
        });
        if self.settings {
            self.settings(&ctx);
        }
        self.smoke_frames += 1;
        if self.ui_smoke.is_some() && self.smoke_frames == 12 {
            self.tab = 1;
        }
        if self.ui_smoke.is_some() && self.smoke_frames == 35 {
            if let Some(audio) = &self.audio {
                audio.control.play();
            }
        }
        if let Some(path) = &self.ui_smoke {
            if self.smoke_frames == 120 {
                let receipt = serde_json::json!({"frames":self.smoke_frames,"tab":"drum editor","grooves":self.content.library.grooves.len(),"samples":self.content.sample_count,"pattern_hits":self.project.pattern().events.len(),"project_valid":self.project.validate(&self.content).is_ok(),"backend":"eframe native glow","audio_connected":self.audio.is_some(),"audio_peak":self.audio.as_ref().map(|a|a.host.telemetry.take_peak()),"stream_errors":self.audio.as_ref().map(|a|a.host.telemetry.stream_errors.load(Ordering::Relaxed)),"process_errors":self.audio.as_ref().map(|a|a.host.telemetry.process_errors.load(Ordering::Relaxed))});
                let _ = std::fs::write(path, receipt.to_string());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
fn meter_ui(ui: &mut egui::Ui, peak: f32, width: f32, height: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter()
        .rect_filled(r, 1., Color32::from_rgb(16, 19, 22));
    let p = if peak <= 0. {
        0.
    } else {
        ((20. * peak.log10() + 60.) / 60.).clamp(0., 1.)
    };
    let fill = if height > width {
        Rect::from_min_max(
            Pos2::new(r.left(), r.bottom() - r.height() * p),
            r.right_bottom(),
        )
    } else {
        Rect::from_min_size(r.min, Vec2::new(r.width() * p, r.height()))
    };
    ui.painter()
        .rect_filled(fill, 1., if peak > 0.95 { ORANGE } else { TEAL });
}
fn trim_history(history: &mut Vec<Project>) {
    let bytes = |p: &Project| {
        p.custom
            .iter()
            .flatten()
            .map(|s| s.data.len())
            .sum::<usize>()
    };
    let mut encoded = history.iter().map(bytes).sum::<usize>();
    while history.len() > 48 || (encoded > 128 * 1024 * 1024 && history.len() > 1) {
        encoded = encoded.saturating_sub(bytes(&history.remove(0)));
    }
}

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("IronRoomSoniclaw")
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Library/Application Support/IronRoomSoniclaw")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir)
                    .join(".local/share")
            })
            .join("iron-room-soniclaw")
    }
}
