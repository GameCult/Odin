use anyhow::Result;
use anyhow::anyhow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::CultNetMessage;
use crate::CultNetReconnectController;
use crate::CultNetReconnectDecision;
use crate::CultNetReconnectPolicy;
use crate::CultNetTransportChannel;
use crate::CultNetTransportDelivery;
use crate::CultNetTransportDescriptor;
use crate::CultNetTransportFrame;
use crate::CultNetTransportOrdering;
use crate::CultNetTransportProfile;
use crate::CultNetTransportProtocol;
use crate::CultNetTransportStats;
use crate::CultNetWireContract;
use crate::create_reconnect_policy;
use crate::decode_cultnet_message_from_slice;
use crate::encode_cultnet_message_to_vec;

const RUDP_MAGIC: [u8; 4] = [0x43, 0x4e, 0x52, 0x30];
const RUDP_VERSION: u8 = 0;
const RUDP_FIXED_HEADER_BYTES: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultNetRudpPacketType {
    Connect,
    Accept,
    Data,
    Ack,
    Ping,
    Pong,
    Disconnect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpPacket {
    pub packet_type: CultNetRudpPacketType,
    pub connection_id: u32,
    pub sequence: u32,
    pub ack: u32,
    pub ack_mask: u32,
    pub channel_id: String,
    pub reliable: bool,
    pub ordered: bool,
    pub sequenced: bool,
    pub fragment_id: u16,
    pub fragment_index: u16,
    pub fragment_count: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpDeliveredFrame {
    pub channel_id: String,
    pub payload: Vec<u8>,
    pub sequence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpReceiveResult {
    pub delivered: Vec<CultNetRudpDeliveredFrame>,
    pub reply: Option<CultNetRudpPacket>,
    pub pong: bool,
    pub pong_payload: Vec<u8>,
    pub disconnected: bool,
    pub disconnect_reason: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpSessionOptions {
    pub connection_id: u32,
    pub initial_sequence: u32,
    pub resend_delay_ms: u64,
    pub max_pending_reliable_packets: Option<usize>,
}

impl Default for CultNetRudpSessionOptions {
    fn default() -> Self {
        Self {
            connection_id: 0,
            initial_sequence: 1,
            resend_delay_ms: 250,
            max_pending_reliable_packets: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CultNetRudpSendOptions {
    pub reliable: bool,
    pub ordered: bool,
    pub sequenced: bool,
    pub now_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingReliablePacket {
    packet: CultNetRudpPacket,
    first_sent_at_ms: u64,
    last_sent_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingOrderedFrame {
    frame: CultNetRudpDeliveredFrame,
    next_sequence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FragmentBuffer {
    channel_id: String,
    ordered: bool,
    fragment_count: u16,
    payloads: BTreeMap<u16, Vec<u8>>,
    sequences: BTreeMap<u16, u32>,
}

pub struct CultNetRudpSession {
    connection_id: u32,
    resend_delay_ms: u64,
    max_pending_reliable_packets: Option<usize>,
    reliable_expire_after_ms: Option<u64>,
    reliable_packets_expired: u64,
    initial_sequence: u32,
    next_sequence: u32,
    next_fragment_id: u16,
    connected: bool,
    last_received_at_ms: Option<u64>,
    last_received_sequence: Option<u32>,
    highest_received_sequence: Option<u32>,
    received_sequences: BTreeSet<u32>,
    pending_reliable: BTreeMap<u32, PendingReliablePacket>,
    ordered_next_sequence_by_channel: BTreeMap<String, u32>,
    ordered_buffers: BTreeMap<String, BTreeMap<u32, PendingOrderedFrame>>,
    fragment_buffers: BTreeMap<(String, u16), FragmentBuffer>,
}

impl CultNetRudpSession {
    pub fn new(options: CultNetRudpSessionOptions) -> Self {
        Self {
            connection_id: options.connection_id,
            resend_delay_ms: options.resend_delay_ms,
            max_pending_reliable_packets: options.max_pending_reliable_packets,
            reliable_expire_after_ms: None,
            reliable_packets_expired: 0,
            initial_sequence: options.initial_sequence,
            next_sequence: options.initial_sequence,
            next_fragment_id: 1,
            connected: false,
            last_received_at_ms: None,
            last_received_sequence: None,
            highest_received_sequence: None,
            received_sequences: BTreeSet::new(),
            pending_reliable: BTreeMap::new(),
            ordered_next_sequence_by_channel: BTreeMap::new(),
            ordered_buffers: BTreeMap::new(),
            fragment_buffers: BTreeMap::new(),
        }
    }

    pub fn connection_id(&self) -> u32 {
        self.connection_id
    }

    pub fn resend_delay_ms(&self) -> u64 {
        self.resend_delay_ms
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn pending_reliable_sequences(&self) -> Vec<u32> {
        self.pending_reliable.keys().copied().collect()
    }

    pub fn reliable_packets_expired(&self) -> u64 {
        self.reliable_packets_expired
    }

    pub fn set_reliable_expire_after_ms(&mut self, expire_after_ms: Option<u64>) {
        self.reliable_expire_after_ms = expire_after_ms;
    }

    pub fn last_received_at_ms(&self) -> Option<u64> {
        self.last_received_at_ms
    }

    pub fn reset_peer_state(&mut self) {
        self.next_sequence = self.initial_sequence;
        self.next_fragment_id = 1;
        self.connected = false;
        self.last_received_at_ms = None;
        self.last_received_sequence = None;
        self.highest_received_sequence = None;
        self.received_sequences.clear();
        self.pending_reliable.clear();
        self.ordered_next_sequence_by_channel.clear();
        self.ordered_buffers.clear();
        self.fragment_buffers.clear();
    }

    pub fn create_connect(&mut self, now_ms: u64, payload: Vec<u8>) -> Result<CultNetRudpPacket> {
        self.ensure_reliable_capacity(1)?;
        let packet = self.create_packet(
            CultNetRudpPacketType::Connect,
            "control",
            payload,
            true,
            true,
            false,
        );
        self.track_reliable(packet.clone(), now_ms);
        Ok(packet)
    }

    pub fn accept_connect(
        &mut self,
        packet: &CultNetRudpPacket,
        now_ms: u64,
        payload: Vec<u8>,
    ) -> Result<CultNetRudpPacket> {
        self.require_connection(packet)?;
        if packet.packet_type != CultNetRudpPacketType::Connect {
            return Err(anyhow!(
                "Expected RUDP connect packet, got {:?}",
                packet.packet_type
            ));
        }

        self.ensure_reliable_capacity(1)?;
        self.remember_received(packet.sequence);
        self.connected = true;
        let response = self.create_packet(
            CultNetRudpPacketType::Accept,
            "control",
            payload,
            true,
            true,
            false,
        );
        self.track_reliable(response.clone(), now_ms);
        Ok(response)
    }

    pub fn send(
        &mut self,
        channel_id: &str,
        payload: Vec<u8>,
        options: CultNetRudpSendOptions,
    ) -> Result<CultNetRudpPacket> {
        self.send_many(channel_id, payload, options, None)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("RUDP send produced no packets"))
    }

    pub fn send_many(
        &mut self,
        channel_id: &str,
        payload: Vec<u8>,
        options: CultNetRudpSendOptions,
        max_fragment_bytes: Option<usize>,
    ) -> Result<Vec<CultNetRudpPacket>> {
        if !self.connected {
            return Err(anyhow!(
                "Cannot send RUDP data before the session is connected"
            ));
        }

        if let Some(max_fragment_bytes) = max_fragment_bytes {
            if max_fragment_bytes == 0 {
                return Err(anyhow!("RUDP max_fragment_bytes must be greater than zero"));
            }
            if payload.len() > max_fragment_bytes {
                let fragment_count = payload.len().div_ceil(max_fragment_bytes);
                if fragment_count > u16::MAX as usize {
                    return Err(anyhow!("RUDP payload requires more than 65535 fragments"));
                }
                self.ensure_reliable_capacity(if options.reliable { fragment_count } else { 0 })?;
                let fragment_id = self.allocate_fragment_id();
                let mut packets = Vec::new();
                for index in 0..fragment_count {
                    let start = index * max_fragment_bytes;
                    let end = (start + max_fragment_bytes).min(payload.len());
                    let packet = self.create_packet_with_fragments(
                        CultNetRudpPacketType::Data,
                        channel_id,
                        payload[start..end].to_vec(),
                        options.reliable,
                        options.ordered,
                        options.sequenced,
                        fragment_id,
                        index as u16,
                        fragment_count as u16,
                    );
                    if packet.reliable {
                        self.track_reliable(packet.clone(), options.now_ms);
                    }
                    packets.push(packet);
                }
                return Ok(packets);
            }
        }

        self.ensure_reliable_capacity(if options.reliable { 1 } else { 0 })?;
        let packet = self.create_packet(
            CultNetRudpPacketType::Data,
            channel_id,
            payload,
            options.reliable,
            options.ordered,
            options.sequenced,
        );
        if packet.reliable {
            self.track_reliable(packet.clone(), options.now_ms);
        }
        Ok(vec![packet])
    }

    pub fn receive(
        &mut self,
        packet: &CultNetRudpPacket,
        now_ms: u64,
    ) -> Result<CultNetRudpReceiveResult> {
        self.require_connection(packet)?;
        self.apply_acknowledgements(packet);
        self.last_received_at_ms = Some(now_ms);
        let expected_sequence_if_uninitialized = self
            .highest_received_sequence
            .map(|sequence| sequence + 1)
            .unwrap_or(packet.sequence);

        if packet.packet_type == CultNetRudpPacketType::Accept {
            self.remember_received(packet.sequence);
            self.connected = true;
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: false,
                pong_payload: Vec::new(),
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        }

        if packet.packet_type == CultNetRudpPacketType::Ping {
            self.remember_received(packet.sequence);
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: Some(self.create_packet(
                    CultNetRudpPacketType::Pong,
                    "control",
                    packet.payload.clone(),
                    false,
                    false,
                    false,
                )),
                pong: false,
                pong_payload: Vec::new(),
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        }

        if packet.packet_type == CultNetRudpPacketType::Ack
            || packet.packet_type == CultNetRudpPacketType::Pong
        {
            if packet.packet_type == CultNetRudpPacketType::Pong {
                self.remember_received(packet.sequence);
            }
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: packet.packet_type == CultNetRudpPacketType::Pong,
                pong_payload: if packet.packet_type == CultNetRudpPacketType::Pong {
                    packet.payload.clone()
                } else {
                    Vec::new()
                },
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        }

        if packet.packet_type == CultNetRudpPacketType::Disconnect {
            self.remember_received(packet.sequence);
            self.connected = false;
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: false,
                pong_payload: Vec::new(),
                disconnected: true,
                disconnect_reason: packet.payload.clone(),
            });
        }

        if packet.packet_type != CultNetRudpPacketType::Data {
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: false,
                pong_payload: Vec::new(),
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        }

        let duplicate = self.received_sequences.contains(&packet.sequence);
        self.remember_received(packet.sequence);
        if duplicate {
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: false,
                pong_payload: Vec::new(),
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        }

        let Some((frame, ordered, next_sequence)) = self.reassemble(packet)? else {
            return Ok(CultNetRudpReceiveResult {
                delivered: Vec::new(),
                reply: None,
                pong: false,
                pong_payload: Vec::new(),
                disconnected: false,
                disconnect_reason: Vec::new(),
            });
        };
        let delivered = if ordered {
            self.deliver_ordered(frame, next_sequence, expected_sequence_if_uninitialized)
        } else {
            vec![frame]
        };
        Ok(CultNetRudpReceiveResult {
            delivered,
            reply: None,
            pong: false,
            pong_payload: Vec::new(),
            disconnected: false,
            disconnect_reason: Vec::new(),
        })
    }

    pub fn create_ack(&mut self) -> CultNetRudpPacket {
        // ACK the packet that actually caused this response. A retransmitted
        // reliable packet can arrive more than 32 global sequence numbers
        // behind replaceable realtime traffic; a highest-sequence-only ACK
        // can never retire it once it falls outside that mask.
        let (ack, ack_mask) = self.direct_ack_state();
        CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Ack,
            connection_id: self.connection_id,
            sequence: 0,
            ack,
            ack_mask,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        }
    }

    pub fn create_ping(&mut self, payload: Vec<u8>) -> CultNetRudpPacket {
        self.create_packet(
            CultNetRudpPacketType::Ping,
            "control",
            payload,
            false,
            false,
            false,
        )
    }

    pub fn create_disconnect(&mut self, reason: Vec<u8>) -> CultNetRudpPacket {
        self.connected = false;
        self.create_packet(
            CultNetRudpPacketType::Disconnect,
            "control",
            reason,
            false,
            false,
            false,
        )
    }

    pub fn check_timeout(&mut self, now_ms: u64, timeout_ms: u64) -> bool {
        if !self.connected {
            return false;
        }
        let Some(last_received_at_ms) = self.last_received_at_ms else {
            return false;
        };
        if now_ms.saturating_sub(last_received_at_ms) <= timeout_ms {
            return false;
        }
        self.connected = false;
        true
    }

    pub fn due_resends(&mut self, now_ms: u64) -> Vec<CultNetRudpPacket> {
        if let Some(expire_after_ms) = self.reliable_expire_after_ms {
            let before = self.pending_reliable.len();
            self.pending_reliable.retain(|_, pending| {
                now_ms.saturating_sub(pending.first_sent_at_ms) <= expire_after_ms
            });
            self.reliable_packets_expired = self
                .reliable_packets_expired
                .saturating_add((before - self.pending_reliable.len()) as u64);
        }
        let mut due = Vec::new();
        for pending in self.pending_reliable.values_mut() {
            if now_ms.saturating_sub(pending.last_sent_at_ms) >= self.resend_delay_ms {
                pending.last_sent_at_ms = now_ms;
                due.push(pending.packet.clone());
            }
        }
        due.sort_by_key(|packet| packet.sequence);
        due
    }

    fn create_packet(
        &mut self,
        packet_type: CultNetRudpPacketType,
        channel_id: &str,
        payload: Vec<u8>,
        reliable: bool,
        ordered: bool,
        sequenced: bool,
    ) -> CultNetRudpPacket {
        self.create_packet_with_fragments(
            packet_type,
            channel_id,
            payload,
            reliable,
            ordered,
            sequenced,
            0,
            0,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_packet_with_fragments(
        &mut self,
        packet_type: CultNetRudpPacketType,
        channel_id: &str,
        payload: Vec<u8>,
        reliable: bool,
        ordered: bool,
        sequenced: bool,
        fragment_id: u16,
        fragment_index: u16,
        fragment_count: u16,
    ) -> CultNetRudpPacket {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("sequence overflow");
        let (ack, ack_mask) = self.ack_state();
        CultNetRudpPacket {
            packet_type,
            connection_id: self.connection_id,
            sequence,
            ack,
            ack_mask,
            channel_id: channel_id.to_string(),
            reliable,
            ordered,
            sequenced,
            fragment_id,
            fragment_index,
            fragment_count,
            payload,
        }
    }

    fn track_reliable(&mut self, packet: CultNetRudpPacket, now_ms: u64) {
        self.pending_reliable.insert(
            packet.sequence,
            PendingReliablePacket {
                packet,
                first_sent_at_ms: now_ms,
                last_sent_at_ms: now_ms,
            },
        );
    }

    fn ensure_reliable_capacity(&self, packet_count: usize) -> Result<()> {
        if packet_count == 0 {
            return Ok(());
        }
        if let Some(limit) = self.max_pending_reliable_packets {
            if self.pending_reliable.len() + packet_count > limit {
                return Err(std::io::Error::new(
                    ErrorKind::WouldBlock,
                    "RUDP reliable send queue is full",
                )
                .into());
            }
        }
        Ok(())
    }

    fn apply_acknowledgements(&mut self, packet: &CultNetRudpPacket) {
        self.pending_reliable.remove(&packet.ack);
        for bit in 0..32 {
            if (packet.ack_mask & (1_u32 << bit)) != 0 && packet.ack > bit {
                self.pending_reliable.remove(&(packet.ack - bit - 1));
            }
        }
    }

    fn remember_received(&mut self, sequence: u32) {
        self.last_received_sequence = Some(sequence);
        self.received_sequences.insert(sequence);
        if self
            .highest_received_sequence
            .is_none_or(|highest| sequence > highest)
        {
            self.highest_received_sequence = Some(sequence);
        }
    }

    fn ack_state(&self) -> (u32, u32) {
        let ack = self.highest_received_sequence.unwrap_or(0);
        let mut ack_mask = 0_u32;
        for bit in 0..32 {
            if ack > bit && self.received_sequences.contains(&(ack - bit - 1)) {
                ack_mask |= 1_u32 << bit;
            }
        }
        (ack, ack_mask)
    }

    fn direct_ack_state(&self) -> (u32, u32) {
        let last = self.last_received_sequence.unwrap_or(0);
        let highest = self.highest_received_sequence.unwrap_or(last);
        let ack = if highest.saturating_sub(last) > 32 {
            last
        } else {
            highest
        };
        let mut ack_mask = 0_u32;
        for bit in 0..32 {
            if ack > bit && self.received_sequences.contains(&(ack - bit - 1)) {
                ack_mask |= 1_u32 << bit;
            }
        }
        (ack, ack_mask)
    }

    fn reassemble(
        &mut self,
        packet: &CultNetRudpPacket,
    ) -> Result<Option<(CultNetRudpDeliveredFrame, bool, u32)>> {
        if packet.fragment_count == 0 {
            return Ok(Some((
                CultNetRudpDeliveredFrame {
                    channel_id: packet.channel_id.clone(),
                    payload: packet.payload.clone(),
                    sequence: packet.sequence,
                },
                packet.ordered,
                packet.sequence + 1,
            )));
        }
        if packet.fragment_id == 0 {
            return Err(anyhow!(
                "RUDP fragmented packet must have a non-zero fragment id"
            ));
        }
        if packet.fragment_index >= packet.fragment_count {
            return Err(anyhow!(
                "RUDP fragment index must be lower than fragment count"
            ));
        }

        let key = (packet.channel_id.clone(), packet.fragment_id);
        let buffer = self
            .fragment_buffers
            .entry(key.clone())
            .or_insert_with(|| FragmentBuffer {
                channel_id: packet.channel_id.clone(),
                ordered: packet.ordered,
                fragment_count: packet.fragment_count,
                payloads: BTreeMap::new(),
                sequences: BTreeMap::new(),
            });
        if buffer.fragment_count != packet.fragment_count || buffer.ordered != packet.ordered {
            return Err(anyhow!(
                "RUDP fragment metadata changed within a fragment set"
            ));
        }
        buffer
            .payloads
            .insert(packet.fragment_index, packet.payload.clone());
        buffer
            .sequences
            .insert(packet.fragment_index, packet.sequence);
        if buffer.payloads.len() < packet.fragment_count as usize {
            return Ok(None);
        }

        let mut payload = Vec::new();
        let mut sequences = Vec::new();
        for index in 0..packet.fragment_count {
            let Some(chunk) = buffer.payloads.get(&index) else {
                return Ok(None);
            };
            let Some(sequence) = buffer.sequences.get(&index) else {
                return Ok(None);
            };
            payload.extend_from_slice(chunk);
            sequences.push(*sequence);
        }
        let channel_id = buffer.channel_id.clone();
        let ordered = buffer.ordered;
        self.fragment_buffers.remove(&key);
        Ok(Some((
            CultNetRudpDeliveredFrame {
                channel_id,
                payload,
                sequence: *sequences.iter().min().unwrap(),
            },
            ordered,
            sequences.iter().max().unwrap() + 1,
        )))
    }

    fn deliver_ordered(
        &mut self,
        frame: CultNetRudpDeliveredFrame,
        next_sequence_after_frame: u32,
        expected_sequence_if_uninitialized: u32,
    ) -> Vec<CultNetRudpDeliveredFrame> {
        let channel_id = frame.channel_id.clone();
        let mut next = if let Some(next) = self
            .ordered_next_sequence_by_channel
            .get(&channel_id)
            .copied()
        {
            next
        } else {
            self.ordered_next_sequence_by_channel.insert(
                channel_id.clone(),
                expected_sequence_if_uninitialized.min(frame.sequence),
            );
            expected_sequence_if_uninitialized.min(frame.sequence)
        };

        while frame.sequence > next
            && self.received_sequences.contains(&next)
            && !self
                .ordered_buffers
                .get(&channel_id)
                .is_some_and(|buffer| buffer.contains_key(&next))
        {
            next = next.saturating_add(1);
            self.ordered_next_sequence_by_channel
                .insert(channel_id.clone(), next);
        }

        if frame.sequence < next {
            return Vec::new();
        }

        if frame.sequence > next {
            self.ordered_buffers.entry(channel_id).or_default().insert(
                frame.sequence,
                PendingOrderedFrame {
                    frame,
                    next_sequence: next_sequence_after_frame,
                },
            );
            return Vec::new();
        }

        self.ordered_next_sequence_by_channel
            .insert(channel_id.clone(), next_sequence_after_frame);
        let mut delivered = vec![frame];
        delivered.extend(self.drain_ordered(&channel_id));
        delivered
    }

    fn drain_ordered(&mut self, channel_id: &str) -> Vec<CultNetRudpDeliveredFrame> {
        let mut delivered = Vec::new();
        loop {
            let Some(next) = self
                .ordered_next_sequence_by_channel
                .get(channel_id)
                .copied()
            else {
                break;
            };
            let Some(buffer) = self.ordered_buffers.get_mut(channel_id) else {
                break;
            };
            let Some(pending) = buffer.remove(&next) else {
                break;
            };
            delivered.push(pending.frame);
            self.ordered_next_sequence_by_channel
                .insert(channel_id.to_string(), pending.next_sequence);
            self.skip_received_non_channel_sequences(channel_id);
        }
        delivered
    }

    fn skip_received_non_channel_sequences(&mut self, channel_id: &str) {
        let Some(mut next) = self
            .ordered_next_sequence_by_channel
            .get(channel_id)
            .copied()
        else {
            return;
        };
        while self.received_sequences.contains(&next)
            && !self
                .ordered_buffers
                .get(channel_id)
                .is_some_and(|buffer| buffer.contains_key(&next))
        {
            next = next.saturating_add(1);
            self.ordered_next_sequence_by_channel
                .insert(channel_id.to_string(), next);
        }
    }

    fn allocate_fragment_id(&mut self) -> u16 {
        let fragment_id = self.next_fragment_id;
        self.next_fragment_id = self.next_fragment_id.wrapping_add(1);
        if self.next_fragment_id == 0 {
            self.next_fragment_id = 1;
        }
        fragment_id
    }

    fn require_connection(&self, packet: &CultNetRudpPacket) -> Result<()> {
        if packet.connection_id != self.connection_id {
            return Err(anyhow!(
                "RUDP packet connection id {} does not match {}",
                packet.connection_id,
                self.connection_id
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultNetRudpSocketMode {
    Client,
    Server,
}

pub struct CultNetRudpSocketTransportOptions {
    pub runtime_id: String,
    pub socket: UdpSocket,
    pub mode: CultNetRudpSocketMode,
    pub remote_addr: Option<SocketAddr>,
    pub connection_id: u32,
    pub initial_sequence: u32,
    pub resend_delay_ms: u64,
    pub transport_id: Option<String>,
    pub max_payload_bytes: Option<u32>,
    pub max_fragment_bytes: Option<u32>,
    pub max_pending_reliable_packets: Option<u32>,
    pub reconnect_policy: Option<CultNetReconnectPolicy>,
}

impl CultNetRudpSocketTransportOptions {
    pub fn client(
        runtime_id: impl Into<String>,
        socket: UdpSocket,
        remote_addr: SocketAddr,
        connection_id: u32,
    ) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            socket,
            mode: CultNetRudpSocketMode::Client,
            remote_addr: Some(remote_addr),
            connection_id,
            initial_sequence: 1,
            resend_delay_ms: 250,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        }
    }

    pub fn server(runtime_id: impl Into<String>, socket: UdpSocket, connection_id: u32) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id,
            initial_sequence: 1,
            resend_delay_ms: 250,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        }
    }
}

pub struct CultNetRudpSocketTransportConnection {
    socket: UdpSocket,
    session: CultNetRudpSession,
    mode: CultNetRudpSocketMode,
    remote_addr: Option<SocketAddr>,
    pub profile: CultNetTransportProfile,
    stats: CultNetTransportStats,
    delivered_frames: VecDeque<CultNetTransportFrame>,
    max_fragment_bytes: Option<usize>,
    disconnect_reason: Option<Vec<u8>>,
    pong_payloads: VecDeque<Vec<u8>>,
    pending_outbound_packets: VecDeque<CultNetRudpPacket>,
    max_pending_outbound_packets: usize,
}

impl CultNetRudpSocketTransportConnection {
    pub fn new(options: CultNetRudpSocketTransportOptions) -> Result<Self> {
        let local_addr = options.socket.local_addr()?;
        let profile = create_rudp_transport_profile(
            options.runtime_id,
            RudpTransportProfileOptions {
                transport_id: options.transport_id,
                host: Some(local_addr.ip().to_string()),
                port: Some(local_addr.port()),
                max_payload_bytes: options.max_payload_bytes,
                max_fragment_bytes: options.max_fragment_bytes,
                max_pending_reliable_packets: options.max_pending_reliable_packets,
                reconnect_policy: options.reconnect_policy,
            },
        );
        let max_pending_reliable_packets = options
            .max_pending_reliable_packets
            .map(|value| value as usize);
        let max_pending_outbound_packets = max_pending_reliable_packets.unwrap_or(1_024).max(1);
        let session = CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: options.connection_id,
            initial_sequence: options.initial_sequence,
            resend_delay_ms: options.resend_delay_ms,
            max_pending_reliable_packets,
        });
        Ok(Self {
            socket: options.socket,
            session,
            mode: options.mode,
            remote_addr: options.remote_addr,
            profile,
            stats: CultNetTransportStats::default(),
            delivered_frames: VecDeque::new(),
            max_fragment_bytes: options.max_fragment_bytes.map(|value| value as usize),
            disconnect_reason: None,
            pong_payloads: VecDeque::new(),
            pending_outbound_packets: VecDeque::new(),
            max_pending_outbound_packets,
        })
    }

    pub fn connected(&self) -> bool {
        self.session.connected()
    }

    pub fn stats(&self) -> CultNetTransportStats {
        self.stats.clone()
    }

    pub fn reliable_packets_expired(&self) -> u64 {
        self.session.reliable_packets_expired()
    }

    pub fn pending_reliable_packets(&self) -> usize {
        self.session.pending_reliable_sequences().len()
    }

    pub fn set_reliable_expire_after_ms(&mut self, expire_after_ms: Option<u64>) {
        self.session.set_reliable_expire_after_ms(expire_after_ms);
    }

    pub fn disconnect_reason(&self) -> Option<&[u8]> {
        self.disconnect_reason.as_deref()
    }

    pub fn pop_pong_payload(&mut self) -> Option<Vec<u8>> {
        self.pong_payloads.pop_front()
    }

    pub fn connect(&mut self, payload: Vec<u8>) -> Result<()> {
        if self.mode != CultNetRudpSocketMode::Client {
            return Err(anyhow!(
                "Only a client RUDP socket transport can initiate connect"
            ));
        }
        let packet = self.session.create_connect(now_ms(), payload)?;
        self.send_packet(&packet)
    }

    pub fn send(&mut self, channel_id: &str, payload: Vec<u8>) -> Result<()> {
        self.flush_pending_outbound_packets()?;
        let packet_count = self.outbound_packet_count(payload.len())?;
        if self.pending_outbound_packets.len().saturating_add(packet_count)
            > self.max_pending_outbound_packets
        {
            return Err(std::io::Error::new(
                ErrorKind::WouldBlock,
                "CultNet RUDP outbound packet queue is full",
            )
            .into());
        }
        let options = channel_send_options(channel_id, now_ms());
        let packets =
            self.session
                .send_many(channel_id, payload, options, self.max_fragment_bytes)?;
        self.pending_outbound_packets.extend(packets);
        self.flush_pending_outbound_packets()?;
        self.stats.frames_sent += 1;
        Ok(())
    }

    pub fn send_schema_message(&mut self, message: &CultNetMessage) -> Result<()> {
        let payload = encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?;
        self.send("schema", payload)
    }

    pub fn receive_schema_message_once(&mut self) -> Result<Option<CultNetMessage>> {
        let Some(frame) = self.receive_once()? else {
            return Ok(None);
        };
        if frame.channel_id != "schema" {
            return Err(anyhow!(
                "Expected RUDP schema frame, received channel {}",
                frame.channel_id
            ));
        }
        Ok(Some(decode_cultnet_message_from_slice(
            &frame.payload,
            CultNetWireContract::CultNetSchemaV0,
        )?))
    }

    pub fn disconnect(&mut self, reason: Vec<u8>) -> Result<()> {
        let packet = self.session.create_disconnect(reason);
        self.send_packet(&packet)
    }

    pub fn ping(&mut self, payload: Vec<u8>) -> Result<()> {
        let packet = self.session.create_ping(payload);
        self.send_packet(&packet)
    }

    pub fn check_timeout(&mut self, timeout_ms: u64) -> bool {
        self.session.check_timeout(now_ms(), timeout_ms)
    }

    pub fn receive_once(&mut self) -> Result<Option<CultNetTransportFrame>> {
        if let Some(frame) = self.delivered_frames.pop_front() {
            return Ok(Some(frame));
        }

        self.poll_receive_once()?;
        Ok(self.delivered_frames.pop_front())
    }

    pub fn poll_receive_once(&mut self) -> Result<bool> {

        let mut wire = vec![0_u8; 65_535];
        let (received, remote_addr) = match self.socket.recv_from(&mut wire) {
            Ok(value) => value,
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        wire.truncate(received);
        self.stats.bytes_received += received as u64;

        let packet = decode_rudp_packet(&wire)?;
        if let Some(expected) = self.remote_addr {
            if expected != remote_addr {
                if self.mode == CultNetRudpSocketMode::Server
                    && packet.packet_type == CultNetRudpPacketType::Connect
                {
                    self.remote_addr = Some(remote_addr);
                } else {
                    return Ok(true);
                }
            }
        } else {
            if self.mode == CultNetRudpSocketMode::Server
                && packet.packet_type != CultNetRudpPacketType::Connect
            {
                return Ok(true);
            }
            self.remote_addr = Some(remote_addr);
        }
        if self.mode == CultNetRudpSocketMode::Server
            && packet.packet_type == CultNetRudpPacketType::Connect
        {
            self.session.reset_peer_state();
            let accept = self.session.accept_connect(&packet, now_ms(), Vec::new())?;
            self.send_packet(&accept)?;
            return Ok(true);
        }

        let result = self.session.receive(&packet, now_ms())?;
        if let Some(reply) = result.reply {
            self.send_packet(&reply)?;
        }
        if result.pong {
            self.pong_payloads.push_back(result.pong_payload);
        }
        if result.disconnected {
            self.disconnect_reason = Some(result.disconnect_reason);
            return Ok(true);
        }

        let delivered_any = !result.delivered.is_empty();
        for frame in result.delivered {
            self.delivered_frames.push_back(CultNetTransportFrame {
                channel_id: frame.channel_id,
                payload: frame.payload,
            });
            self.stats.frames_received += 1;
        }
        // Reliable duplicates still need an ACK. Suppressing the ACK because
        // deduplication produced no application frame creates an immortal
        // resend loop after the original ACK is lost.
        if packet.packet_type == CultNetRudpPacketType::Accept || delivered_any || packet.reliable {
            let ack = self.session.create_ack();
            self.send_packet(&ack)?;
        }
        Ok(true)
    }

    pub fn poll_resends(&mut self) -> Result<()> {
        self.flush_pending_outbound_packets()?;
        if !self.pending_outbound_packets.is_empty() {
            return Ok(());
        }
        for packet in self.session.due_resends(now_ms()) {
            if self.pending_outbound_packets.len() >= self.max_pending_outbound_packets {
                break;
            }
            self.pending_outbound_packets.push_back(packet);
        }
        self.flush_pending_outbound_packets()?;
        Ok(())
    }

    fn outbound_packet_count(&self, payload_bytes: usize) -> Result<usize> {
        match self.max_fragment_bytes {
            Some(0) => Err(anyhow!("RUDP max_fragment_bytes must be greater than zero")),
            Some(max_fragment_bytes) if payload_bytes > max_fragment_bytes => {
                Ok(payload_bytes.div_ceil(max_fragment_bytes))
            }
            _ => Ok(1),
        }
    }

    fn flush_pending_outbound_packets(&mut self) -> Result<()> {
        let Some(remote_addr) = self.remote_addr else {
            return Err(anyhow!("RUDP socket transport does not have a remote endpoint"));
        };
        let sent_bytes = flush_rudp_outbound_queue(&mut self.pending_outbound_packets, |wire| {
            self.socket.send_to(wire, remote_addr)
        })?;
        self.stats.bytes_sent += sent_bytes as u64;
        Ok(())
    }

    fn send_packet(&mut self, packet: &CultNetRudpPacket) -> Result<()> {
        let Some(remote_addr) = self.remote_addr else {
            return Err(anyhow!(
                "RUDP socket transport does not have a remote endpoint"
            ));
        };
        let wire = encode_rudp_packet(packet)?;
        let sent = self.socket.send_to(&wire, remote_addr)?;
        self.stats.bytes_sent += sent as u64;
        Ok(())
    }
}

fn flush_rudp_outbound_queue<F>(
    pending: &mut VecDeque<CultNetRudpPacket>,
    mut send: F,
) -> Result<usize>
where
    F: FnMut(&[u8]) -> std::io::Result<usize>,
{
    let mut sent_bytes = 0_usize;
    while let Some(packet) = pending.front() {
        let wire = encode_rudp_packet(packet)?;
        match send(&wire) {
            Ok(sent) => {
                sent_bytes = sent_bytes.saturating_add(sent);
                pending.pop_front();
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(sent_bytes)
}

pub struct CultNetRudpReconnectLoop<F>
where
    F: FnMut() -> Result<CultNetRudpSocketTransportConnection>,
{
    pub reconnect_controller: CultNetReconnectController,
    create_transport: F,
    connect_payload: Vec<u8>,
    transport: Option<CultNetRudpSocketTransportConnection>,
    stopped: bool,
}

impl<F> CultNetRudpReconnectLoop<F>
where
    F: FnMut() -> Result<CultNetRudpSocketTransportConnection>,
{
    pub fn new(
        reconnect_policy: CultNetReconnectPolicy,
        connect_payload: Vec<u8>,
        create_transport: F,
    ) -> Self {
        Self {
            reconnect_controller: CultNetReconnectController::new(reconnect_policy),
            create_transport,
            connect_payload,
            transport: None,
            stopped: true,
        }
    }

    pub fn with_default_policy(connect_payload: Vec<u8>, create_transport: F) -> Self {
        Self::new(
            create_reconnect_policy(Default::default()),
            connect_payload,
            create_transport,
        )
    }

    pub fn transport(&self) -> Option<&CultNetRudpSocketTransportConnection> {
        self.transport.as_ref()
    }

    pub fn transport_mut(&mut self) -> Option<&mut CultNetRudpSocketTransportConnection> {
        self.transport.as_mut()
    }

    pub fn start(&mut self) -> Result<&mut CultNetRudpSocketTransportConnection> {
        self.stopped = false;
        self.reconnect_controller.reset();
        self.open_transport()
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.transport = None;
        self.reconnect_controller.reset();
    }

    pub fn mark_connected(&mut self) {
        self.reconnect_controller.reset();
    }

    pub fn handle_closed(
        &mut self,
        now_ms: u64,
        jitter_ms: u64,
    ) -> Option<CultNetReconnectDecision> {
        self.transport = None;
        if self.stopped {
            return None;
        }
        Some(self.reconnect_controller.record_failure(now_ms, jitter_ms))
    }

    pub fn reconnect_if_due(&mut self, now_ms: u64) -> Result<bool> {
        if self.stopped || !self.reconnect_controller.can_attempt(now_ms) {
            return Ok(false);
        }
        self.open_transport()?;
        Ok(true)
    }

    fn open_transport(&mut self) -> Result<&mut CultNetRudpSocketTransportConnection> {
        let mut transport = (self.create_transport)()?;
        transport.connect(self.connect_payload.clone())?;
        self.transport = Some(transport);
        Ok(self
            .transport
            .as_mut()
            .expect("RUDP reconnect loop opened a transport"))
    }
}

#[derive(Clone, Debug, Default)]
pub struct RudpTransportProfileOptions {
    pub transport_id: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub max_payload_bytes: Option<u32>,
    pub max_fragment_bytes: Option<u32>,
    pub max_pending_reliable_packets: Option<u32>,
    pub reconnect_policy: Option<CultNetReconnectPolicy>,
}

pub fn create_rudp_transport_profile(
    runtime_id: impl Into<String>,
    options: RudpTransportProfileOptions,
) -> CultNetTransportProfile {
    CultNetTransportProfile {
        schema_version: "cultnet.transport_profile.v0".to_string(),
        runtime_id: runtime_id.into(),
        transports: vec![CultNetTransportDescriptor {
            transport_id: options.transport_id.unwrap_or_else(|| "rudp".to_string()),
            protocol: CultNetTransportProtocol::Rudp,
            host: options.host,
            port: options.port,
            path: None,
            discovery_group: None,
            wire_contracts: Some(vec!["cultnet.schema.v0".to_string()]),
            reconnect_policy: Some(
                options
                    .reconnect_policy
                    .unwrap_or_else(|| create_reconnect_policy(Default::default())),
            ),
            channels: vec![
                CultNetTransportChannel {
                    channel_id: "schema".to_string(),
                    delivery: CultNetTransportDelivery::Reliable,
                    ordering: CultNetTransportOrdering::Ordered,
                    max_payload_bytes: options.max_payload_bytes,
                    max_fragment_bytes: options.max_fragment_bytes,
                    max_pending_reliable_packets: options.max_pending_reliable_packets,
                },
                CultNetTransportChannel {
                    channel_id: "latest".to_string(),
                    delivery: CultNetTransportDelivery::Unreliable,
                    ordering: CultNetTransportOrdering::Sequenced,
                    max_payload_bytes: options.max_payload_bytes,
                    max_fragment_bytes: options.max_fragment_bytes,
                    max_pending_reliable_packets: options.max_pending_reliable_packets,
                },
                CultNetTransportChannel {
                    channel_id: "realtime".to_string(),
                    delivery: CultNetTransportDelivery::Unreliable,
                    ordering: CultNetTransportOrdering::Unordered,
                    max_payload_bytes: options.max_payload_bytes,
                    max_fragment_bytes: options.max_fragment_bytes,
                    max_pending_reliable_packets: options.max_pending_reliable_packets,
                },
                CultNetTransportChannel {
                    channel_id: "media".to_string(),
                    delivery: CultNetTransportDelivery::Reliable,
                    ordering: CultNetTransportOrdering::Unordered,
                    max_payload_bytes: options.max_payload_bytes,
                    max_fragment_bytes: options.max_fragment_bytes,
                    max_pending_reliable_packets: options.max_pending_reliable_packets,
                },
            ],
        }],
    }
}

pub fn encode_rudp_packet(packet: &CultNetRudpPacket) -> Result<Vec<u8>> {
    let channel_id = packet.channel_id.as_bytes();
    if channel_id.len() > u8::MAX as usize {
        return Err(anyhow!(
            "CultNet RUDP channel id cannot exceed 255 UTF-8 bytes"
        ));
    }

    let header_bytes = RUDP_FIXED_HEADER_BYTES + channel_id.len();
    let mut wire = vec![0_u8; header_bytes + packet.payload.len()];
    wire[..4].copy_from_slice(&RUDP_MAGIC);
    wire[4] = RUDP_VERSION;
    wire[5] = packet_type_to_code(packet.packet_type);
    wire[6] = encode_flags(packet);
    wire[7] = header_bytes as u8;
    wire[8..12].copy_from_slice(&packet.connection_id.to_be_bytes());
    wire[12..16].copy_from_slice(&packet.sequence.to_be_bytes());
    wire[16..20].copy_from_slice(&packet.ack.to_be_bytes());
    wire[20..24].copy_from_slice(&packet.ack_mask.to_be_bytes());
    wire[24..26].copy_from_slice(&packet.fragment_id.to_be_bytes());
    wire[26..28].copy_from_slice(&packet.fragment_index.to_be_bytes());
    wire[28..30].copy_from_slice(&packet.fragment_count.to_be_bytes());
    wire[30..34].copy_from_slice(&(packet.payload.len() as u32).to_be_bytes());
    wire[34] = channel_id.len() as u8;
    wire[35] = 0;
    wire[RUDP_FIXED_HEADER_BYTES..header_bytes].copy_from_slice(channel_id);
    wire[header_bytes..].copy_from_slice(&packet.payload);
    Ok(wire)
}

pub fn decode_rudp_packet(wire: &[u8]) -> Result<CultNetRudpPacket> {
    if wire.len() < RUDP_FIXED_HEADER_BYTES {
        return Err(anyhow!(
            "CultNet RUDP packet is shorter than the fixed header"
        ));
    }
    if wire[..4] != RUDP_MAGIC {
        return Err(anyhow!("CultNet RUDP packet has the wrong magic"));
    }
    if wire[4] != RUDP_VERSION {
        return Err(anyhow!(
            "Unsupported CultNet RUDP packet version {}",
            wire[4]
        ));
    }

    let packet_type = packet_type_from_code(wire[5])?;
    let header_bytes = wire[7] as usize;
    let channel_id_len = wire[34] as usize;
    if header_bytes != RUDP_FIXED_HEADER_BYTES + channel_id_len {
        return Err(anyhow!(
            "CultNet RUDP packet header length does not match the channel id length"
        ));
    }
    let payload_len = u32::from_be_bytes(wire[30..34].try_into()?) as usize;
    if wire.len() != header_bytes + payload_len {
        return Err(anyhow!(
            "CultNet RUDP packet payload length does not match the packet size"
        ));
    }

    let flags = wire[6];
    Ok(CultNetRudpPacket {
        packet_type,
        reliable: (flags & 0b0000_0001) != 0,
        ordered: (flags & 0b0000_0010) != 0,
        sequenced: (flags & 0b0000_0100) != 0,
        connection_id: u32::from_be_bytes(wire[8..12].try_into()?),
        sequence: u32::from_be_bytes(wire[12..16].try_into()?),
        ack: u32::from_be_bytes(wire[16..20].try_into()?),
        ack_mask: u32::from_be_bytes(wire[20..24].try_into()?),
        fragment_id: u16::from_be_bytes(wire[24..26].try_into()?),
        fragment_index: u16::from_be_bytes(wire[26..28].try_into()?),
        fragment_count: u16::from_be_bytes(wire[28..30].try_into()?),
        channel_id: String::from_utf8(wire[RUDP_FIXED_HEADER_BYTES..header_bytes].to_vec())?,
        payload: wire[header_bytes..].to_vec(),
    })
}

fn encode_flags(packet: &CultNetRudpPacket) -> u8 {
    (if packet.reliable { 0b0000_0001 } else { 0 })
        | (if packet.ordered { 0b0000_0010 } else { 0 })
        | (if packet.sequenced { 0b0000_0100 } else { 0 })
        | (if packet.fragment_count > 0 {
            0b0000_1000
        } else {
            0
        })
}

fn packet_type_to_code(packet_type: CultNetRudpPacketType) -> u8 {
    match packet_type {
        CultNetRudpPacketType::Connect => 1,
        CultNetRudpPacketType::Accept => 2,
        CultNetRudpPacketType::Data => 3,
        CultNetRudpPacketType::Ack => 4,
        CultNetRudpPacketType::Ping => 5,
        CultNetRudpPacketType::Pong => 6,
        CultNetRudpPacketType::Disconnect => 7,
    }
}

fn channel_send_options(channel_id: &str, now_ms: u64) -> CultNetRudpSendOptions {
    match channel_id {
        "schema" => CultNetRudpSendOptions {
            reliable: true,
            ordered: true,
            sequenced: false,
            now_ms,
        },
        "hid.edge" | "hid.edge.ack" | "hid.subscribe" => CultNetRudpSendOptions {
            reliable: true,
            ordered: false,
            sequenced: false,
            now_ms,
        },
        "latest" => CultNetRudpSendOptions {
            reliable: false,
            ordered: false,
            sequenced: true,
            now_ms,
        },
        "media" => CultNetRudpSendOptions {
            reliable: true,
            ordered: false,
            sequenced: false,
            now_ms,
        },
        _ => CultNetRudpSendOptions {
            reliable: false,
            ordered: false,
            sequenced: false,
            now_ms,
        },
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn packet_type_from_code(code: u8) -> Result<CultNetRudpPacketType> {
    match code {
        1 => Ok(CultNetRudpPacketType::Connect),
        2 => Ok(CultNetRudpPacketType::Accept),
        3 => Ok(CultNetRudpPacketType::Data),
        4 => Ok(CultNetRudpPacketType::Ack),
        5 => Ok(CultNetRudpPacketType::Ping),
        6 => Ok(CultNetRudpPacketType::Pong),
        7 => Ok(CultNetRudpPacketType::Disconnect),
        _ => Err(anyhow!("Unsupported CultNet RUDP packet type {code}")),
    }
}

#[cfg(test)]
mod socket_transport_tests {
    use super::*;

    #[test]
    fn hid_control_channels_are_reliable_while_latest_state_is_replaceable() {
        for channel in ["hid.edge", "hid.edge.ack", "hid.subscribe"] {
            let options = channel_send_options(channel, 7);
            assert!(options.reliable, "{channel} must survive packet loss");
            assert!(
                !options.ordered,
                "{channel} uses application sequence and must not head-of-line block behind latest state"
            );
        }
        let latest = channel_send_options("latest", 7);
        assert!(!latest.reliable);
        assert!(latest.sequenced);
    }

    fn data_packet(sequence: u32) -> CultNetRudpPacket {
        CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Data,
            connection_id: 7,
            sequence,
            ack: 0,
            ack_mask: 0,
            channel_id: "realtime".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: vec![sequence as u8],
        }
    }

    #[test]
    fn fragment_ids_wrap_to_one_instead_of_sticking_at_u16_max() {
        let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions::default());
        session.next_fragment_id = u16::MAX;

        assert_eq!(session.allocate_fragment_id(), u16::MAX);
        assert_eq!(session.allocate_fragment_id(), 1);
        assert_eq!(session.allocate_fragment_id(), 2);
    }

    #[test]
    fn outbound_queue_preserves_allocated_packets_across_would_block() -> Result<()> {
        let mut pending = VecDeque::from([data_packet(41), data_packet(42)]);
        let mut attempts = 0_usize;
        let sent = flush_rudp_outbound_queue(&mut pending, |_| {
            attempts += 1;
            Err(std::io::Error::from(ErrorKind::WouldBlock))
        })?;
        assert_eq!(sent, 0);
        assert_eq!(attempts, 1);
        assert_eq!(pending.iter().map(|packet| packet.sequence).collect::<Vec<_>>(), vec![41, 42]);

        let mut sent_sequences = Vec::new();
        let sent = flush_rudp_outbound_queue(&mut pending, |wire| {
            sent_sequences.push(decode_rudp_packet(wire).unwrap().sequence);
            Ok(wire.len())
        })?;
        assert!(sent > 0);
        assert_eq!(sent_sequences, vec![41, 42]);
        assert!(pending.is_empty());
        Ok(())
    }

    #[test]
    fn exact_ack_releases_one_reliable_packet() -> Result<()> {
        let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: 7,
            ..CultNetRudpSessionOptions::default()
        });
        let connect = session.create_connect(1, Vec::new())?;
        session.receive(
            &CultNetRudpPacket {
                packet_type: CultNetRudpPacketType::Accept,
                connection_id: 7,
                sequence: 1,
                ack: connect.sequence,
                ack_mask: 0,
                channel_id: "control".to_string(),
                reliable: true,
                ordered: true,
                sequenced: false,
                fragment_id: 0,
                fragment_index: 0,
                fragment_count: 0,
                payload: Vec::new(),
            },
            2,
        )?;
        let packet = session.send(
            "media",
            vec![1, 2, 3],
            CultNetRudpSendOptions {
                reliable: true,
                now_ms: 10,
                ..CultNetRudpSendOptions::default()
            },
        )?;
        assert_eq!(session.pending_reliable_sequences(), vec![packet.sequence]);
        session.receive(
            &CultNetRudpPacket {
                packet_type: CultNetRudpPacketType::Ack,
                connection_id: 7,
                sequence: 99,
                ack: packet.sequence,
                ack_mask: 0,
                channel_id: "control".to_string(),
                reliable: false,
                ordered: false,
                sequenced: false,
                fragment_id: 0,
                fragment_index: 0,
                fragment_count: 0,
                payload: Vec::new(),
            },
            11,
        )?;
        assert!(session.pending_reliable_sequences().is_empty());
        Ok(())
    }

    #[test]
    fn ack_directly_names_late_reliable_packet_outside_cumulative_window() -> Result<()> {
        let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: 7,
            ..CultNetRudpSessionOptions::default()
        });
        session.connected = true;

        session.receive(&data_packet(100), 1)?;
        let mut late = data_packet(10);
        late.reliable = true;
        session.receive(&late, 2)?;

        let ack = session.create_ack();
        assert_eq!(ack.ack, 10);
        Ok(())
    }
}
