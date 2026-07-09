use std::ffi::OsString;
use std::process::ExitCode;

use frameforge_core::VERSION;

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

    eprintln!("error: codec '{codec_name}' is compiled as a CLI scaffold, but encoding is not implemented yet");
    ExitCode::from(4)
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
    None
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
