#![forbid(unsafe_code)]
use iron_room_soniclaw::{
    content::{find_assets, Content},
    model::{Pattern, Project},
    Result,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU32},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("Iron Room: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let value = |flag: &str| -> Result<Option<String>> {
        args.iter()
            .position(|a| a == flag)
            .map(|i| {
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("Missing value for {flag}").into())
            })
            .transpose()
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Iron Room Soniclaw 0.1.0\n\nNative acoustic drum studio.\n\n  --assets PATH       Factory asset folder\n  --project FILE      Open an .ironroom-s project\n  --groove INDEX      Select factory groove (0..2374)\n  --render FILE.wav   Offline 24-bit stereo WAV export\n  --midi FILE.mid     Export standard MIDI\n  --rate HZ           WAV rate (default 44100)\n  --list-grooves      Print factory groove catalog\n  --inspect           Print content information\n  --no-audio          Edit and export without opening audio\n  --ui-smoke FILE     Native UI smoke receipt and automatic close\n\nKeep assets beside the executable. Exports refuse to overwrite existing files.");
        return Ok(());
    }
    let root = find_assets(value("--assets")?.map(PathBuf::from))?;
    let content = Content::load(&root)?;
    if args.iter().any(|a| a == "--inspect") {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"samples":content.sample_count,"articulations":content.library.lanes.len(),"channels":content.library.channels.len(),"grooves":content.library.grooves.len(),"decoded_pcm_bytes":content.pcm_bytes,"framework":"Soniclaw 0.144.0","native":true})
            )?
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-grooves") {
        for (i, g) in content.library.grooves.iter().enumerate() {
            println!("{i}\t{}\t{}\t{}", g.name, g.family, g.signature);
        }
        return Ok(());
    }
    let project_path = value("--project")?.map(PathBuf::from);
    let mut project = if let Some(path) = &project_path {
        Project::open(path, &content)?
    } else {
        Project::factory(&content)
    };
    if let Some(index) = value("--groove")? {
        let i: usize = index.parse()?;
        let g = content
            .library
            .grooves
            .get(i)
            .ok_or("Groove index out of range")?;
        project.patterns[project.selected] = Pattern::from_groove(g);
    }
    let render = value("--render")?;
    let midi = value("--midi")?;
    if render.is_some() || midi.is_some() {
        if let Some(path) = render {
            let rate = value("--rate")?
                .map(|s| s.parse::<u32>())
                .transpose()?
                .unwrap_or(44100);
            let frames = iron_room_soniclaw::export::wav(
                content.clone(),
                &project,
                &PathBuf::from(&path),
                rate,
                &AtomicBool::new(false),
                &AtomicU32::new(0),
            )?;
            println!("Rendered {frames} stereo frames to {path}");
        }
        if let Some(path) = midi {
            let hits = iron_room_soniclaw::export::midi(&content, &project, &PathBuf::from(&path))?;
            println!("Exported {hits} hits to {path}");
        }
        return Ok(());
    }
    #[cfg(feature = "desktop")]
    {
        let no_audio = args.iter().any(|a| a == "--no-audio");
        let smoke = value("--ui-smoke")?.map(PathBuf::from);
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1420., 940.])
                .with_min_inner_size([1100., 780.]),
            ..Default::default()
        };
        eframe::run_native(
            "Iron Room · Soniclaw",
            options,
            Box::new(move |cc| {
                Ok(Box::new(iron_room_soniclaw::app::IronRoom::new(
                    cc, content, project, no_audio, smoke,
                )))
            }),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        Err("This build supports headless rendering. Build with default features for the desktop app.".into())
    }
}
