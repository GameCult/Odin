use anyhow::Result;
use anyhow::anyhow;
use std::io::Read;
use std::io::Write;

use crate::CultNetReconnectPolicy;
use crate::CultNetTransportChannel;
use crate::CultNetTransportDelivery;
use crate::CultNetTransportDescriptor;
use crate::CultNetTransportOrdering;
use crate::CultNetTransportProfile;
use crate::CultNetTransportProtocol;
use crate::FRAME_HEADER_BYTES;
use crate::encode_frame;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CultNetTransportStats {
    pub bytes_received: u64,
    pub bytes_sent: u64,
    pub frames_received: u64,
    pub frames_sent: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetTransportFrame {
    pub channel_id: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CultNetReconnectPolicyOptions {
    pub policy_id: String,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_jitter_ms: u64,
    pub max_attempts: Option<u32>,
}

impl Default for CultNetReconnectPolicyOptions {
    fn default() -> Self {
        Self {
            policy_id: "default".to_string(),
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
            max_jitter_ms: 250,
            max_attempts: None,
        }
    }
}

pub fn create_reconnect_policy(options: CultNetReconnectPolicyOptions) -> CultNetReconnectPolicy {
    CultNetReconnectPolicy {
        schema_version: "cultnet.reconnect_policy.v0".to_string(),
        policy_id: if options.policy_id.trim().is_empty() {
            "default".to_string()
        } else {
            options.policy_id
        },
        base_delay_ms: options.base_delay_ms,
        max_delay_ms: options.max_delay_ms,
        max_jitter_ms: options.max_jitter_ms,
        max_attempts: options.max_attempts,
    }
}

pub fn compute_reconnect_delay_ms(
    policy: &CultNetReconnectPolicy,
    attempt: u32,
    jitter_ms: u64,
) -> u64 {
    let normalized_attempt = attempt.max(1);
    let multiplier = 2_u64.saturating_pow(normalized_attempt.saturating_sub(1));
    let capped_base_delay = policy
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(policy.max_delay_ms);
    capped_base_delay + jitter_ms.min(policy.max_jitter_ms)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetReconnectDecision {
    pub attempt: u32,
    pub should_retry: bool,
    pub delay_ms: u64,
    pub next_attempt_at_ms: Option<u64>,
    pub exhausted: bool,
}

#[derive(Clone, Debug)]
pub struct CultNetReconnectController {
    pub policy: CultNetReconnectPolicy,
    attempt: u32,
    next_attempt_at_ms: Option<u64>,
    exhausted: bool,
}

impl Default for CultNetReconnectController {
    fn default() -> Self {
        Self::new(create_reconnect_policy(Default::default()))
    }
}

impl CultNetReconnectController {
    pub fn new(policy: CultNetReconnectPolicy) -> Self {
        Self {
            policy,
            attempt: 0,
            next_attempt_at_ms: None,
            exhausted: false,
        }
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn next_attempt_at_ms(&self) -> Option<u64> {
        self.next_attempt_at_ms
    }

    pub fn exhausted(&self) -> bool {
        self.exhausted
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
        self.next_attempt_at_ms = None;
        self.exhausted = false;
    }

    pub fn can_attempt(&self, now_ms: u64) -> bool {
        !self.exhausted
            && self
                .next_attempt_at_ms
                .is_none_or(|next_attempt_at_ms| now_ms >= next_attempt_at_ms)
    }

    pub fn record_failure(&mut self, now_ms: u64, jitter_ms: u64) -> CultNetReconnectDecision {
        let next_attempt = self.attempt.saturating_add(1);
        if self
            .policy
            .max_attempts
            .is_some_and(|max_attempts| next_attempt > max_attempts)
        {
            self.exhausted = true;
            self.next_attempt_at_ms = None;
            return CultNetReconnectDecision {
                attempt: self.attempt,
                should_retry: false,
                delay_ms: 0,
                next_attempt_at_ms: None,
                exhausted: true,
            };
        }

        self.attempt = next_attempt;
        let delay_ms = compute_reconnect_delay_ms(&self.policy, self.attempt, jitter_ms);
        let next_attempt_at_ms = now_ms.saturating_add(delay_ms);
        self.next_attempt_at_ms = Some(next_attempt_at_ms);
        CultNetReconnectDecision {
            attempt: self.attempt,
            should_retry: true,
            delay_ms,
            next_attempt_at_ms: Some(next_attempt_at_ms),
            exhausted: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TcpFramedTransportProfileOptions {
    pub transport_id: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub max_payload_bytes: Option<u32>,
    pub max_fragment_bytes: Option<u32>,
}

pub fn create_tcp_framed_transport_profile(
    runtime_id: impl Into<String>,
    options: TcpFramedTransportProfileOptions,
) -> CultNetTransportProfile {
    CultNetTransportProfile {
        schema_version: "cultnet.transport_profile.v0".to_string(),
        runtime_id: runtime_id.into(),
        transports: vec![CultNetTransportDescriptor {
            transport_id: options
                .transport_id
                .unwrap_or_else(|| "tcp-framed".to_string()),
            protocol: CultNetTransportProtocol::TcpFramed,
            host: options.host,
            port: options.port,
            path: None,
            discovery_group: None,
            wire_contracts: Some(vec!["cultnet.schema.v0".to_string()]),
            reconnect_policy: None,
            channels: vec![CultNetTransportChannel {
                channel_id: "schema".to_string(),
                delivery: CultNetTransportDelivery::Reliable,
                ordering: CultNetTransportOrdering::Ordered,
                max_payload_bytes: options.max_payload_bytes,
                max_fragment_bytes: options.max_fragment_bytes,
                max_pending_reliable_packets: None,
            }],
        }],
    }
}

pub struct TcpFramedTransportConnection<TStream> {
    stream: TStream,
    pub profile: CultNetTransportProfile,
    stats: CultNetTransportStats,
}

impl<TStream> TcpFramedTransportConnection<TStream> {
    pub fn new(stream: TStream, profile: CultNetTransportProfile) -> Self {
        Self {
            stream,
            profile,
            stats: CultNetTransportStats::default(),
        }
    }

    pub fn stats(&self) -> CultNetTransportStats {
        self.stats.clone()
    }

    pub fn into_inner(self) -> TStream {
        self.stream
    }
}

impl<TStream> TcpFramedTransportConnection<TStream>
where
    TStream: Write,
{
    pub fn send(&mut self, channel_id: &str, payload: &[u8]) -> Result<()> {
        if channel_id != "schema" {
            return Err(anyhow!(
                "tcp_framed transport only supports the schema channel, got {channel_id:?}"
            ));
        }

        let frame = encode_frame(payload)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        self.stats.bytes_sent += frame.len() as u64;
        self.stats.frames_sent += 1;
        Ok(())
    }
}

impl<TStream> TcpFramedTransportConnection<TStream>
where
    TStream: Read,
{
    pub fn receive(&mut self) -> Result<CultNetTransportFrame> {
        let mut header = [0_u8; FRAME_HEADER_BYTES];
        self.stream.read_exact(&mut header)?;
        let payload_len = u32::from_be_bytes(header) as usize;
        let mut payload = vec![0_u8; payload_len];
        self.stream.read_exact(&mut payload)?;
        self.stats.bytes_received += (FRAME_HEADER_BYTES + payload_len) as u64;
        self.stats.frames_received += 1;
        Ok(CultNetTransportFrame {
            channel_id: "schema".to_string(),
            payload,
        })
    }
}
