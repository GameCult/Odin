#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::{
        env, fs, thread,
        time::{Duration, Instant},
    };

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct XInputGamepad {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        thumb_lx: i16,
        thumb_ly: i16,
        thumb_rx: i16,
        thumb_ry: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct XInputState {
        packet_number: u32,
        gamepad: XInputGamepad,
    }

    #[link(name = "xinput")]
    unsafe extern "system" {
        fn XInputGetState(user_index: u32, state: *mut XInputState) -> u32;
    }

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period_ms: u32) -> u32;
        fn timeEndPeriod(period_ms: u32) -> u32;
    }

    struct TimerResolution;
    impl Drop for TimerResolution {
        fn drop(&mut self) {
            unsafe {
                let _ = timeEndPeriod(1);
            }
        }
    }

    let index = env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let duration_seconds = env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let output = env::args()
        .nth(3)
        .context("usage: xinput_edge_observer INDEX SECONDS OUTPUT")?;
    unsafe {
        let _ = timeBeginPeriod(1);
    }
    let _timer_resolution = TimerResolution;

    let started = Instant::now();
    let deadline = started + Duration::from_secs(duration_seconds);
    let mut next_sample = started;
    let mut last_sample = started;
    let mut max_sample_gap = Duration::ZERO;
    let mut connected_samples = 0_u64;
    let mut presses = 0_u64;
    let mut releases = 0_u64;
    let mut seen = false;
    let mut last_pressed = false;
    let mut press_started = None;
    let mut durations_ms = Vec::new();

    while Instant::now() < deadline {
        let now = Instant::now();
        max_sample_gap = max_sample_gap.max(now.saturating_duration_since(last_sample));
        last_sample = now;
        let mut state = XInputState::default();
        if unsafe { XInputGetState(index, &mut state) } == 0 {
            connected_samples += 1;
            let pressed = state.gamepad.buttons & 0x1000 != 0;
            if seen && pressed && !last_pressed {
                presses += 1;
                press_started = Some(now);
            }
            if seen && !pressed && last_pressed {
                releases += 1;
                if let Some(pressed_at) = press_started.take() {
                    durations_ms
                        .push(now.saturating_duration_since(pressed_at).as_secs_f64() * 1000.0);
                }
            }
            last_pressed = pressed;
            seen = true;
        }
        next_sample += Duration::from_millis(1);
        let now = Instant::now();
        if next_sample > now + Duration::from_micros(250) {
            thread::sleep(next_sample - now - Duration::from_micros(250));
        }
        while Instant::now() < next_sample {
            std::hint::spin_loop();
        }
        if Instant::now().saturating_duration_since(next_sample) > Duration::from_millis(5) {
            next_sample = Instant::now();
        }
    }

    let durations = durations_ms
        .iter()
        .map(|v| format!("{v:.3}"))
        .collect::<Vec<_>>()
        .join(",");
    let result = format!(
        "presses={presses} releases={releases} connected_samples={connected_samples} final_pressed={last_pressed} max_sample_gap_us={} durations_ms={durations}\n",
        max_sample_gap.as_micros()
    );
    fs::write(&output, &result).with_context(|| format!("writing {output}"))?;
    print!("{result}");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("xinput_edge_observer is Windows-only");
    std::process::exit(2);
}
