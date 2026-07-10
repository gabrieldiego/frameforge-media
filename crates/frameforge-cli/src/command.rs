use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use frameforge_core::{
    convert_planar_frame_bit_depth, ChromaSampling, PixelFormat, SampleBitDepth, VERSION,
};

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

    if let Some(exit) = validate_filters(&args) {
        return exit;
    }

    let job = match encode_job(&args) {
        Ok(job) => job,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };

    print_encode_config(codec.name, &args, &job);

    match encode_with_model(codec.name, job) {
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

fn validate_filters(args: &EncodeArgs) -> Option<ExitCode> {
    let filters = &args.filters;
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
    if filters.is_empty() {
        return None;
    }
    if args.input.is_none() && is_pattern_source_pipeline(filters) {
        return None;
    }
    if args.input.is_none() {
        eprintln!("error: encode without an input requires a source filter such as --filter pattern=black");
        return Some(ExitCode::from(4));
    }
    eprintln!(
        "error: transform filters are parsed but execution is not implemented yet; only --filter pattern=<name> can source frames without input"
    );
    Some(ExitCode::from(4))
}

fn is_pattern_source_pipeline(filters: &[String]) -> bool {
    if filters.len() != 1 {
        return false;
    }
    args::filter_names(filters).next() == Some("pattern")
}

#[derive(Debug, Clone)]
enum EncodeInput {
    Path(PathBuf),
    Pattern(PatternSourceSpec),
}

#[derive(Debug, Clone)]
struct PatternSourceSpec {
    pattern: PatternKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternKind {
    Black,
    Checker,
    Gradient,
    ColorBlocks,
}

impl PatternSourceSpec {
    fn from_filter(spec: &str) -> Result<Self, String> {
        if args::filter_names(&[spec.to_string()]).next() != Some("pattern") {
            return Err("source filter must be pattern=<name>".to_string());
        }
        let Some((_, value)) = spec.split_once('=').or_else(|| spec.split_once(':')) else {
            return Err("pattern source expects --filter pattern=<name>".to_string());
        };
        let pattern = PatternKind::parse(value)?;
        Ok(Self { pattern })
    }
}

impl PatternKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "black" => Ok(Self::Black),
            "checker" => Ok(Self::Checker),
            "gradient" => Ok(Self::Gradient),
            "color_blocks" | "blocks" => Ok(Self::ColorBlocks),
            other => Err(format!(
                "unknown pattern source '{other}'; accepted patterns: black, checker, gradient, color_blocks"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Checker => "checker",
            Self::Gradient => "gradient",
            Self::ColorBlocks => "color_blocks",
        }
    }
}

fn input_source_filter(args: &EncodeArgs) -> Result<PatternSourceSpec, String> {
    if args.filters.is_empty() {
        return Err("encode requires an input path or source filter".to_string());
    }
    if !is_pattern_source_pipeline(&args.filters) {
        return Err(
            "encode without input currently supports only one source filter: --filter pattern=<name>"
                .to_string(),
        );
    }
    PatternSourceSpec::from_filter(&args.filters[0])
}

fn generated_pattern_input(job: &EncodeJob, source: &PatternSourceSpec) -> Result<Vec<u8>, String> {
    job.format
        .validate_geometry(job.width, job.height)
        .map_err(|err| err.to_string())?;
    match job.format {
        PixelFormat::Yuv420p8 => Ok(generate_yuv420p8(job, source.pattern)),
        PixelFormat::Yuv444p8 => Ok(generate_yuv444p8(job, source.pattern)),
        other => Err(format!(
            "pattern source currently supports yuv420p8 and yuv444p8; got {other}"
        )),
    }
}

fn generate_yuv420p8(job: &EncodeJob, pattern: PatternKind) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in 0..job.frames {
        let (y_plane, u444, v444) = render_pattern_frame(job.width, job.height, frame, pattern);
        let mut u_plane = Vec::with_capacity(job.width * job.height / 4);
        let mut v_plane = Vec::with_capacity(job.width * job.height / 4);
        for y in (0..job.height).step_by(2) {
            for x in (0..job.width).step_by(2) {
                let indices = (
                    y * job.width + x,
                    y * job.width + x + 1,
                    (y + 1) * job.width + x,
                    (y + 1) * job.width + x + 1,
                );
                u_plane.push(
                    ((u444[indices.0] as u16
                        + u444[indices.1] as u16
                        + u444[indices.2] as u16
                        + u444[indices.3] as u16)
                        / 4) as u8,
                );
                v_plane.push(
                    ((v444[indices.0] as u16
                        + v444[indices.1] as u16
                        + v444[indices.2] as u16
                        + v444[indices.3] as u16)
                        / 4) as u8,
                );
            }
        }
        out.extend(y_plane);
        out.extend(u_plane);
        out.extend(v_plane);
    }
    out
}

fn generate_yuv444p8(job: &EncodeJob, pattern: PatternKind) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in 0..job.frames {
        let (y_plane, u_plane, v_plane) =
            render_pattern_frame(job.width, job.height, frame, pattern);
        out.extend(y_plane);
        out.extend(u_plane);
        out.extend(v_plane);
    }
    out
}

fn render_pattern_frame(
    width: usize,
    height: usize,
    frame: usize,
    pattern: PatternKind,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y_plane = vec![0; width * height];
    let mut u_plane = vec![0; width * height];
    let mut v_plane = vec![0; width * height];
    for y in 0..height {
        for x in 0..width {
            let (yy, uu, vv) = pattern_sample(pattern, x, y, frame);
            let idx = y * width + x;
            y_plane[idx] = yy;
            u_plane[idx] = uu;
            v_plane[idx] = vv;
        }
    }
    (y_plane, u_plane, v_plane)
}

fn pattern_sample(pattern: PatternKind, x: usize, y: usize, frame: usize) -> (u8, u8, u8) {
    match pattern {
        PatternKind::Black => (0, 0, 0),
        PatternKind::Checker => {
            if ((x / 8) + (y / 8) + frame) & 1 == 0 {
                (208, 176, 80)
            } else {
                (48, 96, 160)
            }
        }
        PatternKind::Gradient => (
            ((x * 7 + y * 5 + frame * 17) & 0xFF) as u8,
            ((64 + x * 3 + frame * 11) & 0xFF) as u8,
            ((96 + y * 4 + frame * 13) & 0xFF) as u8,
        ),
        PatternKind::ColorBlocks => {
            const PALETTE: [(u8, u8, u8); 4] = [
                (32, 128, 128),
                (80, 96, 176),
                (144, 176, 96),
                (224, 112, 144),
            ];
            PALETTE[((x / 8) + (y / 8) * 2 + frame) % PALETTE.len()]
        }
    }
}

fn open_job_reader(job: &EncodeJob) -> Result<Box<dyn Read>, String> {
    match &job.input {
        EncodeInput::Path(path) => {
            let file = File::open(path)
                .map_err(|err| format!("failed to open input '{}': {err}", path.display()))?;
            let reader = BufReader::new(file).take(selected_input_byte_len(job)?);
            if job.source_format == job.format {
                Ok(Box::new(reader))
            } else {
                Ok(Box::new(BitDepthConvertingReader::new(reader, job)?))
            }
        }
        EncodeInput::Pattern(source) => {
            Ok(Box::new(Cursor::new(generated_pattern_input(job, source)?)))
        }
    }
}

fn selected_input_byte_len(job: &EncodeJob) -> Result<u64, String> {
    let frame_len = job
        .source_format
        .frame_len(job.width, job.height)
        .ok_or_else(|| {
            format!(
                "frame length overflow for {}x{}:{}",
                job.width, job.height, job.source_format
            )
        })?;
    let byte_len = frame_len
        .checked_mul(job.frames)
        .ok_or_else(|| "selected input byte length overflow".to_string())?;
    u64::try_from(byte_len).map_err(|_| "selected input byte length overflows u64".to_string())
}

struct BitDepthConvertingReader<R> {
    inner: R,
    width: usize,
    height: usize,
    source_format: PixelFormat,
    target_format: PixelFormat,
    source_frame: Vec<u8>,
    converted_frame: Vec<u8>,
    converted_offset: usize,
    frames_remaining: usize,
}

impl<R: Read> BitDepthConvertingReader<R> {
    fn new(inner: R, job: &EncodeJob) -> Result<Self, String> {
        let source_frame_len = job
            .source_format
            .frame_len(job.width, job.height)
            .ok_or_else(|| {
                format!(
                    "frame length overflow for {}x{}:{}",
                    job.width, job.height, job.source_format
                )
            })?;
        Ok(Self {
            inner,
            width: job.width,
            height: job.height,
            source_format: job.source_format,
            target_format: job.format,
            source_frame: vec![0; source_frame_len],
            converted_frame: Vec::new(),
            converted_offset: 0,
            frames_remaining: job.frames,
        })
    }

    fn fill_converted_frame(&mut self) -> std::io::Result<bool> {
        if self.frames_remaining == 0 {
            return Ok(false);
        }
        self.inner.read_exact(&mut self.source_frame)?;
        self.converted_frame = convert_planar_frame_bit_depth(
            &self.source_frame,
            self.width,
            self.height,
            self.source_format,
            self.target_format,
        )
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        self.converted_offset = 0;
        self.frames_remaining -= 1;
        Ok(true)
    }
}

impl<R: Read> Read for BitDepthConvertingReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.converted_offset >= self.converted_frame.len() && !self.fill_converted_frame()? {
            return Ok(0);
        }

        let remaining = &self.converted_frame[self.converted_offset..];
        let count = remaining.len().min(output.len());
        output[..count].copy_from_slice(&remaining[..count]);
        self.converted_offset += count;
        Ok(count)
    }
}

fn input_label(input: &EncodeInput) -> String {
    match input {
        EncodeInput::Path(path) => format!("path={}", path.display()),
        EncodeInput::Pattern(source) => format!("source=pattern:{}", source.pattern.name()),
    }
}

#[derive(Debug, Clone, Copy)]
struct FramePsnr {
    y: f64,
    u: f64,
    v: f64,
    all: f64,
}

fn print_frame_metrics(
    codec: &str,
    job: &EncodeJob,
    frame_idx: usize,
    frame_count: usize,
    bitstream_bytes: usize,
    source: &[u8],
    reconstruction: &[u8],
) {
    let bits = bitstream_bytes * 8;
    match frame_psnr(job, source, reconstruction) {
        Some(psnr) => eprintln!(
            "frame: codec={} index={}/{} bits={} bytes={} psnr_y={} psnr_u={} psnr_v={} psnr_all={}",
            codec,
            frame_idx + 1,
            frame_count,
            bits,
            bitstream_bytes,
            format_psnr(psnr.y),
            format_psnr(psnr.u),
            format_psnr(psnr.v),
            format_psnr(psnr.all),
        ),
        None => eprintln!(
            "frame: codec={} index={}/{} bits={} bytes={} psnr=n/a",
            codec,
            frame_idx + 1,
            frame_count,
            bits,
            bitstream_bytes,
        ),
    }
}

fn frame_psnr(job: &EncodeJob, source: &[u8], reconstruction: &[u8]) -> Option<FramePsnr> {
    let y_len = job.width.checked_mul(job.height)?;
    let chroma_len = match job.format {
        PixelFormat::Yuv420p8 => y_len / 4,
        PixelFormat::Yuv444p8 => y_len,
        _ => return None,
    };
    let frame_len = y_len.checked_add(chroma_len.checked_mul(2)?)?;
    if source.len() != frame_len || reconstruction.len() != frame_len {
        return None;
    }

    let y_src = &source[..y_len];
    let y_rec = &reconstruction[..y_len];
    let u_start = y_len;
    let v_start = y_len + chroma_len;
    let u_src = &source[u_start..v_start];
    let u_rec = &reconstruction[u_start..v_start];
    let v_src = &source[v_start..frame_len];
    let v_rec = &reconstruction[v_start..frame_len];

    let y_sse = sse_u8(y_src, y_rec);
    let u_sse = sse_u8(u_src, u_rec);
    let v_sse = sse_u8(v_src, v_rec);
    Some(FramePsnr {
        y: psnr_from_sse(y_sse, y_len),
        u: psnr_from_sse(u_sse, chroma_len),
        v: psnr_from_sse(v_sse, chroma_len),
        all: psnr_from_sse(y_sse + u_sse + v_sse, frame_len),
    })
}

fn sse_u8(source: &[u8], reconstruction: &[u8]) -> u64 {
    source
        .iter()
        .zip(reconstruction)
        .map(|(&src, &rec)| {
            let diff = src as i32 - rec as i32;
            (diff * diff) as u64
        })
        .sum()
}

fn psnr_from_sse(sse: u64, samples: usize) -> f64 {
    if sse == 0 {
        f64::INFINITY
    } else {
        10.0 * ((255.0 * 255.0 * samples as f64) / sse as f64).log10()
    }
}

fn format_psnr(value: f64) -> String {
    if value.is_infinite() {
        "inf".to_string()
    } else {
        format!("{value:.3}")
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(any(feature = "codec-av2", feature = "codec-vvc")),
    allow(dead_code)
)]
struct EncodeJob {
    input: EncodeInput,
    output: PathBuf,
    recon: Option<PathBuf>,
    frames: usize,
    width: usize,
    height: usize,
    source_format: PixelFormat,
    format: PixelFormat,
}

fn print_encode_config(codec_name: &str, args: &EncodeArgs, job: &EncodeJob) {
    let settings = if args.settings.is_empty() {
        "none".to_string()
    } else {
        args.settings.join(",")
    };
    eprintln!(
        "input: {} video={}x{}:{} frames={} fps={}",
        input_label(&job.input),
        job.width,
        job.height,
        job.source_format,
        job.frames,
        args.fps.as_deref().unwrap_or("unspecified")
    );
    if job.source_format != job.format {
        eprintln!(
            "input-convert: bit_depth {} -> {}",
            job.source_format, job.format
        );
    }
    for filter in &args.filters {
        eprintln!("filter: {filter}");
    }
    eprintln!(
        "encoder: codec={} output={} recon={} settings={} preset={}",
        codec_name,
        job.output.display(),
        job.recon
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string()),
        settings,
        args.preset.as_deref().unwrap_or("default")
    );
}

fn encode_with_model(codec_name: &str, job: EncodeJob) -> Result<(), String> {
    match codec_name {
        "av2" => encode_av2(job),
        "vvc" => encode_vvc(job),
        other => Err(format!("codec '{other}' has no encode model wired yet")),
    }
}

fn encode_job(args: &EncodeArgs) -> Result<EncodeJob, String> {
    let input = match args.input.as_deref() {
        Some(path) => EncodeInput::Path(PathBuf::from(path)),
        None => EncodeInput::Pattern(input_source_filter(args)?),
    };
    let output = PathBuf::from(args.output.as_deref().expect("parser requires output"));
    let recon = args.recon.as_deref().map(PathBuf::from);
    let video = args
        .video
        .as_ref()
        .ok_or_else(|| "encode requires --video WxH:pixfmt or filename metadata".to_string())?;
    let source_format = video
        .pixel_format
        .as_deref()
        .ok_or_else(|| {
            "encode requires a pixel format in --video or the input filename".to_string()
        })?
        .parse::<PixelFormat>()?;
    let width = video.width as usize;
    let height = video.height as usize;
    let frames = resolve_frame_count(args, &input, source_format, width, height)?;
    let format = codec_input_format(
        args.codec.as_deref().expect("parser requires codec"),
        source_format,
    );
    Ok(EncodeJob {
        input,
        output,
        recon,
        frames,
        width,
        height,
        source_format,
        format,
    })
}

fn codec_input_format(codec: &str, source_format: PixelFormat) -> PixelFormat {
    if codec_accepts_format(codec, source_format) {
        return source_format;
    }
    let Some(target_depth) = SampleBitDepth::new(8) else {
        return source_format;
    };
    let Some(target_format) = source_format.with_bit_depth(target_depth) else {
        return source_format;
    };
    if source_format.bit_depth().bits() != 8 && codec_accepts_format(codec, target_format) {
        target_format
    } else {
        source_format
    }
}

fn codec_accepts_format(codec: &str, format: PixelFormat) -> bool {
    match codec {
        "av2" => {
            format == PixelFormat::Yuv420p8
                || (format.chroma_sampling() == Some(ChromaSampling::Cs444)
                    && matches!(format.bit_depth().bits(), 8 | 10 | 12))
        }
        "vvc" => format.is_yuv() && format.bit_depth().bits() == 8,
        _ => false,
    }
}

fn resolve_frame_count(
    args: &EncodeArgs,
    input: &EncodeInput,
    format: PixelFormat,
    width: usize,
    height: usize,
) -> Result<usize, String> {
    let frame_len = format
        .frame_len(width, height)
        .ok_or_else(|| format!("frame length overflow for {width}x{height}:{format}"))?;
    if let Some(frames) = args.frames {
        return match input {
            EncodeInput::Path(path) => {
                let available = infer_file_complete_frame_count(path, frame_len)?;
                Ok((frames as usize).min(available))
            }
            EncodeInput::Pattern(_) => Ok(frames as usize),
        };
    }

    match input {
        EncodeInput::Path(path) => infer_file_frame_count_from_eof(path, frame_len),
        EncodeInput::Pattern(_) => {
            Err("source filters require --frames because there is no input EOF".to_string())
        }
    }
}

fn infer_file_complete_frame_count(path: &Path, frame_len: usize) -> Result<usize, String> {
    let metadata = fs::metadata(path)
        .map_err(|err| format!("failed to stat input '{}': {err}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "cannot infer frame count for non-file input '{}'; pass --frames",
            path.display()
        ));
    }
    let byte_len = metadata.len();
    if byte_len == 0 {
        return Err(format!(
            "input '{}' is empty; no complete frames are available",
            path.display()
        ));
    }
    let frame_len = frame_len as u64;
    let frames = byte_len / frame_len;
    if frames == 0 {
        return Err(format!(
            "input '{}' has {} byte(s), less than one {} byte frame",
            path.display(),
            byte_len,
            frame_len
        ));
    }
    usize::try_from(frames).map_err(|_| {
        format!(
            "input '{}' contains too many frames for this platform",
            path.display()
        )
    })
}

fn infer_file_frame_count_from_eof(path: &Path, frame_len: usize) -> Result<usize, String> {
    let complete_frames = infer_file_complete_frame_count(path, frame_len)?;
    let byte_len = fs::metadata(path)
        .map_err(|err| format!("failed to stat input '{}': {err}", path.display()))?
        .len();
    let frame_len = frame_len as u64;
    if byte_len % frame_len != 0 {
        return Err(format!(
            "input '{}' has {} byte(s), which is not a whole number of {} byte frame(s); pass --frames to encode the complete-frame prefix",
            path.display(),
            byte_len,
            frame_len
        ));
    }
    Ok(complete_frames)
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
    let mut input = open_job_reader(&job)?;
    let mut output = create_writer(&job.output)?;
    let mut recon = create_optional_writer(job.recon.as_deref())?;
    let mut frame_metrics = |metrics: frameforge_codecs::av2::Av2EncodeFrameMetrics<'_>| {
        print_frame_metrics(
            "av2",
            &job,
            metrics.frame_idx,
            metrics.frame_count,
            metrics.bitstream_bytes,
            metrics.source,
            metrics.reconstruction,
        );
    };
    frameforge_codecs::av2::av2_encode_fixed_black_444_with_frame_metrics(
        &mut input,
        &mut output,
        recon.as_mut().map(|writer| writer as &mut dyn Write),
        request,
        Some(&mut frame_metrics),
    )?;
    if let (Some(path), Some(writer)) = (job.recon.as_deref(), recon.as_mut()) {
        flush_writer(path, writer)?;
    }
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
    let mut input = open_job_reader(&job)?;
    let mut output = create_writer(&job.output)?;
    let mut recon = create_optional_writer(job.recon.as_deref())?;
    let mut frame_metrics = |metrics: frameforge_codecs::vvc::VvcEncodeFrameMetrics<'_>| {
        print_frame_metrics(
            "vvc",
            &job,
            metrics.frame_idx,
            metrics.frame_count,
            metrics.bitstream_bytes,
            metrics.source,
            metrics.reconstruction,
        );
    };
    frameforge_codecs::vvc::vvc_yuv_encode_stream_with_limits_and_frame_metrics(
        &mut input,
        &mut output,
        recon.as_mut().map(|writer| writer as &mut dyn Write),
        params,
        geometry,
        limits,
        job.format,
        Some(&mut frame_metrics),
    )?;
    if let (Some(path), Some(writer)) = (job.recon.as_deref(), recon.as_mut()) {
        flush_writer(path, writer)?;
    }
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
fn create_writer(path: &Path) -> Result<BufWriter<File>, String> {
    let file = File::create(path)
        .map_err(|err| format!("failed to create output '{}': {err}", path.display()))?;
    Ok(BufWriter::new(file))
}

fn create_optional_writer(path: Option<&Path>) -> Result<Option<BufWriter<File>>, String> {
    path.map(create_writer).transpose()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_yuv_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("frameforge_media_{name}_{unique}.yuv"))
    }

    #[test]
    fn encode_job_infers_file_frames_from_eof_when_frames_omitted() {
        let path = temp_yuv_path("two_frames_8x8");
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&vec![0; 8 * 8 * 3 / 2 * 2])
            .expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv420p8".to_string()),
            }),
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("infer frame count");
        assert_eq!(job.frames, 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_rejects_partial_frame_when_frames_omitted() {
        let path = temp_yuv_path("partial_frame_8x8");
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&vec![0; 8 * 8 * 3 / 2 + 1])
            .expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv420p8".to_string()),
            }),
            frames: None,
            ..EncodeArgs::default()
        };

        let err = encode_job(&args).expect_err("partial frame should fail");
        assert!(err.contains("not a whole number"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_clamps_requested_frames_to_available_file_frames() {
        let path = temp_yuv_path("two_frames_requested_many_8x8");
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&vec![0; 8 * 8 * 3 / 2 * 2])
            .expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv420p8".to_string()),
            }),
            frames: Some(99),
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("clamp frame count");
        assert_eq!(job.frames, 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_converts_high_bit_depth_planar_input_for_current_av2_path() {
        let path = temp_yuv_path("one_frame_8x8_yuv420p10le");
        let yuv420p10 = PixelFormat::yuv420(10).unwrap();
        let samples = yuv420p10.frame_len(8, 8).unwrap() / 2;
        let input = (0..samples)
            .flat_map(|idx| {
                let sample = if idx % 2 == 0 { 0u16 } else { 1023u16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&input).expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv420p10le".to_string()),
            }),
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("build encode job");
        assert_eq!(job.frames, 1);
        assert_eq!(job.source_format, yuv420p10);
        assert_eq!(job.format, PixelFormat::Yuv420p8);

        let mut reader = open_job_reader(&job).expect("open converting reader");
        let mut converted = Vec::new();
        reader
            .read_to_end(&mut converted)
            .expect("read converted frame");
        assert_eq!(
            converted.len(),
            PixelFormat::Yuv420p8.frame_len(8, 8).unwrap()
        );
        assert_eq!(converted[0], 0);
        assert_eq!(converted[1], 255);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_preserves_high_bit_depth_yuv444_for_av2_path() {
        for bits in [10, 12] {
            let format_name = format!("yuv444p{bits}le");
            let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
            let format = PixelFormat::yuv444(bits).unwrap();
            let input = vec![0xAA; format.frame_len(8, 8).unwrap()];
            let mut file = File::create(&path).expect("create temp yuv");
            file.write_all(&input).expect("write temp yuv");
            drop(file);

            let args = EncodeArgs {
                input: Some(path.to_string_lossy().to_string()),
                output: Some("out.obu".to_string()),
                codec: Some("av2".to_string()),
                video: Some(args::VideoSpec {
                    width: 8,
                    height: 8,
                    pixel_format: Some(format_name),
                }),
                frames: None,
                ..EncodeArgs::default()
            };

            let job = encode_job(&args).expect("build encode job");
            assert_eq!(job.frames, 1);
            assert_eq!(job.source_format, format);
            assert_eq!(job.format, format);

            let mut reader = open_job_reader(&job).expect("open reader");
            let mut forwarded = Vec::new();
            reader
                .read_to_end(&mut forwarded)
                .expect("read forwarded frame");
            assert_eq!(forwarded, input);
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn open_job_reader_hides_unselected_file_suffix() {
        let path = temp_yuv_path("reader_prefix_8x8");
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&vec![0xAA; 8 * 8 * 3 / 2 * 3])
            .expect("write temp yuv");
        drop(file);

        let job = EncodeJob {
            input: EncodeInput::Path(path.clone()),
            output: PathBuf::from("out.obu"),
            recon: None,
            frames: 1,
            width: 8,
            height: 8,
            source_format: PixelFormat::Yuv420p8,
            format: PixelFormat::Yuv420p8,
        };
        let mut reader = open_job_reader(&job).expect("open reader");
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).expect("read limited prefix");
        assert_eq!(bytes.len(), 8 * 8 * 3 / 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_requires_frames_for_pattern_source() {
        let args = EncodeArgs {
            input: None,
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv420p8".to_string()),
            }),
            filters: vec!["pattern=black".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let err = encode_job(&args).expect_err("pattern source needs explicit frame count");
        assert!(err.contains("source filters require --frames"), "{err}");
    }
}
