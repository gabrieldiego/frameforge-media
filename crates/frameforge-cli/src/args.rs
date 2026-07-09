use std::ffi::OsString;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Codecs,
    Filters,
    Encode(EncodeArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodeArgs {
    pub input: Option<String>,
    pub output: Option<String>,
    pub codec: Option<String>,
    pub video: Option<VideoSpec>,
    pub frames: Option<u32>,
    pub fps: Option<String>,
    pub filters: Vec<String>,
    pub settings: Vec<String>,
    pub preset: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    pub pixel_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecPathSpec {
    pub codec: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpRow {
    pub syntax: &'static str,
    pub summary: &'static str,
}

pub const USAGE: &[&str] = &[
    "ff --help",
    "ff --version",
    "ff codecs",
    "ff filters",
    "ff encode <input> [input-options] [--filter <spec>] --encode <codec:path> [output-options]",
];

pub const OUTPUT_OPTIONS: &[HelpRow] = &[
    HelpRow {
        syntax: "--encode <codec:path>",
        summary: "Encoder codec/output endpoint, e.g. av2:output.obu",
    },
    HelpRow {
        syntax: "--set <key[=value]>",
        summary: "Encode setting; bare keys such as lossless imply true",
    },
    HelpRow {
        syntax: "--preset <name>",
        summary: "Encoder preset name",
    },
];

pub const INPUT_OPTIONS: &[HelpRow] = &[
    HelpRow {
        syntax: "<input>",
        summary: "Raw input path",
    },
    HelpRow {
        syntax: "filename metadata",
        summary: "Names imply metadata with *_<WxH>[_<fps>][_<frames>f]_<pixfmt>.yuv",
    },
    HelpRow {
        syntax: "--video <WxH:fmt>",
        summary: "Override or provide raw metadata, e.g. 1920x1080:yuv444p",
    },
    HelpRow {
        syntax: "--fps <rate>",
        summary: "Input frame rate, e.g. 30, 29.97, or 30000/1001",
    },
    HelpRow {
        syntax: "-n, --frames <count>",
        summary: "Number of frames to process",
    },
];

pub const FILTER_OPTIONS: &[HelpRow] = &[HelpRow {
    syntax: "-f, --filter <spec>",
    summary: "Filter stage, repeatable, e.g. scale=w=640:h=360",
}];

pub const DISCOVERY_COMMANDS: &[HelpRow] = &[
    HelpRow {
        syntax: "ff codecs",
        summary: "List known codec stages and build-time features",
    },
    HelpRow {
        syntax: "ff filters",
        summary: "List known filter stages and build-time features",
    },
];

pub fn help(version: &str) -> String {
    let mut text = format!("FrameForge {version}\n\nUsage:\n");
    for usage in USAGE {
        text.push_str("  ");
        text.push_str(usage);
        text.push('\n');
    }

    text.push_str("\nInput options apply after <input>; output options apply after --encode.\n");

    text.push_str("\nInput options:\n");
    push_help_rows(&mut text, INPUT_OPTIONS);

    text.push_str("\nFilter options:\n");
    push_help_rows(&mut text, FILTER_OPTIONS);

    text.push_str("\nOutput options:\n");
    push_help_rows(&mut text, OUTPUT_OPTIONS);

    text.push_str("\nStage discovery:\n");
    push_help_rows(&mut text, DISCOVERY_COMMANDS);
    text
}

fn push_help_rows(text: &mut String, rows: &[HelpRow]) {
    let width = rows.iter().map(|row| row.syntax.len()).max().unwrap_or(0) + 2;
    for row in rows {
        text.push_str(&format!(
            "  {:<width$} {}\n",
            row.syntax,
            row.summary,
            width = width
        ));
    }
}

pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut cursor = Cursor::new(args.into_iter().skip(1).collect());
    let Some(command) = cursor.next() else {
        return Ok(Command::Help);
    };

    match command.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "-V" | "--version" | "version" => Ok(Command::Version),
        "codecs" => parse_no_extra(cursor, Command::Codecs, "codecs"),
        "filters" => parse_no_extra(cursor, Command::Filters, "filters"),
        "encode" => parse_encode(cursor),
        other => Err(format!("unknown command '{other}'")),
    }
}

pub fn parse_os<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut converted = Vec::new();
    for arg in args {
        converted.push(
            arg.into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_string())?,
        );
    }
    parse(converted)
}

fn parse_no_extra(mut cursor: Cursor, command: Command, name: &str) -> Result<Command, String> {
    match cursor.next().as_deref() {
        None => Ok(command),
        Some("-h") | Some("--help") => Ok(Command::Help),
        Some(extra) => Err(format!("'{name}' does not accept argument '{extra}'")),
    }
}

fn parse_encode(mut cursor: Cursor) -> Result<Command, String> {
    let mut args = EncodeArgs::default();
    while let Some(arg) = cursor.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--encode" => {
                if args.codec.is_some() || args.output.is_some() {
                    return Err("encode accepts only one --encode endpoint".to_string());
                }
                let endpoint = parse_codec_path_spec(arg.as_str(), &cursor.value(arg.as_str())?)?;
                args.codec = Some(endpoint.codec);
                args.output = Some(endpoint.path);
            }
            "--video" => {
                args.video = Some(parse_video_spec(
                    arg.as_str(),
                    &cursor.value(arg.as_str())?,
                )?)
            }
            "--frames" | "-n" => {
                args.frames = Some(parse_u32(arg.as_str(), &cursor.value(arg.as_str())?)?)
            }
            "--fps" => args.fps = Some(parse_fps(arg.as_str(), &cursor.value(arg.as_str())?)?),
            "--filter" | "-f" => args.filters.push(cursor.value(arg.as_str())?),
            "--set" => args
                .settings
                .push(parse_setting(&cursor.value(arg.as_str())?)),
            "--preset" => args.preset = Some(cursor.value(arg.as_str())?),
            other if other.starts_with('-') => {
                return Err(format!("unknown encode option '{other}'"))
            }
            other => {
                if args.input.is_some() {
                    return Err(format!("unexpected encode argument '{other}'"));
                }
                args.input = Some(other.to_string());
            }
        }
    }

    resolve_encode_input_metadata(&mut args)?;
    if args.input.is_none() {
        return Err("encode requires an input path".to_string());
    }
    if args.codec.is_none() || args.output.is_none() {
        return Err("encode requires --encode codec:path".to_string());
    }
    Ok(Command::Encode(args))
}

fn parse_u32(option: &str, value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{option} expects a positive integer, got '{value}'"))?;
    if parsed == 0 {
        Err(format!("{option} expects a positive integer, got 0"))
    } else {
        Ok(parsed)
    }
}

fn parse_fps(option: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{option} expects a positive frame rate"));
    }

    if let Some((num, den)) = value.split_once('/') {
        let num = parse_u32(option, num)?;
        let den = parse_u32(option, den)?;
        return Ok(format!("{num}/{den}"));
    }

    let mut saw_digit = false;
    let mut saw_dot = false;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            saw_digit = true;
        } else if byte == b'.' && !saw_dot {
            saw_dot = true;
        } else {
            return Err(format!(
                "{option} expects a positive frame rate, got '{value}'"
            ));
        }
    }
    if !saw_digit || value.trim_matches('0').trim_matches('.').is_empty() {
        return Err(format!(
            "{option} expects a positive frame rate, got '{value}'"
        ));
    }
    Ok(value.to_string())
}

fn parse_video_spec(option: &str, value: &str) -> Result<VideoSpec, String> {
    let (dimensions, pixel_format) = value
        .split_once(':')
        .ok_or_else(|| format!("{option} expects WxH:pixfmt, got '{value}'"))?;
    let split = dimensions
        .find('x')
        .or_else(|| dimensions.find('X'))
        .ok_or_else(|| format!("{option} expects WxH:pixfmt, got '{value}'"))?;
    let width = parse_u32(option, &dimensions[..split])?;
    let height = parse_u32(option, &dimensions[split + 1..])?;
    Ok(VideoSpec {
        width,
        height,
        pixel_format: Some(normalize_pixel_format(pixel_format)?),
    })
}

fn parse_codec_path_spec(option: &str, value: &str) -> Result<CodecPathSpec, String> {
    let (codec, path) = value
        .split_once(':')
        .ok_or_else(|| format!("{option} expects codec:path, got '{value}'"))?;
    if codec.is_empty() {
        return Err(format!("{option} codec must not be empty"));
    }
    if path.is_empty() {
        return Err(format!("{option} path must not be empty"));
    }
    Ok(CodecPathSpec {
        codec: codec.to_string(),
        path: path.to_string(),
    })
}

fn parse_setting(value: &str) -> String {
    if value.contains('=') {
        value.to_string()
    } else {
        format!("{value}=true")
    }
}

pub fn setting_name(spec: &str) -> &str {
    spec.split_once('=').map_or(spec, |(name, _)| name)
}

pub fn setting_value(spec: &str) -> Option<&str> {
    spec.split_once('=').map(|(_, value)| value)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InputMetadata {
    video: Option<VideoSpec>,
    frames: Option<u32>,
    fps: Option<String>,
}

fn resolve_encode_input_metadata(args: &mut EncodeArgs) -> Result<(), String> {
    let inferred = match args.input.as_deref() {
        Some(input) => infer_input_metadata(input)?,
        None => InputMetadata::default(),
    };
    if args.frames.is_none() {
        args.frames = inferred.frames;
    }
    if args.fps.is_none() {
        args.fps = inferred.fps;
    }

    if args.video.is_none() {
        args.video = inferred.video;
    }
    Ok(())
}

fn infer_input_metadata(path: &str) -> Result<InputMetadata, String> {
    let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return Ok(InputMetadata::default());
    };
    let name = name.to_ascii_lowercase();
    let dimensions = find_dimensions(&name)?;
    let pixel_format = find_pixel_format(&name)?;
    let video = dimensions.map(|(width, height)| VideoSpec {
        width,
        height,
        pixel_format,
    });
    Ok(InputMetadata {
        video,
        frames: find_frame_count(&name)?,
        fps: find_fps(&name)?,
    })
}

fn find_dimensions(text: &str) -> Result<Option<(u32, u32)>, String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        if !bytes[start].is_ascii_digit() {
            start += 1;
            continue;
        }

        let mut split = start;
        while split < bytes.len() && bytes[split].is_ascii_digit() {
            split += 1;
        }
        if split == bytes.len() || !matches!(bytes[split], b'x' | b'X') {
            start = split.saturating_add(1);
            continue;
        }

        let height_start = split + 1;
        let mut end = height_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == height_start {
            start = end.saturating_add(1);
            continue;
        }

        let width = parse_u32("input filename width", &text[start..split])?;
        let height = parse_u32("input filename height", &text[height_start..end])?;
        return Ok(Some((width, height)));
    }
    Ok(None)
}

fn find_pixel_format(text: &str) -> Result<Option<String>, String> {
    const TOKENS: &[&str] = &[
        "yuv420p16le",
        "yuv420p16",
        "yuv420p12le",
        "yuv420p12",
        "yuv420p10le",
        "yuv420p10",
        "yuv420p8",
        "yuv420p",
        "yuv422p16le",
        "yuv422p16",
        "yuv422p12le",
        "yuv422p12",
        "yuv422p10le",
        "yuv422p10",
        "yuv422p8",
        "yuv422p",
        "yuv444p16le",
        "yuv444p16",
        "yuv444p12le",
        "yuv444p12",
        "yuv444p10le",
        "yuv444p10",
        "yuv444p8",
        "yuv444p",
        "rgb24",
        "i420",
        "i422",
        "i444",
    ];
    for token in TOKENS {
        if text.contains(token) {
            return Ok(Some(normalize_pixel_format(token)?));
        }
    }
    Ok(None)
}

fn find_frame_count(text: &str) -> Result<Option<u32>, String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        if !bytes[start].is_ascii_digit() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        let suffix = &text[end..];
        if suffix.starts_with("frames") || suffix.starts_with('f') {
            return Ok(Some(parse_u32(
                "input filename frame count",
                &text[start..end],
            )?));
        }
        start = end.saturating_add(1);
    }
    Ok(None)
}

fn find_fps(text: &str) -> Result<Option<String>, String> {
    let Some((_, height_end)) = find_dimensions_span(text) else {
        return Ok(None);
    };
    let bytes = text.as_bytes();

    if height_end < bytes.len() && bytes[height_end] == b'p' {
        let fps_start = height_end + 1;
        let mut fps_end = fps_start;
        while fps_end < bytes.len() && bytes[fps_end].is_ascii_digit() {
            fps_end += 1;
        }
        if fps_end > fps_start {
            return Ok(Some(normalize_filename_fps(&text[fps_start..fps_end])?));
        }
    }

    let mut idx = height_end;
    while idx < bytes.len() && matches!(bytes[idx], b'_' | b'-' | b'.') {
        idx += 1;
    }
    let fps_start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > fps_start {
        let suffix = &text[idx..];
        if suffix.starts_with(".yuv")
            || suffix.starts_with(".y4m")
            || suffix.starts_with('_')
            || suffix.starts_with('-')
        {
            return Ok(Some(normalize_filename_fps(&text[fps_start..idx])?));
        }
    }
    Ok(None)
}

fn normalize_filename_fps(value: &str) -> Result<String, String> {
    let fps = parse_u32("input filename fps", value)?;
    if fps >= 1000 && fps % 100 == 97 {
        return Ok(format!("{}.{:02}", fps / 100, fps % 100));
    }
    Ok(fps.to_string())
}

fn find_dimensions_span(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        if !bytes[start].is_ascii_digit() {
            start += 1;
            continue;
        }

        let mut split = start;
        while split < bytes.len() && bytes[split].is_ascii_digit() {
            split += 1;
        }
        if split == bytes.len() || !matches!(bytes[split], b'x' | b'X') {
            start = split.saturating_add(1);
            continue;
        }

        let height_start = split + 1;
        let mut end = height_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > height_start {
            return Some((start, end));
        }
        start = end.saturating_add(1);
    }
    None
}

fn normalize_pixel_format(value: &str) -> Result<String, String> {
    let pixel_format = value.trim().to_ascii_lowercase();
    match pixel_format.as_str() {
        "" => Err("pixel format must not be empty".to_string()),
        "i420" => Ok("yuv420p8".to_string()),
        "i422" => Ok("yuv422p8".to_string()),
        "i444" => Ok("yuv444p8".to_string()),
        "yuv420p" => Ok("yuv420p8".to_string()),
        "yuv422p" => Ok("yuv422p8".to_string()),
        "yuv444p" => Ok("yuv444p8".to_string()),
        other => Ok(other.to_string()),
    }
}

fn stage_name(spec: &str) -> &str {
    spec.split_once('=')
        .or_else(|| spec.split_once(':'))
        .map_or(spec, |(name, _)| name)
}

pub fn filter_names(filters: &[String]) -> impl Iterator<Item = &str> {
    filters.iter().map(|filter| stage_name(filter))
}

#[derive(Debug, Clone)]
struct Cursor {
    args: Vec<String>,
    index: usize,
}

impl Cursor {
    fn new(args: Vec<String>) -> Self {
        Self { args, index: 0 }
    }

    fn next(&mut self) -> Option<String> {
        let value = self.args.get(self.index).cloned();
        if value.is_some() {
            self.index += 1;
        }
        value
    }

    fn value(&mut self, option: &str) -> Result<String, String> {
        let value = self
            .next()
            .ok_or_else(|| format!("{option} requires a value"))?;
        if value.starts_with('-') {
            Err(format!("{option} requires a value, got option '{value}'"))
        } else {
            Ok(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Command, String> {
        parse(words.iter().map(|word| (*word).to_string()))
    }

    #[test]
    fn parses_encode_command() {
        let command = parse_words(&[
            "ff",
            "encode",
            "in.yuv",
            "--video",
            "64x64:yuv444p",
            "--filter",
            "scale=w=64:h=64",
            "--encode",
            "av2:out.obu",
            "--set",
            "lossless",
        ])
        .unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(args.input.as_deref(), Some("in.yuv"));
        assert_eq!(args.output.as_deref(), Some("out.obu"));
        assert_eq!(args.codec.as_deref(), Some("av2"));
        assert_eq!(
            args.video,
            Some(VideoSpec {
                width: 64,
                height: 64,
                pixel_format: Some("yuv444p8".to_string())
            })
        );
        assert_eq!(args.filters, vec!["scale=w=64:h=64"]);
        assert_eq!(args.settings, vec!["lossless=true"]);
    }

    #[test]
    fn infers_dimensions_fps_and_format_from_input_filename() {
        let command = parse_words(&[
            "ff",
            "encode",
            "screen_640x360_1f_yuv444p8.yuv",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(
            args.video,
            Some(VideoSpec {
                width: 640,
                height: 360,
                pixel_format: Some("yuv444p8".to_string())
            })
        );
        assert_eq!(args.frames, Some(1));
        assert_eq!(args.fps, None);
    }

    #[test]
    fn infers_dimensions_and_fps_from_input_filename_without_format() {
        let command = parse_words(&[
            "ff",
            "encode",
            "RaceHorses_416x240_30.yuv",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(
            args.video,
            Some(VideoSpec {
                width: 416,
                height: 240,
                pixel_format: None
            })
        );
        assert_eq!(args.fps.as_deref(), Some("30"));
    }

    #[test]
    fn infers_ctc_style_fps_from_input_filename() {
        let command = parse_words(&[
            "ff",
            "encode",
            "MotorCycle_SDR_640x360p2997_yuv444p.y4m",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(args.fps.as_deref(), Some("29.97"));
        assert_eq!(
            args.video,
            Some(VideoSpec {
                width: 640,
                height: 360,
                pixel_format: Some("yuv444p8".to_string())
            })
        );
    }

    #[test]
    fn explicit_input_options_override_filename_metadata() {
        let command = parse_words(&[
            "ff",
            "encode",
            "clip_416x240_30_1f_yuv420p8.yuv",
            "--video",
            "64x64:yuv444p",
            "--fps",
            "30000/1001",
            "--frames",
            "2",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(
            args.video,
            Some(VideoSpec {
                width: 64,
                height: 64,
                pixel_format: Some("yuv444p8".to_string())
            })
        );
        assert_eq!(args.fps.as_deref(), Some("30000/1001"));
        assert_eq!(args.frames, Some(2));
    }

    #[test]
    fn rejects_malformed_video_spec() {
        let err = parse_words(&[
            "ff",
            "encode",
            "in.yuv",
            "--video",
            "64:yuv444p",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap_err();
        assert_eq!(err, "--video expects WxH:pixfmt, got '64:yuv444p'");
    }

    #[test]
    fn encode_requires_core_io_arguments() {
        let err = parse_words(&["ff", "encode", "--encode", "av2:out.obu"]).unwrap_err();
        assert_eq!(err, "encode requires an input path");
    }

    #[test]
    fn accepts_encode_without_video_spec() {
        let command = parse_words(&["ff", "encode", "in.yuv", "--encode", "av2:out.obu"]).unwrap();

        let Command::Encode(args) = command else {
            panic!("expected encode command");
        };
        assert_eq!(args.input.as_deref(), Some("in.yuv"));
        assert_eq!(args.video, None);
        assert_eq!(args.codec.as_deref(), Some("av2"));
        assert_eq!(args.output.as_deref(), Some("out.obu"));
    }

    #[test]
    fn rejects_encode_endpoint_without_path() {
        let err = parse_words(&["ff", "encode", "in.yuv", "--encode", "av2"]).unwrap_err();
        assert_eq!(err, "--encode expects codec:path, got 'av2'");
    }

    #[test]
    fn rejects_encode_without_encoder_endpoint() {
        let err = parse_words(&["ff", "encode", "in.yuv"]).unwrap_err();
        assert_eq!(err, "encode requires --encode codec:path");
    }

    #[test]
    fn rejects_multiple_encode_inputs() {
        let err = parse_words(&[
            "ff",
            "encode",
            "in.yuv",
            "other.yuv",
            "--encode",
            "av2:out.obu",
        ])
        .unwrap_err();
        assert_eq!(err, "unexpected encode argument 'other.yuv'");
    }

    #[test]
    fn rejects_removed_compatibility_options() {
        for option in [
            "--input-format",
            "--raw-video",
            "--codec",
            "--input",
            "--output",
            "--pix-fmt",
            "--pixel-format",
            "--width",
            "--height",
            "-hgt",
        ] {
            let err = parse_words(&[
                "ff",
                "encode",
                "in.yuv",
                option,
                "value",
                "--encode",
                "av2:out.obu",
            ])
            .unwrap_err();
            assert_eq!(err, format!("unknown encode option '{option}'"));
        }
    }

    #[test]
    fn help_is_owned_by_parser_options() {
        let text = help("test");
        for expected in [
            "ff encode <input>",
            "filename metadata",
            "*_<WxH>[_<fps>][_<frames>f]_<pixfmt>.yuv",
            "--encode <codec:path>",
            "--video <WxH:fmt>",
            "--fps <rate>",
            "-n, --frames <count>",
            "-f, --filter <spec>",
            "--set <key[=value]>",
            "--preset <name>",
        ] {
            assert!(text.contains(expected), "missing help entry: {expected}");
        }
        for removed in [
            "-c, --codec <codec>",
            "-i, --input <path>",
            "-o, --output <path>",
            "--pix-fmt",
            "--pixel-format",
            "--width",
            "--height",
            "--lossless",
            "Compatibility options",
            "--input-format",
            "--raw-video",
        ] {
            assert!(!text.contains(removed), "stale help entry: {removed}");
        }
        assert!(!text.contains("ff pipeline"));
        assert!(!text.contains("--decode"));
        assert!(text.contains("[input-options]"));
        assert!(text.contains("[output-options]"));
    }
}
