use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Cursor, Read, Write};
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Y4mMetadata {
    width: usize,
    height: usize,
    format: PixelFormat,
    fps: Option<String>,
}

fn is_y4m_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("y4m"))
}

fn read_y4m_file_metadata(path: &Path) -> Result<Option<Y4mMetadata>, String> {
    if !is_y4m_path(path) {
        return Ok(None);
    }
    let file = File::open(path)
        .map_err(|err| format!("failed to open input '{}': {err}", path.display()))?;
    let mut reader = BufReader::new(file);
    read_y4m_stream_header(&mut reader, &y4m_context(path)).map(Some)
}

fn y4m_context(path: &Path) -> String {
    format!("Y4M input '{}'", path.display())
}

fn read_y4m_stream_header<R: BufRead>(
    reader: &mut R,
    context: &str,
) -> Result<Y4mMetadata, String> {
    let mut header = Vec::new();
    let bytes = reader
        .read_until(b'\n', &mut header)
        .map_err(|err| format!("failed to read {context} header: {err}"))?;
    if bytes == 0 {
        return Err(format!("{context} is empty"));
    }
    if !header.ends_with(b"\n") {
        return Err(format!("{context} header is missing a newline"));
    }
    let header = String::from_utf8(header)
        .map_err(|_| format!("{context} header must be valid UTF-8/ASCII"))?;
    parse_y4m_metadata(
        header.trim_end_matches(|ch| ch == '\r' || ch == '\n'),
        context,
    )
}

fn parse_y4m_metadata(header: &str, context: &str) -> Result<Y4mMetadata, String> {
    let fields = y4m_header_fields(header, context)?;
    Ok(Y4mMetadata {
        width: parse_y4m_positive_usize(y4m_header_tag(&fields, 'W'), "width", context)?,
        height: parse_y4m_positive_usize(y4m_header_tag(&fields, 'H'), "height", context)?,
        format: y4m_pixel_format(y4m_header_tag(&fields, 'C'))?,
        fps: y4m_fps(y4m_header_tag(&fields, 'F'), context)?,
    })
}

fn y4m_header_fields<'a>(header: &'a str, context: &str) -> Result<Vec<&'a str>, String> {
    let fields = header.split_whitespace().collect::<Vec<_>>();
    if fields.first() != Some(&"YUV4MPEG2") {
        return Err(format!("{context} is not a Y4M stream"));
    }
    Ok(fields)
}

fn y4m_header_tag<'a>(fields: &'a [&str], tag: char) -> Option<&'a str> {
    fields.iter().skip(1).find_map(|field| {
        let mut chars = field.chars();
        if chars.next() == Some(tag) {
            Some(chars.as_str())
        } else {
            None
        }
    })
}

fn parse_y4m_positive_usize(
    value: Option<&str>,
    field: &str,
    context: &str,
) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{context} header is missing {field}"))?;
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{context} {field} expects an integer, got '{value}'"))?;
    if parsed == 0 {
        Err(format!(
            "{context} {field} expects a positive integer, got 0"
        ))
    } else {
        Ok(parsed)
    }
}

fn y4m_pixel_format(chroma_tag: Option<&str>) -> Result<PixelFormat, String> {
    let normalized = chroma_tag.unwrap_or("420").to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "420" | "420jpeg" | "420mpeg2" | "420paldv"
    ) {
        return Ok(PixelFormat::yuv420(8).expect("8-bit YUV must be supported"));
    }
    if let Some(bits) = numeric_y4m_bit_depth(&normalized, "420p") {
        return PixelFormat::yuv420(bits)
            .ok_or_else(|| format!("unsupported Y4M chroma format: {normalized}"));
    }
    if normalized == "422" {
        return Ok(PixelFormat::yuv422(8).expect("8-bit YUV must be supported"));
    }
    if let Some(bits) = numeric_y4m_bit_depth(&normalized, "422p") {
        return PixelFormat::yuv422(bits)
            .ok_or_else(|| format!("unsupported Y4M chroma format: {normalized}"));
    }
    if normalized == "444" {
        return Ok(PixelFormat::yuv444(8).expect("8-bit YUV must be supported"));
    }
    if let Some(bits) = numeric_y4m_bit_depth(&normalized, "444p") {
        return PixelFormat::yuv444(bits)
            .ok_or_else(|| format!("unsupported Y4M chroma format: {normalized}"));
    }
    Err(format!(
        "unsupported Y4M chroma format: {}",
        chroma_tag.unwrap_or("<default>")
    ))
}

fn numeric_y4m_bit_depth(normalized: &str, prefix: &str) -> Option<u8> {
    normalized.strip_prefix(prefix)?.parse::<u8>().ok()
}

fn y4m_fps(value: Option<&str>, context: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let (num, den) = value
        .split_once(':')
        .ok_or_else(|| format!("{context} fps expects N:D, got '{value}'"))?;
    let num = num
        .parse::<u32>()
        .map_err(|_| format!("{context} fps expects N:D, got '{value}'"))?;
    let den = den
        .parse::<u32>()
        .map_err(|_| format!("{context} fps expects N:D, got '{value}'"))?;
    if num == 0 || den == 0 {
        return Err(format!("{context} fps expects positive N:D, got '{value}'"));
    }
    if den == 1 {
        Ok(Some(num.to_string()))
    } else {
        Ok(Some(format!("{num}/{den}")))
    }
}

fn validate_y4m_job_metadata(
    metadata: &Y4mMetadata,
    job: &EncodeJob,
    path: &Path,
) -> Result<(), String> {
    if metadata.width != job.width
        || metadata.height != job.height
        || metadata.format != job.source_format
    {
        return Err(format!(
            "Y4M input '{}' declares {}x{}:{}, but encode job expects {}x{}:{}",
            path.display(),
            metadata.width,
            metadata.height,
            metadata.format,
            job.width,
            job.height,
            job.source_format
        ));
    }
    Ok(())
}

struct Y4mFrameReader<R> {
    inner: R,
    frame_len: usize,
    frame: Vec<u8>,
    frame_offset: usize,
    frames_remaining: usize,
    frame_index: usize,
    context: String,
}

impl<R: BufRead> Y4mFrameReader<R> {
    fn new(mut inner: R, job: &EncodeJob, path: &Path) -> Result<Self, String> {
        let context = y4m_context(path);
        let metadata = read_y4m_stream_header(&mut inner, &context)?;
        if job.validate_y4m_metadata {
            validate_y4m_job_metadata(&metadata, job, path)?;
        }
        let frame_len = job
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
            frame_len,
            frame: vec![0; frame_len],
            frame_offset: frame_len,
            frames_remaining: job.frames,
            frame_index: 0,
            context,
        })
    }

    fn fill_frame(&mut self) -> io::Result<bool> {
        if self.frames_remaining == 0 {
            return Ok(false);
        }
        let mut header = Vec::new();
        let bytes = self.inner.read_until(b'\n', &mut header)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{} is missing frame {}", self.context, self.frame_index + 1),
            ));
        }
        if !valid_y4m_frame_header(&header) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} has invalid frame marker at frame {}",
                    self.context,
                    self.frame_index + 1
                ),
            ));
        }
        self.inner.read_exact(&mut self.frame)?;
        self.frame_offset = 0;
        self.frames_remaining -= 1;
        self.frame_index += 1;
        Ok(true)
    }
}

impl<R: BufRead> Read for Y4mFrameReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.frame_offset >= self.frame_len && !self.fill_frame()? {
            return Ok(0);
        }
        let remaining = &self.frame[self.frame_offset..];
        let count = remaining.len().min(output.len());
        output[..count].copy_from_slice(&remaining[..count]);
        self.frame_offset += count;
        Ok(count)
    }
}

fn valid_y4m_frame_header(header: &[u8]) -> bool {
    header.ends_with(b"\n")
        && header.starts_with(b"FRAME")
        && header.get(5).is_some_and(|byte| byte.is_ascii_whitespace())
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
            let reader: Box<dyn Read> = if is_y4m_path(path) {
                Box::new(Y4mFrameReader::new(BufReader::new(file), job, path)?)
            } else {
                Box::new(BufReader::new(file).take(selected_input_byte_len(job)?))
            };
            if job.source_format == job.format {
                Ok(reader)
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
    if source == reconstruction {
        return Some(FramePsnr {
            y: f64::INFINITY,
            u: f64::INFINITY,
            v: f64::INFINITY,
            all: f64::INFINITY,
        });
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
    fps: Option<String>,
    validate_y4m_metadata: bool,
    width: usize,
    height: usize,
    source_format: PixelFormat,
    format: PixelFormat,
    lossless: bool,
    qp: Option<u8>,
    av2_predictive: bool,
}

fn print_encode_config(codec_name: &str, args: &EncodeArgs, job: &EncodeJob) {
    let mut settings = args.settings.clone();
    if let Some(qp) = job.qp {
        settings.push(format!("qp={qp}"));
    }
    let settings = if settings.is_empty() {
        "none".to_string()
    } else {
        settings.join(",")
    };
    eprintln!(
        "input: {} video={}x{}:{} frames={} fps={}",
        input_label(&job.input),
        job.width,
        job.height,
        job.source_format,
        job.frames,
        job.fps.as_deref().unwrap_or("unspecified")
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
    let y4m_metadata = match &input {
        EncodeInput::Path(path) => read_y4m_file_metadata(path)?,
        EncodeInput::Pattern(_) => None,
    };
    let (width, height, source_format) = resolve_video_metadata(args, y4m_metadata.as_ref())?;
    let frames = resolve_frame_count(args, &input, source_format, width, height)?;
    let codec = args.codec.as_deref().expect("parser requires codec");
    let lossless = boolean_setting_enabled(&args.settings, "lossless")?;
    if lossless && args.qp.is_some() {
        return Err("--qp is mutually exclusive with --set lossless".to_string());
    }
    if args.qp.is_some() && codec != "av2" {
        return Err("--qp is currently implemented for AV2 encode only".to_string());
    }
    let av2_predictive = boolean_setting_enabled(&args.settings, "predictive")?;
    if source_format == PixelFormat::Rgb24 {
        if codec != "av2" {
            return Err("rgb24 encode is currently implemented for AV2 only".to_string());
        }
    }
    let format = if lossless {
        source_format
    } else {
        codec_input_format(codec, source_format)
    };
    if lossless && !codec_supports_lossless_stream(codec, format) {
        return Err(format!(
            "lossless encode is not implemented for {codec} {format}"
        ));
    }
    Ok(EncodeJob {
        input,
        output,
        recon,
        frames,
        fps: resolve_fps_metadata(args, y4m_metadata.as_ref()),
        validate_y4m_metadata: y4m_metadata.is_some() && !args.explicit_video,
        width,
        height,
        source_format,
        format,
        lossless,
        qp: args.qp,
        av2_predictive,
    })
}

fn resolve_video_metadata(
    args: &EncodeArgs,
    y4m_metadata: Option<&Y4mMetadata>,
) -> Result<(usize, usize, PixelFormat), String> {
    match (args.video.as_ref(), y4m_metadata) {
        (Some(video), Some(metadata)) if args.explicit_video => {
            resolve_video_spec(video, Some(metadata.format))
        }
        (Some(_), Some(metadata)) | (None, Some(metadata)) => {
            Ok((metadata.width, metadata.height, metadata.format))
        }
        (Some(video), None) => resolve_video_spec(video, None),
        (None, None) => Err(
            "encode requires --video WxH:pixfmt, filename metadata, or a Y4M header".to_string(),
        ),
    }
}

fn resolve_video_spec(
    video: &args::VideoSpec,
    fallback_format: Option<PixelFormat>,
) -> Result<(usize, usize, PixelFormat), String> {
    let source_format = match video.pixel_format.as_deref() {
        Some(format) => format.parse::<PixelFormat>()?,
        None => fallback_format.ok_or_else(|| {
            "encode requires a pixel format in --video, input filename, or Y4M header".to_string()
        })?,
    };
    Ok((video.width as usize, video.height as usize, source_format))
}

fn resolve_fps_metadata(args: &EncodeArgs, y4m_metadata: Option<&Y4mMetadata>) -> Option<String> {
    if args.explicit_fps {
        return args.fps.clone();
    }
    y4m_metadata
        .and_then(|metadata| metadata.fps.clone())
        .or_else(|| args.fps.clone())
}

fn boolean_setting_enabled(settings: &[String], setting_name: &str) -> Result<bool, String> {
    for spec in settings {
        if args::setting_name(spec) != setting_name {
            continue;
        }
        let value = args::setting_value(spec).unwrap_or("true");
        match value {
            "true" | "1" | "yes" | "on" => return Ok(true),
            "false" | "0" | "no" | "off" => return Ok(false),
            _ => {
                return Err(format!(
                    "{setting_name} expects true or false, got '{value}'"
                ))
            }
        }
    }
    Ok(false)
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
            format == PixelFormat::Rgb24
                || (matches!(
                    format.chroma_sampling(),
                    Some(ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444)
                ) && matches!(format.bit_depth().bits(), 8 | 10))
        }
        "vvc" => match format.chroma_sampling() {
            Some(ChromaSampling::Cs420) => vvc_accepts_bit_depth(format),
            Some(ChromaSampling::Cs422) => format.bit_depth().bits() == 8,
            Some(ChromaSampling::Cs444) => vvc_accepts_bit_depth(format),
            _ => false,
        },
        _ => false,
    }
}

fn codec_supports_lossless_stream(codec: &str, format: PixelFormat) -> bool {
    match codec {
        "av2" => {
            format == PixelFormat::Rgb24
                || (matches!(
                    format.chroma_sampling(),
                    Some(ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444)
                ) && matches!(format.bit_depth().bits(), 8 | 10))
        }
        "vvc" => {
            matches!(
                format.chroma_sampling(),
                Some(ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444)
            ) && vvc_accepts_bit_depth(format)
        }
        _ => false,
    }
}

const VVC_MIN_BIT_DEPTH: u8 = 8;
const VVC_MAX_BIT_DEPTH: u8 = 12;

fn vvc_accepts_bit_depth(format: PixelFormat) -> bool {
    (VVC_MIN_BIT_DEPTH..=VVC_MAX_BIT_DEPTH).contains(&format.bit_depth().bits())
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
                if is_y4m_path(path) {
                    infer_y4m_complete_frame_count(path, frame_len, Some(frames as usize))
                } else {
                    let available = infer_file_complete_frame_count(path, frame_len)?;
                    Ok((frames as usize).min(available))
                }
            }
            EncodeInput::Pattern(_) => Ok(frames as usize),
        };
    }

    match input {
        EncodeInput::Path(path) => {
            if is_y4m_path(path) {
                infer_y4m_complete_frame_count(path, frame_len, None)
            } else {
                infer_file_frame_count_from_eof(path, frame_len)
            }
        }
        EncodeInput::Pattern(_) => {
            Err("source filters require --frames because there is no input EOF".to_string())
        }
    }
}

fn infer_y4m_complete_frame_count(
    path: &Path,
    frame_len: usize,
    limit: Option<usize>,
) -> Result<usize, String> {
    let file = File::open(path)
        .map_err(|err| format!("failed to open input '{}': {err}", path.display()))?;
    let mut reader = BufReader::new(file);
    let context = y4m_context(path);
    read_y4m_stream_header(&mut reader, &context)?;
    let mut frames = 0usize;
    while limit.map_or(true, |limit| frames < limit) {
        let mut frame_header = Vec::new();
        let bytes = reader
            .read_until(b'\n', &mut frame_header)
            .map_err(|err| format!("failed to read {context} frame marker: {err}"))?;
        if bytes == 0 {
            break;
        }
        if !valid_y4m_frame_header(&frame_header) {
            return Err(format!(
                "{context} has invalid frame marker at frame {}",
                frames + 1
            ));
        }
        skip_exact_y4m_payload(&mut reader, frame_len, &context, frames + 1)?;
        frames += 1;
    }
    if frames == 0 {
        return Err(format!("{context} contains no complete frames"));
    }
    Ok(frames)
}

fn skip_exact_y4m_payload<R: Read>(
    reader: &mut R,
    frame_len: usize,
    context: &str,
    frame_number: usize,
) -> Result<(), String> {
    let mut remaining = frame_len;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len());
        reader
            .read_exact(&mut buffer[..chunk])
            .map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => {
                    format!("{context} is too short while reading frame {frame_number}")
                }
                _ => format!("failed to read {context} frame {frame_number}: {err}"),
            })?;
        remaining -= chunk;
    }
    Ok(())
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
    let options = frameforge_codecs::av2::Av2EncodeOptions {
        lossless: job.lossless,
        qp: job.qp,
        predictive: job.av2_predictive,
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
    frameforge_codecs::av2::av2_encode_fixed_black_444_with_options_and_frame_metrics(
        &mut input,
        &mut output,
        recon.as_mut().map(|writer| writer as &mut dyn Write),
        request,
        options,
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
    frameforge_codecs::vvc::vvc_yuv_encode_stream_with_limits_and_options_and_frame_metrics(
        &mut input,
        &mut output,
        recon.as_mut().map(|writer| writer as &mut dyn Write),
        params,
        geometry,
        limits,
        job.format,
        frameforge_codecs::vvc::VvcEncodeOptions {
            lossless: job.lossless,
        },
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
        temp_input_path(name, "yuv")
    }

    fn temp_y4m_path(name: &str) -> PathBuf {
        temp_input_path(name, "y4m")
    }

    fn temp_input_path(name: &str, extension: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("frameforge_media_{name}_{unique}.{extension}"))
    }

    fn write_y4m(path: &Path, header: &str, frames: &[Vec<u8>]) {
        let mut file = File::create(path).expect("create temp y4m");
        file.write_all(header.as_bytes()).expect("write y4m header");
        for frame in frames {
            file.write_all(b"FRAME\n").expect("write y4m frame marker");
            file.write_all(frame).expect("write y4m frame");
        }
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
    fn encode_job_infers_y4m_metadata_and_frames_from_header() {
        let path = temp_y4m_path("two_frames_4x4");
        let frame_len = PixelFormat::Yuv420p8.frame_len(4, 4).unwrap();
        let first = vec![0x11; frame_len];
        let second = vec![0x22; frame_len];
        write_y4m(
            &path,
            "YUV4MPEG2 W4 H4 F15:1 Ip A0:0 C420jpeg XYSCSS=420JPEG\n",
            &[first.clone(), second.clone()],
        );

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("infer Y4M metadata");
        assert_eq!(job.width, 4);
        assert_eq!(job.height, 4);
        assert_eq!(job.source_format, PixelFormat::Yuv420p8);
        assert_eq!(job.frames, 2);
        assert_eq!(job.fps.as_deref(), Some("15"));

        let mut reader = open_job_reader(&job).expect("open Y4M reader");
        let mut raw = Vec::new();
        reader.read_to_end(&mut raw).expect("read raw frames");
        let mut expected = first;
        expected.extend(second);
        assert_eq!(raw, expected);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_uses_y4m_metadata_before_filename_metadata() {
        let path = temp_y4m_path("clip_8x8_30_yuv444p8");
        let frame_len = PixelFormat::Yuv420p8.frame_len(4, 4).unwrap();
        write_y4m(
            &path,
            "YUV4MPEG2 W4 H4 F24:1 Ip A0:0 C420\n",
            &[vec![0; frame_len]],
        );

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("yuv444p8".to_string()),
            }),
            fps: Some("30".to_string()),
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("Y4M header should win over inferred filename metadata");
        assert_eq!(job.width, 4);
        assert_eq!(job.height, 4);
        assert_eq!(job.source_format, PixelFormat::Yuv420p8);
        assert_eq!(job.fps.as_deref(), Some("24"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_allows_explicit_video_to_override_y4m_header() {
        let path = temp_y4m_path("override_header");
        let explicit_frame_len = PixelFormat::Yuv420p8.frame_len(4, 4).unwrap();
        let frame = vec![0x33; explicit_frame_len];
        write_y4m(
            &path,
            "YUV4MPEG2 W8 H8 F30:1 Ip A0:0 C420\n",
            std::slice::from_ref(&frame),
        );

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 4,
                height: 4,
                pixel_format: Some("yuv420p8".to_string()),
            }),
            explicit_video: true,
            frames: Some(1),
            fps: Some("60".to_string()),
            explicit_fps: true,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("explicit metadata should override Y4M header");
        assert_eq!(job.width, 4);
        assert_eq!(job.height, 4);
        assert_eq!(job.source_format, PixelFormat::Yuv420p8);
        assert_eq!(job.frames, 1);
        assert_eq!(job.fps.as_deref(), Some("60"));

        let mut reader = open_job_reader(&job).expect("open overridden Y4M reader");
        let mut raw = Vec::new();
        reader.read_to_end(&mut raw).expect("read raw frame");
        assert_eq!(raw, frame);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_preserves_high_bit_depth_yuv420_for_av2_path() {
        for bits in [10] {
            let format_name = format!("yuv420p{bits}le");
            let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
            let format = PixelFormat::yuv420(bits).unwrap();
            let samples = format.frame_len(8, 8).unwrap() / format.bytes_per_sample();
            let max_sample = format.bit_depth().max_sample();
            let input = (0..samples)
                .flat_map(|idx| {
                    let sample = if idx % 2 == 0 { 0u16 } else { max_sample };
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
    fn encode_job_preserves_high_bit_depth_yuv444_for_av2_path() {
        for bits in [10] {
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
    fn encode_job_preserves_high_bit_depth_yuv420_for_vvc_path() {
        for bits in [10, 12] {
            let format_name = format!("yuv420p{bits}le");
            let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
            let format = PixelFormat::yuv420(bits).unwrap();
            let input = vec![0x55; format.frame_len(8, 8).unwrap()];
            let mut file = File::create(&path).expect("create temp yuv");
            file.write_all(&input).expect("write temp yuv");
            drop(file);

            let args = EncodeArgs {
                input: Some(path.to_string_lossy().to_string()),
                output: Some("out.266".to_string()),
                codec: Some("vvc".to_string()),
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
    fn encode_job_accepts_lossless_yuv420_for_vvc_path() {
        let format_name = "yuv420p10le";
        let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
        let format = PixelFormat::yuv420(10).unwrap();
        let input = vec![0; format.frame_len(8, 8).unwrap()];
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&input).expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.266".to_string()),
            codec: Some("vvc".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some(format_name.to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("lossless yuv420 is native for VVC");
        assert!(job.lossless);
        assert_eq!(job.source_format, format);
        assert_eq!(job.format, format);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_rejects_qp_with_lossless() {
        let path = temp_yuv_path("one_frame_8x8_qp_lossless_conflict");
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&vec![0; 8 * 8 * 3 / 2])
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
            settings: vec!["lossless=true".to_string()],
            qp: Some(16),
            frames: None,
            ..EncodeArgs::default()
        };

        let err = encode_job(&args).expect_err("QP and lossless should conflict");
        assert!(
            err.contains("--qp is mutually exclusive with --set lossless"),
            "{err}"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_preserves_high_bit_depth_yuv444_for_vvc_path() {
        for bits in [10, 12] {
            let format_name = format!("yuv444p{bits}le");
            let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
            let format = PixelFormat::yuv444(bits).unwrap();
            let input = vec![0x66; format.frame_len(8, 8).unwrap()];
            let mut file = File::create(&path).expect("create temp yuv");
            file.write_all(&input).expect("write temp yuv");
            drop(file);

            let args = EncodeArgs {
                input: Some(path.to_string_lossy().to_string()),
                output: Some("out.266".to_string()),
                codec: Some("vvc".to_string()),
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
    fn encode_job_rejects_lossless_without_bit_depth_fallback() {
        let bits = 13;
        let format_name = format!("yuv420p{bits}le");
        let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
        let format = PixelFormat::yuv420(bits).unwrap();
        let input = vec![0; format.frame_len(8, 8).unwrap()];
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
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let err = encode_job(&args).expect_err("lossless fallback must be rejected");
        assert!(
            err.contains("lossless encode is not implemented for av2 yuv420p13le"),
            "{err}"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_accepts_lossless_yuv420_for_av2_path() {
        let format_name = "yuv420p10le";
        let path = temp_yuv_path(&format!("one_frame_8x8_{format_name}"));
        let format = PixelFormat::yuv420(10).unwrap();
        let input = vec![0; format.frame_len(8, 8).unwrap()];
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
                pixel_format: Some(format_name.to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("AV2 lossless 4:2:0 is native");
        assert!(job.lossless);
        assert_eq!(job.source_format, format);
        assert_eq!(job.format, format);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_accepts_lossless_rgb24_for_av2_path() {
        let path = temp_input_path("one_frame_8x8_rgb24", "rgb");
        let input = (0..PixelFormat::Rgb24.frame_len(8, 8).unwrap())
            .map(|index| ((index * 19 + 11) & 0xff) as u8)
            .collect::<Vec<_>>();
        let mut file = File::create(&path).expect("create temp rgb");
        file.write_all(&input).expect("write temp rgb");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("rgb24".to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("AV2 lossless rgb24 is native");
        assert!(job.lossless);
        assert_eq!(job.source_format, PixelFormat::Rgb24);
        assert_eq!(job.format, PixelFormat::Rgb24);
        let mut reader = open_job_reader(&job).expect("open rgb reader");
        let mut forwarded = Vec::new();
        reader
            .read_to_end(&mut forwarded)
            .expect("read forwarded rgb frame");
        assert_eq!(forwarded, input);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_accepts_non_lossless_rgb24_for_av2_path() {
        let path = temp_input_path("one_frame_8x8_rgb24_lossy", "rgb");
        let input = vec![0; PixelFormat::Rgb24.frame_len(8, 8).unwrap()];
        let mut file = File::create(&path).expect("create temp rgb");
        file.write_all(&input).expect("write temp rgb");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.obu".to_string()),
            codec: Some("av2".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("rgb24".to_string()),
            }),
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("AV2 non-lossless rgb24 is native");
        assert!(!job.lossless);
        assert_eq!(job.source_format, PixelFormat::Rgb24);
        assert_eq!(job.format, PixelFormat::Rgb24);
        let mut reader = open_job_reader(&job).expect("open rgb reader");
        let mut forwarded = Vec::new();
        reader
            .read_to_end(&mut forwarded)
            .expect("read forwarded rgb frame");
        assert_eq!(forwarded, input);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_rejects_rgb24_for_vvc_path() {
        let path = temp_input_path("one_frame_8x8_rgb24_vvc", "rgb");
        let mut file = File::create(&path).expect("create temp rgb");
        file.write_all(&vec![0; PixelFormat::Rgb24.frame_len(8, 8).unwrap()])
            .expect("write temp rgb");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.vvc".to_string()),
            codec: Some("vvc".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some("rgb24".to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let err = encode_job(&args).expect_err("VVC rgb24 path should be rejected");
        assert!(
            err.contains("rgb24 encode is currently implemented for AV2 only"),
            "{err}"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_accepts_lossless_yuv422_for_av2_without_bit_depth_fallback() {
        let format_name = "yuv422p10le";
        let path = temp_yuv_path(&format!("one_frame_8x8_av2_{format_name}"));
        let format = PixelFormat::yuv422(10).unwrap();
        let input = vec![0; format.frame_len(8, 8).unwrap()];
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
                pixel_format: Some(format_name.to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("AV2 lossless 4:2:2 is native");
        assert!(job.lossless);
        assert_eq!(job.source_format, format);
        assert_eq!(job.format, format);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn encode_job_accepts_lossless_yuv422_for_vvc_path() {
        let format_name = "yuv422p10le";
        let path = temp_yuv_path(&format!("one_frame_8x8_vvc_{format_name}"));
        let format = PixelFormat::yuv422(10).unwrap();
        let input = vec![0; format.frame_len(8, 8).unwrap()];
        let mut file = File::create(&path).expect("create temp yuv");
        file.write_all(&input).expect("write temp yuv");
        drop(file);

        let args = EncodeArgs {
            input: Some(path.to_string_lossy().to_string()),
            output: Some("out.266".to_string()),
            codec: Some("vvc".to_string()),
            video: Some(args::VideoSpec {
                width: 8,
                height: 8,
                pixel_format: Some(format_name.to_string()),
            }),
            settings: vec!["lossless=true".to_string()],
            frames: None,
            ..EncodeArgs::default()
        };

        let job = encode_job(&args).expect("VVC lossless 4:2:2 is native");
        assert!(job.lossless);
        assert_eq!(job.source_format, format);
        assert_eq!(job.format, format);
        let _ = fs::remove_file(path);
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
            fps: None,
            validate_y4m_metadata: false,
            width: 8,
            height: 8,
            source_format: PixelFormat::Yuv420p8,
            format: PixelFormat::Yuv420p8,
            lossless: false,
            qp: None,
            av2_predictive: false,
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
