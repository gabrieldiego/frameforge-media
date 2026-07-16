use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{AV2_MI_SIZE, AV2_MVP_SUPERBLOCK_SIZE};

const REPORT_ENV: &str = "FRAMEFORGE_AV2_SB_BITS";

static CURRENT_FRAME: AtomicUsize = AtomicUsize::new(0);
static REPORT: OnceLock<Option<Mutex<Av2SbBitReport>>> = OnceLock::new();

pub(crate) fn set_current_frame(frame_index: usize) {
    CURRENT_FRAME.store(frame_index, Ordering::Relaxed);
}

#[derive(Debug)]
struct Av2SbBitReport {
    destination: Av2SbBitReportDestination,
}

#[derive(Debug)]
enum Av2SbBitReportDestination {
    File(BufWriter<std::fs::File>),
    Stderr,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum Av2SbBitCategory {
    Partition,
    LumaMode,
    ChromaMode,
    Residual,
    Intrabc,
    Inter,
    Palette,
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct Av2SbBitCollector {
    path: &'static str,
    frame_index: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    tile_width: usize,
    tile_height: usize,
    sb_cols: usize,
    costs: Vec<Av2SbBitCost>,
}

#[derive(Debug, Clone, Default)]
struct Av2SbBitCost {
    sb_x: usize,
    sb_y: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    partition_bits: usize,
    luma_mode_bits: usize,
    chroma_mode_bits: usize,
    residual_bits: usize,
    intrabc_bits: usize,
    inter_bits: usize,
    palette_bits: usize,
    other_bits: usize,
    decision_count: usize,
    leaf_count: usize,
}

impl Av2SbBitCollector {
    pub(crate) fn new(
        path: &'static str,
        tile_origin_x: usize,
        tile_origin_y: usize,
        tile_width: usize,
        tile_height: usize,
    ) -> Self {
        let sb_cols = tile_width.div_ceil(AV2_MVP_SUPERBLOCK_SIZE);
        let sb_rows = tile_height.div_ceil(AV2_MVP_SUPERBLOCK_SIZE);
        let mut costs = Vec::with_capacity(sb_cols * sb_rows);
        for sb_row in 0..sb_rows {
            for sb_col in 0..sb_cols {
                let x = tile_origin_x + sb_col * AV2_MVP_SUPERBLOCK_SIZE;
                let y = tile_origin_y + sb_row * AV2_MVP_SUPERBLOCK_SIZE;
                let visible_right = tile_origin_x + tile_width;
                let visible_bottom = tile_origin_y + tile_height;
                costs.push(Av2SbBitCost {
                    sb_x: x / AV2_MVP_SUPERBLOCK_SIZE,
                    sb_y: y / AV2_MVP_SUPERBLOCK_SIZE,
                    x,
                    y,
                    width: AV2_MVP_SUPERBLOCK_SIZE.min(visible_right - x),
                    height: AV2_MVP_SUPERBLOCK_SIZE.min(visible_bottom - y),
                    ..Av2SbBitCost::default()
                });
            }
        }
        Self {
            path,
            frame_index: CURRENT_FRAME.load(Ordering::Relaxed),
            tile_origin_x,
            tile_origin_y,
            tile_width,
            tile_height,
            sb_cols,
            costs,
        }
    }

    pub(crate) fn record(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        before_bits: usize,
        after_bits: usize,
        category: Av2SbBitCategory,
        leaf: bool,
    ) {
        let delta = after_bits.saturating_sub(before_bits);
        let sb_col = (col_mi * AV2_MI_SIZE) / AV2_MVP_SUPERBLOCK_SIZE;
        let sb_row = (row_mi * AV2_MI_SIZE) / AV2_MVP_SUPERBLOCK_SIZE;
        let index = sb_row
            .checked_mul(self.sb_cols)
            .and_then(|base| base.checked_add(sb_col));
        let Some(cost) = index.and_then(|index| self.costs.get_mut(index)) else {
            return;
        };
        match category {
            Av2SbBitCategory::Partition => cost.partition_bits += delta,
            Av2SbBitCategory::LumaMode => cost.luma_mode_bits += delta,
            Av2SbBitCategory::ChromaMode => cost.chroma_mode_bits += delta,
            Av2SbBitCategory::Residual => cost.residual_bits += delta,
            Av2SbBitCategory::Intrabc => cost.intrabc_bits += delta,
            Av2SbBitCategory::Inter => cost.inter_bits += delta,
            Av2SbBitCategory::Palette => cost.palette_bits += delta,
            Av2SbBitCategory::Other => cost.other_bits += delta,
        }
        cost.decision_count += 1;
        if leaf {
            cost.leaf_count += 1;
        }
    }

    pub(crate) fn flush_if_enabled(&self) {
        let Some(report) = report() else {
            return;
        };
        let mut report = match report.lock() {
            Ok(report) => report,
            Err(poisoned) => poisoned.into_inner(),
        };
        for cost in &self.costs {
            if let Err(err) = report.write_line(self, cost) {
                eprintln!("failed to write {REPORT_ENV} report: {err}");
                return;
            }
        }
        if let Err(err) = report.flush() {
            eprintln!("failed to flush {REPORT_ENV} report: {err}");
        }
    }
}

impl Av2SbBitCost {
    fn total_bits(&self) -> usize {
        self.partition_bits
            + self.luma_mode_bits
            + self.chroma_mode_bits
            + self.residual_bits
            + self.intrabc_bits
            + self.inter_bits
            + self.palette_bits
            + self.other_bits
    }
}

impl Av2SbBitReport {
    fn write_line(&mut self, collector: &Av2SbBitCollector, cost: &Av2SbBitCost) -> io::Result<()> {
        let line = format!(
            "{{\"codec\":\"av2\",\"source\":\"frameforge\",\"path\":\"{}\",\"frame_index\":{},\"tile_origin_x\":{},\"tile_origin_y\":{},\"tile_width\":{},\"tile_height\":{},\"sb_x\":{},\"sb_y\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"partition_bits\":{},\"luma_mode_bits\":{},\"chroma_mode_bits\":{},\"residual_bits\":{},\"intrabc_bits\":{},\"inter_bits\":{},\"palette_bits\":{},\"other_bits\":{},\"total_symbol_bits\":{},\"decision_count\":{},\"leaf_count\":{}}}\n",
            collector.path,
            collector.frame_index,
            collector.tile_origin_x,
            collector.tile_origin_y,
            collector.tile_width,
            collector.tile_height,
            cost.sb_x,
            cost.sb_y,
            cost.x,
            cost.y,
            cost.width,
            cost.height,
            cost.partition_bits,
            cost.luma_mode_bits,
            cost.chroma_mode_bits,
            cost.residual_bits,
            cost.intrabc_bits,
            cost.inter_bits,
            cost.palette_bits,
            cost.other_bits,
            cost.total_bits(),
            cost.decision_count,
            cost.leaf_count,
        );
        match &mut self.destination {
            Av2SbBitReportDestination::File(file) => file.write_all(line.as_bytes()),
            Av2SbBitReportDestination::Stderr => std::io::stderr().write_all(line.as_bytes()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.destination {
            Av2SbBitReportDestination::File(file) => file.flush(),
            Av2SbBitReportDestination::Stderr => std::io::stderr().flush(),
        }
    }
}

fn report() -> Option<&'static Mutex<Av2SbBitReport>> {
    REPORT
        .get_or_init(|| {
            let value = std::env::var_os(REPORT_ENV)?;
            if value == "0" {
                return None;
            }
            if value == "-" {
                return Some(Mutex::new(Av2SbBitReport {
                    destination: Av2SbBitReportDestination::Stderr,
                }));
            }
            let file = match OpenOptions::new().create(true).append(true).open(&value) {
                Ok(file) => file,
                Err(err) => {
                    eprintln!(
                        "failed to open {REPORT_ENV} destination '{}': {err}",
                        value.to_string_lossy()
                    );
                    return None;
                }
            };
            Some(Mutex::new(Av2SbBitReport {
                destination: Av2SbBitReportDestination::File(BufWriter::new(file)),
            }))
        })
        .as_ref()
}
