#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::c_int;

pub const MUNINN_MOVE_TRACKER_OK: c_int = 0;
pub const MUNINN_MOVE_TRACKER_NULL: c_int = -1;
pub const MUNINN_MOVE_TRACKER_INVALID: c_int = -2;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveTrackerConfig {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub tile_size: u32,
    pub threshold_min: u8,
    pub min_area_px: u32,
    pub max_candidates: u32,
    pub source_id_hash: u64,
    pub frame_sequence: u64,
}

impl Default for MoveTrackerConfig {
    fn default() -> Self {
        Self {
            width: 320,
            height: 240,
            stride_bytes: 320,
            tile_size: 16,
            threshold_min: 180,
            min_area_px: 4,
            max_candidates: 64,
            source_id_hash: 0,
            frame_sequence: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveTrackerDispatchPlan {
    pub group_size_x: u32,
    pub group_size_y: u32,
    pub groups_x: u32,
    pub groups_y: u32,
    pub max_candidates: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveMarkerCandidate {
    pub source_id_hash: u64,
    pub frame_sequence: u64,
    pub tile_x: u32,
    pub tile_y: u32,
    pub center_x_px: f32,
    pub center_y_px: f32,
    pub radius_px: f32,
    pub area_px: u32,
    pub mean_luma: f32,
    pub peak_luma: u32,
    pub score: f32,
}

pub fn dispatch_plan(config: MoveTrackerConfig) -> Option<MoveTrackerDispatchPlan> {
    if !config_is_valid(config) {
        return None;
    }

    let tile = config.tile_size;
    Some(MoveTrackerDispatchPlan {
        group_size_x: tile,
        group_size_y: tile,
        groups_x: config.width.div_ceil(tile),
        groups_y: config.height.div_ceil(tile),
        max_candidates: config.max_candidates,
    })
}

pub fn extract_luma_candidates(
    frame: &[u8],
    config: MoveTrackerConfig,
) -> Option<Vec<MoveMarkerCandidate>> {
    if !config_is_valid(config) {
        return None;
    }

    let required = required_frame_bytes(config)?;
    if frame.len() < required {
        return None;
    }

    let mut candidates = Vec::new();
    let tile = config.tile_size;
    let groups_x = config.width.div_ceil(tile);
    let groups_y = config.height.div_ceil(tile);

    for tile_y in 0..groups_y {
        for tile_x in 0..groups_x {
            if candidates.len() >= config.max_candidates as usize {
                return Some(candidates);
            }

            if let Some(candidate) = extract_tile(frame, config, tile_x, tile_y) {
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    Some(candidates)
}

fn extract_tile(
    frame: &[u8],
    config: MoveTrackerConfig,
    tile_x: u32,
    tile_y: u32,
) -> Option<MoveMarkerCandidate> {
    let x0 = tile_x * config.tile_size;
    let y0 = tile_y * config.tile_size;
    let x1 = (x0 + config.tile_size).min(config.width);
    let y1 = (y0 + config.tile_size).min(config.height);
    let threshold = u32::from(config.threshold_min);
    let mut area = 0u32;
    let mut sum_x = 0u64;
    let mut sum_y = 0u64;
    let mut sum_w = 0u64;
    let mut peak = 0u32;

    for y in y0..y1 {
        let row = y as usize * config.stride_bytes as usize;
        for x in x0..x1 {
            let luma = u32::from(frame[row + x as usize]);
            if luma < threshold {
                continue;
            }

            area += 1;
            peak = peak.max(luma);
            let weight = u64::from(luma - threshold + 1);
            sum_x += u64::from(x) * weight;
            sum_y += u64::from(y) * weight;
            sum_w += weight;
        }
    }

    if area < config.min_area_px || sum_w == 0 {
        return None;
    }

    let center_x = sum_x as f32 / sum_w as f32;
    let center_y = sum_y as f32 / sum_w as f32;
    let radius = (area as f32 / core::f32::consts::PI).sqrt();
    let mean_luma = threshold as f32 + ((sum_w as f32 / area as f32) - 1.0);
    let area_score = (area as f32 / (config.tile_size * config.tile_size) as f32).min(1.0);
    let brightness_score = peak as f32 / 255.0;

    Some(MoveMarkerCandidate {
        source_id_hash: config.source_id_hash,
        frame_sequence: config.frame_sequence,
        tile_x,
        tile_y,
        center_x_px: center_x,
        center_y_px: center_y,
        radius_px: radius,
        area_px: area,
        mean_luma,
        peak_luma: peak,
        score: brightness_score * (0.65 + area_score * 0.35),
    })
}

fn config_is_valid(config: MoveTrackerConfig) -> bool {
    config.width > 0
        && config.height > 0
        && config.stride_bytes >= config.width
        && config.tile_size == 16
        && config.min_area_px > 0
        && config.max_candidates > 0
}

fn required_frame_bytes(config: MoveTrackerConfig) -> Option<usize> {
    let rows = usize::try_from(config.height).ok()?;
    let stride = usize::try_from(config.stride_bytes).ok()?;
    rows.checked_mul(stride)
}

#[unsafe(no_mangle)]
pub extern "C" fn muninn_move_tracker_dispatch_plan(
    config: MoveTrackerConfig,
    out_plan: *mut MoveTrackerDispatchPlan,
) -> c_int {
    if out_plan.is_null() {
        return MUNINN_MOVE_TRACKER_NULL;
    }

    let Some(plan) = dispatch_plan(config) else {
        return MUNINN_MOVE_TRACKER_INVALID;
    };

    unsafe {
        *out_plan = plan;
    }
    MUNINN_MOVE_TRACKER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn muninn_move_tracker_extract_luma_cpu(
    config: MoveTrackerConfig,
    frame: *const u8,
    frame_len: usize,
    out_candidates: *mut MoveMarkerCandidate,
    out_capacity: usize,
    out_count: *mut usize,
) -> c_int {
    if frame.is_null() || out_candidates.is_null() || out_count.is_null() {
        return MUNINN_MOVE_TRACKER_NULL;
    }

    let frame = unsafe { core::slice::from_raw_parts(frame, frame_len) };
    let Some(candidates) = extract_luma_candidates(frame, config) else {
        return MUNINN_MOVE_TRACKER_INVALID;
    };

    let written = candidates.len().min(out_capacity);
    unsafe {
        core::ptr::copy_nonoverlapping(candidates.as_ptr(), out_candidates, written);
        *out_count = written;
    }
    MUNINN_MOVE_TRACKER_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_plan_covers_partial_tiles() {
        let config = MoveTrackerConfig {
            width: 33,
            height: 17,
            stride_bytes: 40,
            tile_size: 16,
            ..MoveTrackerConfig::default()
        };

        let plan = dispatch_plan(config).expect("valid plan");
        assert_eq!(3, plan.groups_x);
        assert_eq!(2, plan.groups_y);
        assert_eq!(16, plan.group_size_x);
        assert_eq!(16, plan.group_size_y);
    }

    #[test]
    fn extracts_weighted_marker_candidate() {
        let config = MoveTrackerConfig {
            width: 32,
            height: 24,
            stride_bytes: 32,
            tile_size: 16,
            threshold_min: 180,
            min_area_px: 3,
            max_candidates: 8,
            source_id_hash: 42,
            frame_sequence: 7,
        };
        let mut frame = vec![0u8; (config.stride_bytes * config.height) as usize];
        for (x, y, luma) in [
            (10u32, 8u32, 230u8),
            (11, 8, 250),
            (10, 9, 240),
            (11, 9, 255),
        ] {
            frame[(y * config.stride_bytes + x) as usize] = luma;
        }

        let candidates = extract_luma_candidates(&frame, config).expect("candidates");
        assert_eq!(1, candidates.len());
        let candidate = candidates[0];
        assert_eq!(42, candidate.source_id_hash);
        assert_eq!(7, candidate.frame_sequence);
        assert_eq!(0, candidate.tile_x);
        assert_eq!(0, candidate.tile_y);
        assert_eq!(4, candidate.area_px);
        assert_eq!(255, candidate.peak_luma);
        assert!(candidate.center_x_px > 10.4 && candidate.center_x_px < 10.7);
        assert!(candidate.center_y_px > 8.4 && candidate.center_y_px < 8.7);
        assert!(candidate.score > 0.65);
    }

    #[test]
    fn rejects_short_frames_and_empty_config() {
        assert!(extract_luma_candidates(&[], MoveTrackerConfig::default()).is_none());
        assert!(
            dispatch_plan(MoveTrackerConfig {
                width: 0,
                ..MoveTrackerConfig::default()
            })
            .is_none()
        );
    }
}
