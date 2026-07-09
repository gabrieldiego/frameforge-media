use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use frameforge_core::{PixelFormat, VERSION};

use crate::args::{self, Command, EncodeArgs};
use crate::catalog::{
    self, setting_values_label, settings_label, StageInfo, StageKind, CODECS, FILTERS,
    GLOBAL_SETTINGS,
};

pub fn run<I>(raw_args: I) -> ExitCode
where
    I: IntoIterator<Item = OsString>,
{
    match args::parse_os(raw_args) {
        Ok(Command::Help) => {
            print_help();
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("ff {VERSION}");
            ExitCode::SUCCESS
        }
        Ok(Command::Codecs) => {
            print_stage_table("Codecs", CODECS);
            ExitCode::SUCCESS
        }
        Ok(Command::Filters) => {
            print_stage_table("Filters", FILTERS);
            ExitCode::SUCCESS
        }
        Ok(Command::Encode(args)) => run_encode(args),
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("run 'ff --help' for usage");
            ExitCode::from(2)
        }
    }
}

fn run_encode(args: EncodeArgs) -> ExitCode {
    let codec_name = args.codec.as_deref().expect("encode parser requires codec");
    let Some(codec) = catalog::codec(codec_name) else {
        eprintln!("error: unknown codec '{codec_name}'");
        eprintln!("run 'ff codecs' to list known codec stages");
        return ExitCode::from(2);
    };

    if let Some(exit) = validate_codec_settings(codec, &args.settings) {
        return exit;
    }

    if !codec.compiled {
        eprintln!(
            "error: codec '{codec_name}' is not compiled into this binary; rebuild with CARGO_FEATURES=\"{}\"",
            codec.feature
        );
        return ExitCode::from(3);
    }

    if let Some(exit) = validate_filters(&args.filters) {
        return exit;
    }

    match encode_with_model(codec.name, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

fn validate_codec_settings(codec: StageInfo, settings: &[String]) -> Option<ExitCode> {
    for spec in settings {
        let name = args::setting_name(spec);
        let Some(setting) = catalog::global_setting(name).or_else(|| codec.setting(name)) else {
            eprintln!("error: unknown encode setting '{name}'");
            eprintln!(
                "accepted settings: {}",
                settings_label(GLOBAL_SETTINGS, codec.settings)
            );
            return Some(ExitCode::from(2));
        };
        let value = args::setting_value(spec).unwrap_or("true");
        if !setting.value.accepts(value) {
            eprintln!(
                "error: codec '{}' setting '{}' expects one of {}, got '{}'",
                codec.name,
                setting.name,
                setting_values_label(setting),
                value
            );
            return Some(ExitCode::from(2));
        }
    }
    None
}

fn validate_filters(filters: &[String]) -> Option<ExitCode> {
    for filter_name in args::filter_names(filters) {
        let Some(filter) = catalog::filter(filter_name) else {
            eprintln!("error: unknown filter '{filter_name}'");
            eprintln!("run 'ff filters' to list known filter stages");
            return Some(ExitCode::from(2));
        };
        if !filter.compiled {
            eprintln!(
                "error: filter '{filter_name}' is not compiled into this binary; rebuild with CARGO_FEATURES=\"{}\"",
                filter.feature
            );
            return Some(ExitCode::from(3));
        }
    }
    if !filters.is_empty() {
        eprintln!("error: filters are parsed but execution is not implemented yet");
        return Some(ExitCode::from(4));
    }
    None
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(any(feature = "codec-av2", feature = "codec-vvc")),
    allow(dead_code)
)]
struct EncodeJob {
    input: PathBuf,
    output: PathBuf,
    frames: usize,
    width: usize,
    height: usize,
    format: PixelFormat,
}

fn encode_with_model(codec_name: &str, args: &EncodeArgs) -> Result<(), String> {
    let job = encode_job(args)?;
    match codec_name {
        "av2" => encode_av2(job),
        "vvc" => encode_vvc(job),
        other => Err(format!("codec '{other}' has no encode model wired yet")),
    }
}

fn encode_job(args: &EncodeArgs) -> Result<EncodeJob, String> {
    let input = PathBuf::from(args.input.as_deref().expect("parser requires input"));
    let output = PathBuf::from(args.output.as_deref().expect("parser requires output"));
    let video = args
        .video
        .as_ref()
        .ok_or_else(|| "encode requires --video WxH:pixfmt or filename metadata".to_string())?;
    let format = video
        .pixel_format
        .as_deref()
        .ok_or_else(|| {
            "encode requires a pixel format in --video or the input filename".to_string()
        })?
        .parse::<PixelFormat>()?;
    Ok(EncodeJob {
        input,
        output,
        frames: args.frames.unwrap_or(1) as usize,
        width: video.width as usize,
        height: video.height as usize,
        format,
    })
}

#[cfg(feature = "codec-av2")]
fn encode_av2(job: EncodeJob) -> Result<(), String> {
    let request = frameforge_codecs::av2::Av2EncodeRequest {
        params: frameforge_codecs::av2::Av2EncodeParams { frames: job.frames },
        geometry: frameforge_codecs::av2::Av2VideoGeometry {
            width: job.width,
            height: job.height,
        },
        format: job.format,
    };
    let mut input = open_reader(&job.input)?;
    let mut output = create_writer(&job.output)?;
    frameforge_codecs::av2::av2_encode_fixed_black_444(&mut input, &mut output, None, request)?;
    flush_writer(&job.output, &mut output)
}

#[cfg(not(feature = "codec-av2"))]
fn encode_av2(_job: EncodeJob) -> Result<(), String> {
    Err("AV2 support is not compiled into this binary".to_string())
}

#[cfg(feature = "codec-vvc")]
fn encode_vvc(job: EncodeJob) -> Result<(), String> {
    if !job.format.is_yuv() {
        return Err(format!(
            "VVC encoder expects planar YUV input; got {}x{} {}",
            job.width, job.height, job.format
        ));
    }

    let params = frameforge_codecs::vvc::VvcEncodeParams { frames: job.frames };
    let geometry = frameforge_codecs::vvc::VvcVideoGeometry {
        width: job.width,
        height: job.height,
    };
    let limits = frameforge_codecs::vvc::VvcVideoLimits::unbounded();
    geometry.validate_against(limits)?;
    let mut input = open_reader(&job.input)?;
    let mut output = create_writer(&job.output)?;
    frameforge_codecs::vvc::vvc_yuv_encode_stream_with_limits(
        &mut input,
        &mut output,
        None,
        params,
        geometry,
        limits,
        job.format,
    )?;
    flush_writer(&job.output, &mut output)
}

#[cfg(not(feature = "codec-vvc"))]
fn encode_vvc(_job: EncodeJob) -> Result<(), String> {
    Err("VVC support is not compiled into this binary".to_string())
}

#[cfg_attr(
    not(any(feature = "codec-av2", feature = "codec-vvc")),
    allow(dead_code)
)]
fn open_reader(path: &Path) -> Result<BufReader<File>, String> {
    let file = File::open(path)
        .map_err(|err| format!("failed to open input '{}': {err}", path.display()))?;
    Ok(BufReader::new(file))
}

#[cfg_attr(
    not(any(feature = "codec-av2", feature = "codec-vvc")),
    allow(dead_code)
)]
fn create_writer(path: &Path) -> Result<BufWriter<File>, String> {
    let file = File::create(path)
        .map_err(|err| format!("failed to create output '{}': {err}", path.display()))?;
    Ok(BufWriter::new(file))
}

#[cfg_attr(
    not(any(feature = "codec-av2", feature = "codec-vvc")),
    allow(dead_code)
)]
fn flush_writer(path: &Path, writer: &mut BufWriter<File>) -> Result<(), String> {
    writer
        .flush()
        .map_err(|err| format!("failed to flush output '{}': {err}", path.display()))
}

fn print_help() {
    print!("{}", args::help(VERSION));
}

fn print_stage_table(title: &str, stages: &[StageInfo]) {
    println!("{title}:");
    println!(
        "{:<12} {:<8} {:<16} {:<12} Summary",
        "Name", "Kind", "Feature", "Status"
    );
    for stage in stages {
        println!(
            "{:<12} {:<8} {:<16} {:<12} {}",
            stage.name,
            kind_name(stage.kind),
            stage.feature,
            stage.build_status(),
            stage.summary
        );
    }

    if stages.iter().any(|stage| !stage.settings.is_empty()) {
        println!();
        println!("Codec-specific settings:");
        let mut printed = Vec::new();
        for stage in stages {
            for setting in stage.settings {
                if printed.contains(&setting.name) {
                    continue;
                }
                printed.push(setting.name);
                println!(
                    "  {} ({}) - {}",
                    setting.name,
                    setting_values_label(*setting),
                    setting.summary
                );
            }
        }
    }

    if stages.iter().any(|stage| stage.kind == StageKind::Codec) && !GLOBAL_SETTINGS.is_empty() {
        println!();
        println!("Accepted settings:");
        for setting in GLOBAL_SETTINGS {
            println!(
                "  {} ({}) - {}",
                setting.name,
                setting_values_label(*setting),
                setting.summary
            );
        }
    }
}

fn kind_name(kind: StageKind) -> &'static str {
    match kind {
        StageKind::Codec => "codec",
        StageKind::Filter => "filter",
    }
}
