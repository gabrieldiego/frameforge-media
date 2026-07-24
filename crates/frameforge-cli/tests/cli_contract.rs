#![cfg_attr(
    not(all(feature = "codec-av2", feature = "filter-identity")),
    allow(unused_imports)
)]

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(all(feature = "codec-av2", feature = "filter-identity"))]
fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_nanos();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let dir = workspace_root
        .join("target/frameforge-cli-contract")
        .join(format!("{name}_{unique}"));
    fs::create_dir_all(&dir).expect("create test directory");
    dir
}

#[cfg(all(feature = "codec-av2", feature = "filter-identity"))]
fn ff() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ff"))
}

#[cfg(all(feature = "codec-av2", feature = "filter-identity"))]
fn yuv420p8_frame_len(width: usize, height: usize) -> usize {
    width * height * 3 / 2
}

#[cfg(all(feature = "codec-av2", feature = "filter-identity"))]
#[test]
fn identity_filter_encodes_file_input_and_preserves_lossless_recon() {
    let dir = temp_dir("identity_file");
    let input = dir.join("clip_16x16_30_1f_yuv420p8.yuv");
    let output = dir.join("out.obu");
    let recon = dir.join("recon.yuv");
    let frame = vec![0u8; yuv420p8_frame_len(16, 16)];
    File::create(&input)
        .expect("create input")
        .write_all(&frame)
        .expect("write input");

    let result = Command::new(ff())
        .args([
            "encode",
            input.to_str().expect("input path utf8"),
            "--filter",
            "identity",
            "--encode",
            &format!("av2:{}", output.display()),
            "--recon",
            recon.to_str().expect("recon path utf8"),
            "--set",
            "lossless",
        ])
        .output()
        .expect("run ff encode");

    assert!(
        result.status.success(),
        "ff failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.metadata().expect("output metadata").len() > 0);
    assert_eq!(fs::read(recon).expect("read recon"), frame);
}

#[cfg(all(feature = "codec-av2", feature = "filter-identity"))]
#[test]
fn identity_filter_runs_after_pattern_source() {
    let dir = temp_dir("identity_pattern");
    let output = dir.join("pattern.obu");
    let recon = dir.join("pattern_recon.yuv");

    let result = Command::new(ff())
        .args([
            "encode",
            "--filter",
            "pattern=black",
            "--filter",
            "identity",
            "--video",
            "16x16:yuv420p8",
            "--frames",
            "1",
            "--encode",
            &format!("av2:{}", output.display()),
            "--recon",
            recon.to_str().expect("recon path utf8"),
            "--set",
            "lossless",
        ])
        .output()
        .expect("run ff encode");

    assert!(
        result.status.success(),
        "ff failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.metadata().expect("output metadata").len() > 0);
    assert_eq!(
        fs::read(recon).expect("read recon"),
        vec![0u8; yuv420p8_frame_len(16, 16)]
    );
}

#[cfg(all(
    feature = "codec-av2",
    feature = "filter-crop",
    feature = "filter-identity"
))]
#[test]
fn crop_filter_reports_explicit_scaffold_error() {
    let dir = temp_dir("crop_scaffold");
    let input = dir.join("clip_16x16_30_1f_yuv420p8.yuv");
    let output = dir.join("out.obu");
    File::create(&input)
        .expect("create input")
        .write_all(&vec![0u8; yuv420p8_frame_len(16, 16)])
        .expect("write input");

    let result = Command::new(ff())
        .args([
            "encode",
            input.to_str().expect("input path utf8"),
            "--filter",
            "crop=x=0:y=0:w=8:h=8",
            "--encode",
            &format!("av2:{}", output.display()),
        ])
        .output()
        .expect("run ff encode");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("discovery scaffold but execution is not implemented"),
        "{stderr}"
    );
}
