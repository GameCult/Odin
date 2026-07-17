#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::{env, thread, time::Duration};

    let count = env::args()
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(100);
    let pressed_ms = env::args()
        .nth(2)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(8);
    let gap_ms = env::args()
        .nth(3)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(92);

    let client = vigem_client::Client::connect().context("connecting to ViGEmBus")?;
    let mut target =
        vigem_client::Xbox360Wired::new(client, vigem_client::TargetId::XBOX360_WIRED);
    target.plugin().context("plugging quick-tap fixture")?;
    target.wait_ready().context("waiting for quick-tap fixture")?;

    let neutral = vigem_client::XGamepad::default();
    let pressed = vigem_client::XGamepad {
        buttons: vigem_client::XButtons::from(vigem_client::XButtons::A),
        ..Default::default()
    };
    target.update(&neutral).context("neutralizing fixture")?;
    println!("fixture_ready count={count} pressed_ms={pressed_ms} gap_ms={gap_ms}");
    // Leave enough time for Muninn's dynamic XInput discovery and Sleipnir's
    // subscription handshake before the first non-replaceable edge.
    thread::sleep(Duration::from_secs(15));

    for sequence in 1..=count {
        target.update(&pressed).context("pressing fixture A")?;
        thread::sleep(Duration::from_millis(pressed_ms));
        target.update(&neutral).context("releasing fixture A")?;
        println!("fixture_tap sequence={sequence}");
        thread::sleep(Duration::from_millis(gap_ms));
    }

    thread::sleep(Duration::from_secs(3));
    target.update(&neutral).context("final fixture neutral")?;
    target.unplug().context("unplugging quick-tap fixture")?;
    println!("fixture_complete count={count}");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("vigem_quick_tap_fixture is Windows-only");
    std::process::exit(2);
}
