use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const MAX_DATAGRAM: usize = 65_535;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Profile {
    loss_basis_points: u32,
    burst_every: u64,
    burst_length: u64,
    duplicate_every: u64,
    reorder_every: u64,
    reorder_delay_ms: u64,
    delay_ms: u64,
    jitter_ms: u64,
    stall_at_ms: u64,
    stall_for_ms: u64,
    max_scheduled: usize,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            loss_basis_points: 0,
            burst_every: 0,
            burst_length: 0,
            duplicate_every: 0,
            reorder_every: 0,
            reorder_delay_ms: 0,
            delay_ms: 0,
            jitter_ms: 0,
            stall_at_ms: 0,
            stall_for_ms: 0,
            max_scheduled: 4096,
        }
    }
}

impl Profile {
    fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        let mut profile = Self::default();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("{}:{}: expected key = value", path.display(), index + 1))?;
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            let number = value.parse::<u64>().map_err(|_| {
                format!(
                    "{}:{}: {key} must be an unsigned integer",
                    path.display(),
                    index + 1
                )
            })?;
            match key {
                "loss_basis_points" => {
                    profile.loss_basis_points = number
                        .try_into()
                        .map_err(|_| "loss_basis_points is too large")?
                }
                "burst_every" => profile.burst_every = number,
                "burst_length" => profile.burst_length = number,
                "duplicate_every" => profile.duplicate_every = number,
                "reorder_every" => profile.reorder_every = number,
                "reorder_delay_ms" => profile.reorder_delay_ms = number,
                "delay_ms" => profile.delay_ms = number,
                "jitter_ms" => profile.jitter_ms = number,
                "stall_at_ms" => profile.stall_at_ms = number,
                "stall_for_ms" => profile.stall_for_ms = number,
                "max_scheduled" => {
                    profile.max_scheduled = number
                        .try_into()
                        .map_err(|_| "max_scheduled is too large")?
                }
                _ => {
                    return Err(format!(
                        "{}:{}: unknown profile key {key}",
                        path.display(),
                        index + 1
                    ));
                }
            }
        }
        if profile.loss_basis_points > 10_000 {
            return Err("loss_basis_points must be <= 10000".into());
        }
        if profile.max_scheduled == 0 {
            return Err("max_scheduled must be greater than zero".into());
        }
        Ok(profile)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    ClientToUpstream,
    UpstreamToClient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Decision {
    drop: bool,
    copies: u8,
    delay_ms: u64,
    reordered: bool,
    stalled: bool,
}

#[derive(Clone, Debug)]
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9e3779b97f4a7c15 } else { seed })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[derive(Debug)]
struct Scheduler {
    profile: Profile,
    rng: DeterministicRng,
    seen: u64,
    burst_remaining: u64,
    queue: VecDeque<Scheduled>,
    stats: Stats,
}

#[derive(Debug)]
struct Scheduled {
    due_ms: u64,
    direction: Direction,
    bytes: Vec<u8>,
    client: SocketAddr,
}

#[derive(Debug, Default)]
struct Stats {
    received: u64,
    forwarded: u64,
    dropped: u64,
    duplicated: u64,
    reordered: u64,
    stalled: u64,
    queue_overflow: u64,
    max_queue: usize,
}

impl Scheduler {
    fn new(profile: Profile, seed: u64) -> Self {
        Self {
            profile,
            rng: DeterministicRng::new(seed),
            seen: 0,
            burst_remaining: 0,
            queue: VecDeque::new(),
            stats: Stats::default(),
        }
    }

    fn decide(&mut self, now_ms: u64) -> Decision {
        self.seen += 1;
        let stalled = self.profile.stall_for_ms > 0
            && now_ms >= self.profile.stall_at_ms
            && now_ms
                < self
                    .profile
                    .stall_at_ms
                    .saturating_add(self.profile.stall_for_ms);
        if stalled {
            return Decision {
                drop: true,
                copies: 0,
                delay_ms: 0,
                reordered: false,
                stalled: true,
            };
        }
        if self.burst_remaining == 0
            && self.profile.burst_every > 0
            && self.seen % self.profile.burst_every == 0
        {
            self.burst_remaining = self.profile.burst_length;
        }
        if self.burst_remaining > 0 {
            self.burst_remaining -= 1;
            return Decision {
                drop: true,
                copies: 0,
                delay_ms: 0,
                reordered: false,
                stalled: false,
            };
        }
        if self.profile.loss_basis_points > 0
            && self.rng.next() % 10_000 < u64::from(self.profile.loss_basis_points)
        {
            return Decision {
                drop: true,
                copies: 0,
                delay_ms: 0,
                reordered: false,
                stalled: false,
            };
        }
        let duplicate =
            self.profile.duplicate_every > 0 && self.seen % self.profile.duplicate_every == 0;
        let reordered =
            self.profile.reorder_every > 0 && self.seen % self.profile.reorder_every == 0;
        let jitter = if self.profile.jitter_ms == 0 {
            0
        } else {
            self.rng.next() % (self.profile.jitter_ms + 1)
        };
        Decision {
            drop: false,
            copies: if duplicate { 2 } else { 1 },
            delay_ms: self.profile.delay_ms
                + jitter
                + if reordered {
                    self.profile.reorder_delay_ms
                } else {
                    0
                },
            reordered,
            stalled: false,
        }
    }

    fn admit(&mut self, now_ms: u64, direction: Direction, bytes: &[u8], client: SocketAddr) {
        self.stats.received += 1;
        let decision = self.decide(now_ms);
        if decision.drop {
            self.stats.dropped += 1;
            if decision.stalled {
                self.stats.stalled += 1;
            }
            return;
        }
        if decision.copies == 2 {
            self.stats.duplicated += 1;
        }
        if decision.reordered {
            self.stats.reordered += 1;
        }
        for copy in 0..decision.copies {
            if self.queue.len() >= self.profile.max_scheduled {
                self.stats.queue_overflow += 1;
                self.stats.dropped += 1;
                continue;
            }
            let delay = decision.delay_ms + u64::from(copy);
            let item = Scheduled {
                due_ms: now_ms.saturating_add(delay),
                direction,
                bytes: bytes.to_vec(),
                client,
            };
            let position = self
                .queue
                .iter()
                .position(|queued| queued.due_ms > item.due_ms)
                .unwrap_or(self.queue.len());
            self.queue.insert(position, item);
            self.stats.max_queue = self.stats.max_queue.max(self.queue.len());
        }
    }

    fn pop_due(&mut self, now_ms: u64) -> Option<Scheduled> {
        if self.queue.front().is_some_and(|item| item.due_ms <= now_ms) {
            self.queue.pop_front()
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct Options {
    listen: SocketAddr,
    upstream: SocketAddr,
    profile: PathBuf,
    seed: u64,
    metrics: Option<PathBuf>,
}

fn parse_options() -> Result<Options, String> {
    let mut args = env::args().skip(1);
    let mut listen = None;
    let mut upstream = None;
    let mut profile = None;
    let mut seed = 1;
    let mut metrics = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} requires a value"));
        match arg.as_str() {
            "--listen" => {
                listen = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--listen must be a socket address")?,
                )
            }
            "--upstream" => {
                upstream = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--upstream must be a socket address")?,
                )
            }
            "--profile" => profile = Some(PathBuf::from(value()?)),
            "--seed" => {
                seed = value()?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer")?
            }
            "--metrics" => metrics = Some(PathBuf::from(value()?)),
            "--help" | "-h" => return Err(usage().into()),
            _ => return Err(format!("unknown argument {arg}\n{}", usage())),
        }
    }
    Ok(Options {
        listen: listen.ok_or("--listen is required")?,
        upstream: upstream.ok_or("--upstream is required")?,
        profile: profile.ok_or("--profile is required")?,
        seed,
        metrics,
    })
}

fn usage() -> &'static str {
    "usage: cultnet-impair --listen HOST:PORT --upstream HOST:PORT --profile PATH [--seed N] [--metrics PATH]"
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn write_metrics(path: &Path, stats: &Stats) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    writeln!(
        file,
        "received,forwarded,dropped,duplicated,reordered,stalled,queue_overflow,max_queue"
    )?;
    writeln!(
        file,
        "{},{},{},{},{},{},{},{}",
        stats.received,
        stats.forwarded,
        stats.dropped,
        stats.duplicated,
        stats.reordered,
        stats.stalled,
        stats.queue_overflow,
        stats.max_queue
    )
}

fn run(options: Options) -> Result<(), String> {
    let profile = Profile::load(&options.profile)?;
    let client_socket = UdpSocket::bind(options.listen)
        .map_err(|error| format!("binding {}: {error}", options.listen))?;
    let upstream_socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|error| format!("binding upstream socket: {error}"))?;
    upstream_socket
        .connect(options.upstream)
        .map_err(|error| format!("connecting {}: {error}", options.upstream))?;
    client_socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    upstream_socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    println!(
        "cultnet-impair listen={} upstream={} seed={} profile={}",
        options.listen,
        options.upstream,
        options.seed,
        options.profile.display()
    );
    let start = Instant::now();
    let mut scheduler = Scheduler::new(profile, options.seed);
    let mut client = None;
    let mut buffer = vec![0; MAX_DATAGRAM];
    loop {
        let now = elapsed_ms(start);
        match client_socket.recv_from(&mut buffer) {
            Ok((size, address)) => {
                client = Some(address);
                scheduler.admit(now, Direction::ClientToUpstream, &buffer[..size], address);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
        match upstream_socket.recv(&mut buffer) {
            Ok(size) => {
                if let Some(address) = client {
                    scheduler.admit(now, Direction::UpstreamToClient, &buffer[..size], address);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
        while let Some(item) = scheduler.pop_due(now) {
            let result = match item.direction {
                Direction::ClientToUpstream => upstream_socket.send(&item.bytes),
                Direction::UpstreamToClient => client_socket.send_to(&item.bytes, item.client),
            };
            if result.is_ok() {
                scheduler.stats.forwarded += 1;
            }
        }
        if let Some(path) = options.metrics.as_deref() {
            if now % 1000 == 0 {
                let _ = write_metrics(path, &scheduler.stats);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    match parse_options().and_then(run) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn client() -> SocketAddr {
        "127.0.0.1:1234".parse().unwrap()
    }

    #[test]
    fn iid_decisions_are_seed_reproducible() {
        let profile = Profile {
            loss_basis_points: 2500,
            ..Default::default()
        };
        let mut a = Scheduler::new(profile.clone(), 42);
        let mut b = Scheduler::new(profile, 42);
        let left: Vec<_> = (0..100).map(|_| a.decide(0)).collect();
        let right: Vec<_> = (0..100).map(|_| b.decide(0)).collect();
        assert_eq!(left, right);
        assert!(left.iter().any(|d| d.drop));
        assert!(left.iter().any(|d| !d.drop));
    }

    #[test]
    fn burst_drop_is_exact() {
        let profile = Profile {
            burst_every: 5,
            burst_length: 3,
            ..Default::default()
        };
        let mut scheduler = Scheduler::new(profile, 1);
        let drops: Vec<_> = (1..=9).filter(|_| scheduler.decide(0).drop).collect();
        assert_eq!(drops.len(), 3);
    }

    #[test]
    fn duplicate_delay_and_reorder_are_deterministic() {
        let profile = Profile {
            duplicate_every: 2,
            reorder_every: 3,
            reorder_delay_ms: 20,
            delay_ms: 5,
            ..Default::default()
        };
        let mut scheduler = Scheduler::new(profile, 1);
        assert_eq!(scheduler.decide(0).copies, 1);
        assert_eq!(scheduler.decide(0).copies, 2);
        let third = scheduler.decide(0);
        assert!(third.reordered);
        assert_eq!(third.delay_ms, 25);
    }

    #[test]
    fn stall_drops_only_inside_window() {
        let profile = Profile {
            stall_at_ms: 100,
            stall_for_ms: 50,
            ..Default::default()
        };
        let mut scheduler = Scheduler::new(profile, 1);
        assert!(!scheduler.decide(99).drop);
        let stalled = scheduler.decide(100);
        assert!(stalled.drop && stalled.stalled);
        assert!(scheduler.decide(149).drop);
        assert!(!scheduler.decide(150).drop);
    }

    #[test]
    fn reorder_delay_places_packet_behind_later_packet() {
        let profile = Profile {
            reorder_every: 2,
            reorder_delay_ms: 20,
            ..Default::default()
        };
        let mut scheduler = Scheduler::new(profile, 1);
        scheduler.admit(0, Direction::ClientToUpstream, b"first", client());
        scheduler.admit(0, Direction::ClientToUpstream, b"held", client());
        scheduler.admit(1, Direction::ClientToUpstream, b"later", client());
        assert_eq!(scheduler.pop_due(1).unwrap().bytes, b"first");
        assert_eq!(scheduler.pop_due(1).unwrap().bytes, b"later");
        assert!(scheduler.pop_due(19).is_none());
        assert_eq!(scheduler.pop_due(20).unwrap().bytes, b"held");
    }

    #[test]
    fn scheduled_queue_is_bounded_and_counts_overflow() {
        let profile = Profile {
            delay_ms: 100,
            max_scheduled: 2,
            ..Default::default()
        };
        let mut scheduler = Scheduler::new(profile, 1);
        for value in 0..5 {
            scheduler.admit(0, Direction::ClientToUpstream, &[value], client());
        }
        assert_eq!(scheduler.queue.len(), 2);
        assert_eq!(scheduler.stats.max_queue, 2);
        assert_eq!(scheduler.stats.queue_overflow, 3);
        assert_eq!(scheduler.stats.dropped, 3);
    }
}
