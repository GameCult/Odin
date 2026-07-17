use anyhow::{Context, Result, anyhow};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding, CultNetWireContract,
    decode_cultnet_message_from_slice, encode_cultnet_message_to_vec,
};
use odin_core::{
    MUNINN_MEDIA_AUDIO_PACKET_SCHEMA, MUNINN_MEDIA_AUDIO_PARITY_SHARD_SCHEMA,
    MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA, MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA,
    MUNINN_MEDIA_VIDEO_PARITY_SHARD_SCHEMA, MuninnMediaAudioPacketRecord,
    MuninnMediaAudioParityShardRecord, MuninnMediaReceiverFeedbackRecord,
    MuninnMediaVideoAccessUnitRecord, MuninnMediaVideoParityShardRecord,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

// Deadline-bound audio/video must use the experimental CultNet realtime lane.
// The `media` lane is reliable without expiry and can accumulate corpses behind
// loss; application-level parity/repair and playout deadlines own recovery.
pub const MUNINN_MEDIA_RUDP_CHANNEL: &str = "realtime";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoAccessUnit {
    pub bytes: Vec<u8>,
    pub keyframe: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NalUnit<'a> {
    start: usize,
    end: usize,
    payload: &'a [u8],
    nal_type: u8,
}

pub fn video_annex_b_access_units(codec: &str, input: &[u8]) -> Result<Vec<VideoAccessUnit>> {
    match normalized_video_codec(codec).as_deref() {
        Some("h264") => h264_annex_b_access_units(input),
        Some("h265") => h265_annex_b_access_units(input),
        Some("av1") => Err(anyhow!(
            "AV1 access unit splitting is not Annex B; provide an AV1 OBU packetizer"
        )),
        _ => Err(anyhow!("unsupported Annex B video codec {codec}")),
    }
}

pub fn h264_annex_b_access_units(input: &[u8]) -> Result<Vec<VideoAccessUnit>> {
    let nal_units = annex_b_nal_units(input, "H.264", h264_nal_type)?;
    let mut access_units = Vec::new();
    let mut current_start = None;
    let mut current_end = 0_usize;
    let mut current_has_vcl = false;
    let mut current_keyframe = false;

    for nal in nal_units {
        let starts_new = if nal.nal_type == 9 {
            current_start.is_some()
        } else if is_h264_vcl_nal(nal.nal_type) {
            current_has_vcl && h264_first_mb_in_slice(nal.payload).unwrap_or(1) == 0
        } else {
            false
        };

        if starts_new {
            if let Some(start) = current_start {
                access_units.push(VideoAccessUnit {
                    bytes: input[start..current_end].to_vec(),
                    keyframe: current_keyframe,
                });
            }
            current_start = None;
            current_has_vcl = false;
            current_keyframe = false;
        }

        if current_start.is_none() {
            current_start = Some(nal.start);
        }
        current_end = nal.end;
        if is_h264_vcl_nal(nal.nal_type) {
            current_has_vcl = true;
        }
        if nal.nal_type == 5 {
            current_keyframe = true;
        }
    }

    if let Some(start) = current_start {
        access_units.push(VideoAccessUnit {
            bytes: input[start..current_end].to_vec(),
            keyframe: current_keyframe,
        });
    }

    Ok(access_units)
}

pub fn h265_annex_b_access_units(input: &[u8]) -> Result<Vec<VideoAccessUnit>> {
    let nal_units = annex_b_nal_units(input, "H.265", h265_nal_type)?;
    let mut access_units = Vec::new();
    let mut current_start = None;
    let mut current_end = 0_usize;
    let mut current_has_vcl = false;
    let mut current_keyframe = false;

    for nal in nal_units {
        let starts_new = if nal.nal_type == 35 {
            current_start.is_some()
        } else if is_h265_vcl_nal(nal.nal_type) {
            current_has_vcl && h265_first_slice_segment_in_pic(nal.payload).unwrap_or(false)
        } else {
            current_has_vcl && is_h265_pre_vcl_boundary_nal(nal.nal_type)
        };

        if starts_new {
            if let Some(start) = current_start {
                access_units.push(VideoAccessUnit {
                    bytes: input[start..current_end].to_vec(),
                    keyframe: current_keyframe,
                });
            }
            current_start = None;
            current_has_vcl = false;
            current_keyframe = false;
        }

        if current_start.is_none() {
            current_start = Some(nal.start);
        }
        current_end = nal.end;
        if is_h265_vcl_nal(nal.nal_type) {
            current_has_vcl = true;
        }
        if is_h265_irap_nal(nal.nal_type) {
            current_keyframe = true;
        }
    }

    if let Some(start) = current_start {
        access_units.push(VideoAccessUnit {
            bytes: input[start..current_end].to_vec(),
            keyframe: current_keyframe,
        });
    }

    Ok(access_units)
}

pub struct VideoFramePacketizeOptions<'a> {
    pub stream_id: &'a str,
    pub session_id: &'a str,
    pub codec: &'a str,
    pub frame_id: u64,
    pub pts_ticks: i64,
    pub duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_ticks: i64,
    pub max_payload_bytes: usize,
}

pub struct VideoAnnexBStreamPacketizeOptions<'a> {
    pub stream_id: &'a str,
    pub session_id: &'a str,
    pub codec: &'a str,
    pub first_frame_id: u64,
    pub first_pts_ticks: i64,
    pub frame_duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_delay_ticks: i64,
    pub max_payload_bytes: usize,
}

pub struct VideoAnnexBStreamWireOptions<'a> {
    pub packetize: VideoAnnexBStreamPacketizeOptions<'a>,
    pub stored_at: &'a str,
    pub source_runtime_id: &'a str,
    pub source_role: &'a str,
}

pub struct VideoAnnexBStreamSendConfig {
    pub stream_id: String,
    pub session_id: String,
    pub codec: String,
    pub first_frame_id: u64,
    pub first_pts_ticks: i64,
    pub frame_duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_delay_ticks: i64,
    pub max_payload_bytes: usize,
    pub max_pending_bytes: usize,
    pub source_runtime_id: String,
    pub source_role: String,
}

pub struct AudioPacketizeOptions<'a> {
    pub stream_id: &'a str,
    pub session_id: &'a str,
    pub codec: &'a str,
    pub packet_id: u64,
    pub pts_ticks: i64,
    pub duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_ticks: i64,
}

pub struct AudioPacketWireOptions<'a> {
    pub packetize: AudioPacketizeOptions<'a>,
    pub stored_at: &'a str,
    pub source_runtime_id: &'a str,
    pub source_role: &'a str,
}

pub struct AudioAdtsStreamSendConfig {
    pub stream_id: String,
    pub session_id: String,
    pub codec: String,
    pub first_packet_id: u64,
    pub first_pts_ticks: i64,
    pub packet_duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_delay_ticks: i64,
    pub max_pending_bytes: usize,
    pub source_runtime_id: String,
    pub source_role: String,
}

pub struct AudioPcmStreamSendConfig {
    pub stream_id: String,
    pub session_id: String,
    pub codec: String,
    pub first_packet_id: u64,
    pub first_pts_ticks: i64,
    pub packet_duration_ticks: u32,
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub deadline_delay_ticks: i64,
    pub channels: u32,
    pub bytes_per_sample: u32,
    pub max_pending_bytes: usize,
    pub source_runtime_id: String,
    pub source_role: String,
}

pub struct ReceiverFeedbackOptions<'a> {
    pub stream_id: &'a str,
    pub session_id: &'a str,
    pub receiver_id: &'a str,
    pub highest_decodable_frame_id: Option<u64>,
    pub missing_frame_ids: Vec<u64>,
    pub missing_video_chunk_keys: Vec<String>,
    pub late_frame_ids: Vec<u64>,
    pub requested_keyframe: bool,
    pub jitter_us: i64,
    pub decode_queue_us: i64,
    pub observed_at: &'a str,
}

pub struct ExpiredVideoFrameFeedbackOptions<'a> {
    pub receiver_id: &'a str,
    pub jitter_us: i64,
    pub decode_queue_us: i64,
    pub observed_at: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MuninnMediaWireRecord {
    Video(MuninnMediaVideoAccessUnitRecord),
    VideoParity(MuninnMediaVideoParityShardRecord),
    Audio(MuninnMediaAudioPacketRecord),
    AudioParity(MuninnMediaAudioParityShardRecord),
    Feedback(MuninnMediaReceiverFeedbackRecord),
}

#[derive(Serialize)]
struct VideoAccessUnitWirePayload<'a>(
    &'a str,
    &'a str,
    u64,
    &'a str,
    i64,
    u32,
    u32,
    u32,
    bool,
    Option<u64>,
    i64,
    u16,
    u16,
    #[serde(with = "serde_bytes")] &'a [u8],
);

#[derive(Serialize)]
struct VideoParityShardWirePayload<'a>(
    &'a str,
    &'a str,
    u64,
    &'a str,
    i64,
    u32,
    u32,
    u32,
    bool,
    Option<u64>,
    i64,
    u16,
    u16,
    u16,
    u32,
    u32,
    #[serde(with = "serde_bytes")] &'a [u8],
);

#[derive(Serialize)]
struct AudioPacketWirePayload<'a>(
    &'a str,
    &'a str,
    u64,
    &'a str,
    i64,
    u32,
    u32,
    u32,
    i64,
    #[serde(with = "serde_bytes")] &'a [u8],
);

#[derive(Serialize)]
struct AudioParityShardWirePayload<'a>(
    &'a str,
    &'a str,
    u64,
    &'a str,
    i64,
    u32,
    u32,
    u32,
    i64,
    u16,
    u16,
    u16,
    u32,
    #[serde(with = "serde_bytes")] &'a [u8],
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MuninnMediaSendPayload {
    pub channel_id: &'static str,
    pub payload: Vec<u8>,
}

pub struct VideoAnnexBStreamSendState {
    config: VideoAnnexBStreamSendConfig,
    pending: Vec<u8>,
    next_frame_id: u64,
    next_pts_ticks: i64,
}

impl VideoAnnexBStreamSendState {
    pub fn new(config: VideoAnnexBStreamSendConfig) -> Result<Self> {
        if matches!(normalized_video_codec(&config.codec), None | Some("av1")) {
            return Err(anyhow!(
                "Annex B stream sender requires H.264/AVC or H.265/HEVC codec"
            ));
        }
        if config.stream_id.is_empty() {
            return Err(anyhow!("stream_id must be non-empty"));
        }
        if config.session_id.is_empty() {
            return Err(anyhow!("session_id must be non-empty"));
        }
        if config.frame_duration_ticks == 0 {
            return Err(anyhow!("frame_duration_ticks must be greater than zero"));
        }
        if config.timebase_num == 0 || config.timebase_den == 0 {
            return Err(anyhow!("video timebase must be non-zero"));
        }
        if config.deadline_delay_ticks < 0 {
            return Err(anyhow!("deadline_delay_ticks must be non-negative"));
        }
        if config.max_payload_bytes == 0 {
            return Err(anyhow!("max_payload_bytes must be greater than zero"));
        }
        if config.max_pending_bytes == 0 {
            return Err(anyhow!("max_pending_bytes must be greater than zero"));
        }
        if config.source_runtime_id.is_empty() {
            return Err(anyhow!("source_runtime_id must be non-empty"));
        }
        if config.source_role.is_empty() {
            return Err(anyhow!("source_role must be non-empty"));
        }

        Ok(Self {
            next_frame_id: config.first_frame_id,
            next_pts_ticks: config.first_pts_ticks,
            config,
            pending: Vec::new(),
        })
    }

    pub fn push(&mut self, stored_at: &str, bytes: &[u8]) -> Result<Vec<MuninnMediaSendPayload>> {
        if !bytes.is_empty() {
            self.pending.extend_from_slice(bytes);
        }
        if self.pending.len() > self.config.max_pending_bytes {
            return Err(anyhow!(
                "Annex B stream sender pending buffer exceeded {} bytes without a complete frame",
                self.config.max_pending_bytes
            ));
        }
        self.emit_available(stored_at, false)
    }

    pub fn finish(&mut self, stored_at: &str) -> Result<Vec<MuninnMediaSendPayload>> {
        self.emit_available(stored_at, true)
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    pub fn next_frame_id(&self) -> u64 {
        self.next_frame_id
    }

    fn emit_available(
        &mut self,
        stored_at: &str,
        flush_last: bool,
    ) -> Result<Vec<MuninnMediaSendPayload>> {
        if stored_at.is_empty() {
            return Err(anyhow!("stored_at must be non-empty"));
        }
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }

        let access_units = match video_annex_b_access_units(&self.config.codec, &self.pending) {
            Ok(access_units) => access_units,
            Err(error)
                if error.to_string().contains("has no start codes")
                    || error.to_string().contains("has no NAL payloads") =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error),
        };
        let emit_count = if flush_last {
            access_units.len()
        } else {
            access_units.len().saturating_sub(1)
        };
        if emit_count == 0 {
            return Ok(Vec::new());
        }

        let mut send_payloads = Vec::new();
        let mut emitted_bytes = 0_usize;
        for access_unit in access_units.iter().take(emit_count) {
            emitted_bytes = emitted_bytes
                .checked_add(access_unit.bytes.len())
                .ok_or_else(|| anyhow!("emitted Annex B byte count overflow"))?;
            let deadline_ticks = self
                .next_pts_ticks
                .checked_add(self.config.deadline_delay_ticks)
                .ok_or_else(|| anyhow!("video deadline_ticks overflow"))?;
            let records = packetize_video_access_unit(
                VideoFramePacketizeOptions {
                    stream_id: &self.config.stream_id,
                    session_id: &self.config.session_id,
                    codec: &self.config.codec,
                    frame_id: self.next_frame_id,
                    pts_ticks: self.next_pts_ticks,
                    duration_ticks: self.config.frame_duration_ticks,
                    timebase_num: self.config.timebase_num,
                    timebase_den: self.config.timebase_den,
                    deadline_ticks,
                    max_payload_bytes: self.config.max_payload_bytes,
                },
                access_unit,
            )?;
            let wire_records = video_wire_records_with_parity(&records)?;
            send_payloads.extend(wire_payloads_to_media_send(encode_media_wire_records(
                &wire_records,
                stored_at,
                &self.config.source_runtime_id,
                &self.config.source_role,
            )?));
            self.advance_frame_clock()?;
        }

        self.pending.drain(..emitted_bytes);
        Ok(send_payloads)
    }

    fn advance_frame_clock(&mut self) -> Result<()> {
        self.next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("video frame_id overflow"))?;
        self.next_pts_ticks = self
            .next_pts_ticks
            .checked_add(i64::from(self.config.frame_duration_ticks))
            .ok_or_else(|| anyhow!("video pts_ticks overflow"))?;
        Ok(())
    }
}

pub struct AudioAdtsStreamSendState {
    config: AudioAdtsStreamSendConfig,
    pending: Vec<u8>,
    next_packet_id: u64,
    next_pts_ticks: i64,
}

impl AudioAdtsStreamSendState {
    pub fn new(config: AudioAdtsStreamSendConfig) -> Result<Self> {
        if !matches!(
            config.codec.trim().to_ascii_lowercase().as_str(),
            "aac" | "aac-adts" | "adts" | "audio/aac"
        ) {
            return Err(anyhow!("ADTS stream sender requires AAC/ADTS codec"));
        }
        if config.stream_id.is_empty() {
            return Err(anyhow!("stream_id must be non-empty"));
        }
        if config.session_id.is_empty() {
            return Err(anyhow!("session_id must be non-empty"));
        }
        if config.packet_duration_ticks == 0 {
            return Err(anyhow!("packet_duration_ticks must be greater than zero"));
        }
        if config.timebase_num == 0 || config.timebase_den == 0 {
            return Err(anyhow!("audio timebase must be non-zero"));
        }
        if config.deadline_delay_ticks < 0 {
            return Err(anyhow!("deadline_delay_ticks must be non-negative"));
        }
        if config.max_pending_bytes == 0 {
            return Err(anyhow!("max_pending_bytes must be greater than zero"));
        }
        if config.source_runtime_id.is_empty() {
            return Err(anyhow!("source_runtime_id must be non-empty"));
        }
        if config.source_role.is_empty() {
            return Err(anyhow!("source_role must be non-empty"));
        }

        Ok(Self {
            next_packet_id: config.first_packet_id,
            next_pts_ticks: config.first_pts_ticks,
            config,
            pending: Vec::new(),
        })
    }

    pub fn push(&mut self, stored_at: &str, bytes: &[u8]) -> Result<Vec<MuninnMediaSendPayload>> {
        if !bytes.is_empty() {
            self.pending.extend_from_slice(bytes);
        }
        if self.pending.len() > self.config.max_pending_bytes {
            return Err(anyhow!(
                "ADTS stream sender pending buffer exceeded {} bytes without a complete packet",
                self.config.max_pending_bytes
            ));
        }
        self.emit_available(stored_at)
    }

    pub fn finish(&mut self, stored_at: &str) -> Result<Vec<MuninnMediaSendPayload>> {
        let payloads = self.emit_available(stored_at)?;
        if !self.pending.is_empty() {
            return Err(anyhow!(
                "ADTS stream sender finished with {} trailing bytes",
                self.pending.len()
            ));
        }
        Ok(payloads)
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    pub fn next_packet_id(&self) -> u64 {
        self.next_packet_id
    }

    fn emit_available(&mut self, stored_at: &str) -> Result<Vec<MuninnMediaSendPayload>> {
        if stored_at.is_empty() {
            return Err(anyhow!("stored_at must be non-empty"));
        }

        let frames = complete_adts_frames(&self.pending)?;
        if frames.consumed_bytes == 0 {
            return Ok(Vec::new());
        }

        let mut payloads = Vec::new();
        for frame in frames.frames {
            let deadline_ticks = self
                .next_pts_ticks
                .checked_add(self.config.deadline_delay_ticks)
                .ok_or_else(|| anyhow!("audio deadline_ticks overflow"))?;
            payloads.push(audio_packet_send_payload(
                AudioPacketWireOptions {
                    packetize: AudioPacketizeOptions {
                        stream_id: &self.config.stream_id,
                        session_id: &self.config.session_id,
                        codec: &self.config.codec,
                        packet_id: self.next_packet_id,
                        pts_ticks: self.next_pts_ticks,
                        duration_ticks: self.config.packet_duration_ticks,
                        timebase_num: self.config.timebase_num,
                        timebase_den: self.config.timebase_den,
                        deadline_ticks,
                    },
                    stored_at,
                    source_runtime_id: &self.config.source_runtime_id,
                    source_role: &self.config.source_role,
                },
                &frame,
            )?);
            self.advance_packet_clock()?;
        }

        self.pending.drain(..frames.consumed_bytes);
        Ok(payloads)
    }

    fn advance_packet_clock(&mut self) -> Result<()> {
        self.next_packet_id = self
            .next_packet_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("audio packet_id overflow"))?;
        self.next_pts_ticks = self
            .next_pts_ticks
            .checked_add(i64::from(self.config.packet_duration_ticks))
            .ok_or_else(|| anyhow!("audio pts_ticks overflow"))?;
        Ok(())
    }
}

pub struct AudioPcmStreamSendState {
    config: AudioPcmStreamSendConfig,
    pending: Vec<u8>,
    next_packet_id: u64,
    next_pts_ticks: i64,
    fec_block: Vec<MuninnMediaAudioPacketRecord>,
}

impl AudioPcmStreamSendState {
    pub fn new(config: AudioPcmStreamSendConfig) -> Result<Self> {
        if config.stream_id.is_empty() {
            return Err(anyhow!("stream_id must be non-empty"));
        }
        if config.session_id.is_empty() {
            return Err(anyhow!("session_id must be non-empty"));
        }
        if config.codec.is_empty() {
            return Err(anyhow!("codec must be non-empty"));
        }
        if config.packet_duration_ticks == 0 {
            return Err(anyhow!("packet_duration_ticks must be greater than zero"));
        }
        if config.timebase_num == 0 || config.timebase_den == 0 {
            return Err(anyhow!("audio timebase must be non-zero"));
        }
        if config.deadline_delay_ticks < 0 {
            return Err(anyhow!("deadline_delay_ticks must be non-negative"));
        }
        if config.channels == 0 {
            return Err(anyhow!("audio channels must be non-zero"));
        }
        if config.bytes_per_sample == 0 {
            return Err(anyhow!("audio bytes_per_sample must be non-zero"));
        }
        if config.max_pending_bytes == 0 {
            return Err(anyhow!("max_pending_bytes must be greater than zero"));
        }
        if config.source_runtime_id.is_empty() {
            return Err(anyhow!("source_runtime_id must be non-empty"));
        }
        if config.source_role.is_empty() {
            return Err(anyhow!("source_role must be non-empty"));
        }

        Ok(Self {
            next_packet_id: config.first_packet_id,
            next_pts_ticks: config.first_pts_ticks,
            config,
            pending: Vec::new(),
            fec_block: Vec::with_capacity(AUDIO_FEC_DATA_SHARDS),
        })
    }

    pub fn push(&mut self, stored_at: &str, bytes: &[u8]) -> Result<Vec<MuninnMediaSendPayload>> {
        if !bytes.is_empty() {
            self.pending.extend_from_slice(bytes);
        }
        if self.pending.len() > self.config.max_pending_bytes {
            return Err(anyhow!(
                "PCM stream sender pending buffer exceeded {} bytes without a complete packet",
                self.config.max_pending_bytes
            ));
        }
        self.emit_available(stored_at)
    }

    pub fn finish(&mut self, stored_at: &str) -> Result<Vec<MuninnMediaSendPayload>> {
        let payloads = self.emit_available(stored_at)?;
        if !self.pending.is_empty() {
            return Err(anyhow!(
                "PCM stream sender finished with {} trailing bytes",
                self.pending.len()
            ));
        }
        Ok(payloads)
    }

    fn emit_available(&mut self, stored_at: &str) -> Result<Vec<MuninnMediaSendPayload>> {
        if stored_at.is_empty() {
            return Err(anyhow!("stored_at must be non-empty"));
        }

        let bytes_per_frame = usize::try_from(self.config.channels)
            .ok()
            .and_then(|channels| {
                usize::try_from(self.config.bytes_per_sample)
                    .ok()
                    .map(|bytes_per_sample| channels.saturating_mul(bytes_per_sample))
            })
            .ok_or_else(|| anyhow!("PCM stream sender bytes_per_frame overflow"))?;
        let bytes_per_packet = bytes_per_frame
            .checked_mul(self.config.packet_duration_ticks as usize)
            .ok_or_else(|| anyhow!("PCM stream sender bytes_per_packet overflow"))?;
        if bytes_per_packet == 0 {
            return Err(anyhow!(
                "PCM stream sender bytes_per_packet must be non-zero"
            ));
        }
        let complete_packets = self.pending.len() / bytes_per_packet;
        if complete_packets == 0 {
            return Ok(Vec::new());
        }

        let mut payloads = Vec::with_capacity(complete_packets);
        for packet_index in 0..complete_packets {
            let start = packet_index * bytes_per_packet;
            let end = start + bytes_per_packet;
            let deadline_ticks = self
                .next_pts_ticks
                .checked_add(self.config.deadline_delay_ticks)
                .ok_or_else(|| anyhow!("audio deadline_ticks overflow"))?;
            let record = packetize_audio_packet(
                AudioPacketizeOptions {
                    stream_id: &self.config.stream_id,
                    session_id: &self.config.session_id,
                    codec: &self.config.codec,
                    packet_id: self.next_packet_id,
                    pts_ticks: self.next_pts_ticks,
                    duration_ticks: self.config.packet_duration_ticks,
                    timebase_num: self.config.timebase_num,
                    timebase_den: self.config.timebase_den,
                    deadline_ticks,
                },
                &self.pending[start..end],
            )?;
            payloads.push(wire_payload_to_media_send(encode_media_wire_record(
                &MuninnMediaWireRecord::Audio(record.clone()),
                stored_at,
                &self.config.source_runtime_id,
                &self.config.source_role,
            )?));
            self.fec_block.push(record);
            if self.fec_block.len() == AUDIO_FEC_DATA_SHARDS {
                payloads.extend(audio_fec_parity_send_payloads(
                    &self.fec_block,
                    stored_at,
                    &self.config.source_runtime_id,
                    &self.config.source_role,
                )?);
                self.fec_block.clear();
            }
            self.advance_packet_clock()?;
        }

        self.pending.drain(..complete_packets * bytes_per_packet);
        Ok(payloads)
    }

    fn advance_packet_clock(&mut self) -> Result<()> {
        self.next_packet_id = self
            .next_packet_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("audio packet_id overflow"))?;
        self.next_pts_ticks = self
            .next_pts_ticks
            .checked_add(i64::from(self.config.packet_duration_ticks))
            .ok_or_else(|| anyhow!("audio pts_ticks overflow"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VideoChunkKey {
    pub frame_id: u64,
    pub chunk_index: u16,
}

impl VideoChunkKey {
    pub fn new(frame_id: u64, chunk_index: u16) -> Self {
        Self {
            frame_id,
            chunk_index,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        if value.matches(':').count() != 1 {
            return Err(anyhow!(
                "video chunk key must contain exactly one ':' separator"
            ));
        }
        let (frame_id, chunk_index) = value
            .split_once(':')
            .ok_or_else(|| anyhow!("video chunk key must be '<frame_id>:<chunk_index>'"))?;
        Ok(Self {
            frame_id: frame_id
                .parse()
                .map_err(|_| anyhow!("video chunk key frame_id must be an integer"))?,
            chunk_index: chunk_index
                .parse()
                .map_err(|_| anyhow!("video chunk key chunk_index must be an integer"))?,
        })
    }

    pub fn as_feedback_key(&self) -> String {
        format!("{}:{}", self.frame_id, self.chunk_index)
    }
}

pub fn video_chunk_feedback_key(frame_id: u64, chunk_index: u16) -> String {
    VideoChunkKey::new(frame_id, chunk_index).as_feedback_key()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiredVideoFrame {
    pub key: VideoFrameKey,
    pub deadline_ticks: i64,
    pub missing_video_chunk_keys: Vec<String>,
    pub decode_chain_invalidated: bool,
}

#[derive(Clone, Debug)]
pub struct VideoFrameAssembly {
    stream_id: String,
    session_id: String,
    frame_id: u64,
    chunk_count: u16,
    chunks: BTreeMap<u16, MuninnMediaVideoAccessUnitRecord>,
}

impl VideoFrameAssembly {
    pub fn new(first_chunk: MuninnMediaVideoAccessUnitRecord) -> Result<Self> {
        if first_chunk.chunk_count == 0 {
            return Err(anyhow!("video frame assembly chunk_count must be non-zero"));
        }
        if first_chunk.chunk_index >= first_chunk.chunk_count {
            return Err(anyhow!(
                "video frame assembly chunk_index {} is outside chunk_count {}",
                first_chunk.chunk_index,
                first_chunk.chunk_count
            ));
        }
        if first_chunk.payload.is_empty() {
            return Err(anyhow!(
                "video frame assembly chunk payload must be non-empty"
            ));
        }

        let mut chunks = BTreeMap::new();
        chunks.insert(first_chunk.chunk_index, first_chunk.clone());
        Ok(Self {
            stream_id: first_chunk.stream_id,
            session_id: first_chunk.session_id,
            frame_id: first_chunk.frame_id,
            chunk_count: first_chunk.chunk_count,
            chunks,
        })
    }

    pub fn insert(&mut self, chunk: MuninnMediaVideoAccessUnitRecord) -> Result<()> {
        self.require_matching_frame(&chunk)?;
        if self.chunks.contains_key(&chunk.chunk_index) {
            return Err(anyhow!(
                "video frame assembly has duplicate chunk_index {}",
                chunk.chunk_index
            ));
        }
        self.chunks.insert(chunk.chunk_index, chunk);
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.chunks.len() == self.chunk_count as usize
    }

    pub fn missing_video_chunk_keys(&self) -> Vec<String> {
        (0..self.chunk_count)
            .filter(|chunk_index| !self.chunks.contains_key(chunk_index))
            .map(|chunk_index| video_chunk_feedback_key(self.frame_id, chunk_index))
            .collect()
    }

    pub fn deadline_ticks(&self) -> i64 {
        self.chunks
            .values()
            .next()
            .map(|chunk| chunk.deadline_ticks)
            .unwrap_or_default()
    }

    pub fn invalidates_decode_chain(&self) -> bool {
        self.chunks
            .values()
            .next()
            .is_some_and(|chunk| chunk.keyframe || chunk.dependency_frame_id.is_some())
    }

    pub fn reassemble(&self) -> Result<VideoAccessUnit> {
        let chunks = self.chunks.values().cloned().collect::<Vec<_>>();
        reassemble_video_access_unit(&chunks)
    }

    fn require_matching_frame(&self, chunk: &MuninnMediaVideoAccessUnitRecord) -> Result<()> {
        if chunk.stream_id != self.stream_id {
            return Err(anyhow!("video frame assembly received mixed stream_id"));
        }
        if chunk.session_id != self.session_id {
            return Err(anyhow!("video frame assembly received mixed session_id"));
        }
        if chunk.frame_id != self.frame_id {
            return Err(anyhow!("video frame assembly received mixed frame_id"));
        }
        if chunk.chunk_count != self.chunk_count {
            return Err(anyhow!("video frame assembly received mixed chunk_count"));
        }
        if chunk.chunk_index >= self.chunk_count {
            return Err(anyhow!(
                "video frame assembly chunk_index {} is outside chunk_count {}",
                chunk.chunk_index,
                self.chunk_count
            ));
        }
        if chunk.payload.is_empty() {
            return Err(anyhow!(
                "video frame assembly chunk payload must be non-empty"
            ));
        }
        if let Some(first) = self.chunks.values().next() {
            if chunk.codec != first.codec
                || chunk.pts_ticks != first.pts_ticks
                || chunk.duration_ticks != first.duration_ticks
                || chunk.timebase_num != first.timebase_num
                || chunk.timebase_den != first.timebase_den
                || chunk.deadline_ticks != first.deadline_ticks
                || chunk.keyframe != first.keyframe
                || chunk.dependency_frame_id != first.dependency_frame_id
            {
                return Err(anyhow!(
                    "video frame assembly received mixed media metadata"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VideoFrameKey {
    pub stream_id: String,
    pub session_id: String,
    pub frame_id: u64,
}

impl VideoFrameKey {
    pub fn from_chunk(chunk: &MuninnMediaVideoAccessUnitRecord) -> Self {
        Self {
            stream_id: chunk.stream_id.clone(),
            session_id: chunk.session_id.clone(),
            frame_id: chunk.frame_id,
        }
    }
}

#[derive(Default)]
pub struct VideoFrameAssemblySet {
    frames: BTreeMap<VideoFrameKey, VideoFrameAssembly>,
}

impl VideoFrameAssemblySet {
    pub fn insert_chunk(
        &mut self,
        chunk: MuninnMediaVideoAccessUnitRecord,
    ) -> Result<Option<VideoAccessUnit>> {
        let key = VideoFrameKey::from_chunk(&chunk);
        if let Some(assembly) = self.frames.get_mut(&key) {
            assembly.insert(chunk)?;
            if assembly.is_complete() {
                let access_unit = assembly.reassemble()?;
                self.frames.remove(&key);
                return Ok(Some(access_unit));
            }
            return Ok(None);
        }

        let assembly = VideoFrameAssembly::new(chunk)?;
        if assembly.is_complete() {
            return Ok(Some(assembly.reassemble()?));
        }
        self.frames.insert(key, assembly);
        Ok(None)
    }

    pub fn missing_video_chunk_keys(&self, key: &VideoFrameKey) -> Vec<String> {
        self.frames
            .get(key)
            .map(VideoFrameAssembly::missing_video_chunk_keys)
            .unwrap_or_default()
    }

    pub fn pending_frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn expire_late_frames(&mut self, now_ticks: i64) -> Vec<ExpiredVideoFrame> {
        let expired_keys = self
            .frames
            .iter()
            .filter_map(|(key, assembly)| {
                if assembly.deadline_ticks() <= now_ticks {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        expired_keys
            .into_iter()
            .filter_map(|key| {
                self.frames.remove(&key).map(|assembly| ExpiredVideoFrame {
                    key,
                    deadline_ticks: assembly.deadline_ticks(),
                    missing_video_chunk_keys: assembly.missing_video_chunk_keys(),
                    decode_chain_invalidated: assembly.invalidates_decode_chain(),
                })
            })
            .collect()
    }
}

pub fn packetize_video_access_unit(
    options: VideoFramePacketizeOptions<'_>,
    access_unit: &VideoAccessUnit,
) -> Result<Vec<MuninnMediaVideoAccessUnitRecord>> {
    if options.stream_id.is_empty() {
        return Err(anyhow!("stream_id must be non-empty"));
    }
    if options.session_id.is_empty() {
        return Err(anyhow!("session_id must be non-empty"));
    }
    if options.codec.is_empty() {
        return Err(anyhow!("codec must be non-empty"));
    }
    if options.timebase_num == 0 || options.timebase_den == 0 {
        return Err(anyhow!("video timebase must be non-zero"));
    }
    if options.duration_ticks == 0 {
        return Err(anyhow!("video duration_ticks must be greater than zero"));
    }
    if options.deadline_ticks < options.pts_ticks {
        return Err(anyhow!("video deadline_ticks must not precede pts_ticks"));
    }
    if options.max_payload_bytes == 0 {
        return Err(anyhow!("max_payload_bytes must be greater than zero"));
    }
    if access_unit.bytes.is_empty() {
        return Err(anyhow!("access unit payload must be non-empty"));
    }

    let chunk_count = access_unit.bytes.len().div_ceil(options.max_payload_bytes);
    if chunk_count > u16::MAX as usize {
        return Err(anyhow!("video access unit requires more than 65535 chunks"));
    }

    let mut records = Vec::with_capacity(chunk_count);
    for (chunk_index, payload) in access_unit
        .bytes
        .chunks(options.max_payload_bytes)
        .enumerate()
    {
        records.push(MuninnMediaVideoAccessUnitRecord {
            stream_id: options.stream_id.to_string(),
            session_id: options.session_id.to_string(),
            frame_id: options.frame_id,
            codec: options.codec.to_string(),
            pts_ticks: options.pts_ticks,
            duration_ticks: options.duration_ticks,
            timebase_num: options.timebase_num,
            timebase_den: options.timebase_den,
            keyframe: access_unit.keyframe,
            dependency_frame_id: if access_unit.keyframe
                || !access_unit_references_previous_frame(options.codec, access_unit)
            {
                None
            } else {
                options.frame_id.checked_sub(1)
            },
            deadline_ticks: options.deadline_ticks,
            chunk_index: chunk_index as u16,
            chunk_count: chunk_count as u16,
            payload: payload.to_vec(),
        });
    }

    Ok(records)
}

const MUNINN_VIDEO_PARITY_SHARDS: u16 = 16;

fn video_fec_coefficient(parity_index: u16, parity_count: u16, data_index: u16) -> u8 {
    debug_assert!(u32::from(parity_count) + u32::from(data_index) <= 255);
    gf256_inv((parity_index as u8) ^ ((parity_count + data_index) as u8))
}

pub fn build_video_parity_shards(
    chunks: &[MuninnMediaVideoAccessUnitRecord],
) -> Result<Vec<MuninnMediaVideoParityShardRecord>> {
    if chunks.len() <= 1 {
        return Ok(Vec::new());
    }
    let first = chunks
        .first()
        .ok_or_else(|| anyhow!("video parity requires at least one chunk"))?;
    validate_video_record(first)?;
    if chunks.len() != first.chunk_count as usize {
        return Err(anyhow!(
            "video parity requires all chunks: expected {}, received {}",
            first.chunk_count,
            chunks.len()
        ));
    }

    let mut by_index = BTreeMap::new();
    for chunk in chunks {
        validate_video_record(chunk)?;
        if chunk.stream_id != first.stream_id
            || chunk.session_id != first.session_id
            || chunk.frame_id != first.frame_id
            || chunk.codec != first.codec
            || chunk.pts_ticks != first.pts_ticks
            || chunk.duration_ticks != first.duration_ticks
            || chunk.timebase_num != first.timebase_num
            || chunk.timebase_den != first.timebase_den
            || chunk.keyframe != first.keyframe
            || chunk.dependency_frame_id != first.dependency_frame_id
            || chunk.deadline_ticks != first.deadline_ticks
            || chunk.chunk_count != first.chunk_count
        {
            return Err(anyhow!("video parity chunks have mixed metadata"));
        }
        if by_index.insert(chunk.chunk_index, chunk).is_some() {
            return Err(anyhow!(
                "video parity received duplicate chunk_index {}",
                chunk.chunk_index
            ));
        }
    }

    let mut chunk_payload_len = 0_usize;
    let mut last_chunk_payload_bytes = 0_u32;
    for index in 0..first.chunk_count {
        let chunk = by_index
            .get(&index)
            .ok_or_else(|| anyhow!("video parity missing chunk_index {index}"))?;
        chunk_payload_len = chunk_payload_len.max(chunk.payload.len());
        if index == first.chunk_count - 1 {
            last_chunk_payload_bytes = u32::try_from(chunk.payload.len())
                .context("video parity last chunk payload length exceeds u32")?;
        }
    }
    let chunk_payload_bytes = u32::try_from(chunk_payload_len)
        .context("video parity chunk payload length exceeds u32")?;
    let parity_count = first
        .chunk_count
        .min(MUNINN_VIDEO_PARITY_SHARDS)
        .min(256_u16.saturating_sub(first.chunk_count));
    let mut records = Vec::with_capacity(parity_count as usize);
    for parity_index in 0..parity_count {
        let mut parity = vec![0_u8; chunk_payload_len];
        for index in 0..first.chunk_count {
            let chunk = by_index.get(&index).expect("chunk index was checked above");
            let coefficient = video_fec_coefficient(parity_index, parity_count, index);
            for (offset, byte) in chunk.payload.iter().enumerate() {
                parity[offset] ^= gf256_mul(*byte, coefficient);
            }
        }
        records.push(MuninnMediaVideoParityShardRecord {
            stream_id: first.stream_id.clone(),
            session_id: first.session_id.clone(),
            frame_id: first.frame_id,
            codec: first.codec.clone(),
            pts_ticks: first.pts_ticks,
            duration_ticks: first.duration_ticks,
            timebase_num: first.timebase_num,
            timebase_den: first.timebase_den,
            keyframe: first.keyframe,
            dependency_frame_id: first.dependency_frame_id,
            deadline_ticks: first.deadline_ticks,
            chunk_count: first.chunk_count,
            parity_index,
            parity_count,
            chunk_payload_bytes,
            last_chunk_payload_bytes,
            payload: parity,
        });
    }
    Ok(records)
}

fn video_wire_records_with_parity(
    records: &[MuninnMediaVideoAccessUnitRecord],
) -> Result<Vec<MuninnMediaWireRecord>> {
    let mut wire_records = Vec::new();
    let mut offset = 0_usize;
    while offset < records.len() {
        let first = &records[offset];
        let mut end = offset + 1;
        while end < records.len()
            && records[end].stream_id == first.stream_id
            && records[end].session_id == first.session_id
            && records[end].frame_id == first.frame_id
        {
            end += 1;
        }
        let frame_records = &records[offset..end];
        let parity = build_video_parity_shards(frame_records)?;
        let parity_split = parity.len().div_ceil(2);
        wire_records.extend(
            parity[..parity_split]
                .iter()
                .cloned()
                .map(MuninnMediaWireRecord::VideoParity),
        );
        wire_records.extend(
            frame_records
                .iter()
                .cloned()
                .map(MuninnMediaWireRecord::Video),
        );
        wire_records.extend(
            parity[parity_split..]
                .iter()
                .cloned()
                .map(MuninnMediaWireRecord::VideoParity),
        );
        offset = end;
    }
    Ok(wire_records)
}

pub fn packetize_video_annex_b_stream(
    options: VideoAnnexBStreamPacketizeOptions<'_>,
    input: &[u8],
) -> Result<Vec<MuninnMediaVideoAccessUnitRecord>> {
    if options.frame_duration_ticks == 0 {
        return Err(anyhow!("frame_duration_ticks must be greater than zero"));
    }
    if options.deadline_delay_ticks < 0 {
        return Err(anyhow!("deadline_delay_ticks must be non-negative"));
    }

    let access_units = video_annex_b_access_units(options.codec, input)?;
    let mut records = Vec::new();
    for (index, access_unit) in access_units.iter().enumerate() {
        let frame_offset = u64::try_from(index).context("video frame index overflow")?;
        let frame_id = options
            .first_frame_id
            .checked_add(frame_offset)
            .ok_or_else(|| anyhow!("video frame_id overflow"))?;
        let pts_offset = i64::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(i64::from(options.frame_duration_ticks)))
            .ok_or_else(|| anyhow!("video pts_ticks overflow"))?;
        let pts_ticks = options
            .first_pts_ticks
            .checked_add(pts_offset)
            .ok_or_else(|| anyhow!("video pts_ticks overflow"))?;
        let deadline_ticks = pts_ticks
            .checked_add(options.deadline_delay_ticks)
            .ok_or_else(|| anyhow!("video deadline_ticks overflow"))?;

        records.extend(packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: options.stream_id,
                session_id: options.session_id,
                codec: options.codec,
                frame_id,
                pts_ticks,
                duration_ticks: options.frame_duration_ticks,
                timebase_num: options.timebase_num,
                timebase_den: options.timebase_den,
                deadline_ticks,
                max_payload_bytes: options.max_payload_bytes,
            },
            access_unit,
        )?);
    }

    Ok(records)
}

pub fn packetize_audio_packet(
    options: AudioPacketizeOptions<'_>,
    payload: &[u8],
) -> Result<MuninnMediaAudioPacketRecord> {
    if options.stream_id.is_empty() {
        return Err(anyhow!("stream_id must be non-empty"));
    }
    if options.session_id.is_empty() {
        return Err(anyhow!("session_id must be non-empty"));
    }
    if options.codec.is_empty() {
        return Err(anyhow!("codec must be non-empty"));
    }
    if options.timebase_num == 0 || options.timebase_den == 0 {
        return Err(anyhow!("audio timebase must be non-zero"));
    }
    if options.duration_ticks == 0 {
        return Err(anyhow!("audio duration_ticks must be greater than zero"));
    }
    if options.deadline_ticks < options.pts_ticks {
        return Err(anyhow!("audio deadline_ticks must not precede pts_ticks"));
    }
    if payload.is_empty() {
        return Err(anyhow!("audio packet payload must be non-empty"));
    }

    Ok(MuninnMediaAudioPacketRecord {
        stream_id: options.stream_id.to_string(),
        session_id: options.session_id.to_string(),
        packet_id: options.packet_id,
        codec: options.codec.to_string(),
        pts_ticks: options.pts_ticks,
        duration_ticks: options.duration_ticks,
        timebase_num: options.timebase_num,
        timebase_den: options.timebase_den,
        deadline_ticks: options.deadline_ticks,
        payload: payload.to_vec(),
    })
}

pub const AUDIO_FEC_DATA_SHARDS: usize = 4;
pub const AUDIO_FEC_PARITY_SHARDS: usize = 2;

/// Builds the fixed 4+2 systematic audio code used on the realtime lane.
/// P0 = D0+D1+D2+D3; P1 = D0+2D1+4D2+8D3 in GF(256), polynomial 0x11d.
pub fn audio_fec_parity_records(
    data: &[MuninnMediaAudioPacketRecord],
) -> Result<[MuninnMediaAudioParityShardRecord; AUDIO_FEC_PARITY_SHARDS]> {
    if data.len() != AUDIO_FEC_DATA_SHARDS {
        return Err(anyhow!("audio FEC requires exactly 4 data shards"));
    }
    for record in data {
        validate_audio_record(record)?;
    }
    let first = &data[0];
    let shard_len = first.payload.len();
    for (index, record) in data.iter().enumerate() {
        let expected_packet_id = first
            .packet_id
            .checked_add(index as u64)
            .ok_or_else(|| anyhow!("audio FEC packet_id overflow"))?;
        let expected_pts = first
            .pts_ticks
            .checked_add(i64::from(first.duration_ticks) * index as i64)
            .ok_or_else(|| anyhow!("audio FEC pts overflow"))?;
        if record.stream_id != first.stream_id
            || record.session_id != first.session_id
            || record.codec != first.codec
            || record.timebase_num != first.timebase_num
            || record.timebase_den != first.timebase_den
            || record.duration_ticks != first.duration_ticks
            || record.packet_id != expected_packet_id
            || record.pts_ticks != expected_pts
            || record.payload.len() != shard_len
        {
            return Err(anyhow!(
                "audio FEC data shards have mixed or non-contiguous metadata"
            ));
        }
    }
    let mut parity = [vec![0u8; shard_len], vec![0u8; shard_len]];
    for (data_index, record) in data.iter().enumerate() {
        let coefficient = [1u8, 2, 4, 8][data_index];
        for (byte_index, &byte) in record.payload.iter().enumerate() {
            parity[0][byte_index] ^= byte;
            parity[1][byte_index] ^= gf256_mul(byte, coefficient);
        }
    }
    let make = |parity_index: usize| MuninnMediaAudioParityShardRecord {
        stream_id: first.stream_id.clone(),
        session_id: first.session_id.clone(),
        base_packet_id: first.packet_id,
        codec: first.codec.clone(),
        base_pts_ticks: first.pts_ticks,
        packet_duration_ticks: first.duration_ticks,
        timebase_num: first.timebase_num,
        timebase_den: first.timebase_den,
        deadline_ticks: data
            .iter()
            .map(|r| r.deadline_ticks)
            .max()
            .unwrap_or(first.deadline_ticks),
        data_shard_count: 4,
        parity_index: parity_index as u16,
        parity_shard_count: 2,
        shard_payload_bytes: shard_len as u32,
        payload: parity[parity_index].clone(),
    };
    Ok([make(0), make(1)])
}

fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80;
        a <<= 1;
        if carry != 0 {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    product
}

fn gf256_inv(value: u8) -> u8 {
    debug_assert_ne!(value, 0);
    let mut result = 1u8;
    for _ in 0..254 {
        result = gf256_mul(result, value);
    }
    result
}

/// Pure receiver-reference implementation. Shards 0..4 are data; 4..6 are parity.
pub fn recover_audio_fec_data(mut shards: [Option<Vec<u8>>; 6]) -> Result<[Vec<u8>; 4]> {
    let shard_len = shards
        .iter()
        .flatten()
        .next()
        .ok_or_else(|| anyhow!("audio FEC has no shards"))?
        .len();
    if shard_len == 0 || shards.iter().flatten().any(|s| s.len() != shard_len) {
        return Err(anyhow!(
            "audio FEC shards must have one non-zero constant size"
        ));
    }
    let missing: Vec<usize> = (0..4).filter(|&i| shards[i].is_none()).collect();
    if missing.len() > 2 {
        return Err(anyhow!(
            "audio FEC cannot recover more than two data shards"
        ));
    }
    let coefficients = [[1u8, 1, 1, 1], [1u8, 2, 4, 8]];
    let parity_rows: Vec<usize> = (0..2)
        .filter(|&p| shards[4 + p].is_some())
        .take(missing.len())
        .collect();
    if parity_rows.len() < missing.len() {
        return Err(anyhow!("audio FEC has insufficient parity"));
    }
    for byte_index in 0..shard_len {
        let mut matrix = vec![vec![0u8; missing.len() + 1]; missing.len()];
        for (row, &p) in parity_rows.iter().enumerate() {
            let mut rhs = shards[4 + p].as_ref().unwrap()[byte_index];
            for data_index in 0..4 {
                if let Some(data) = &shards[data_index] {
                    rhs ^= gf256_mul(coefficients[p][data_index], data[byte_index]);
                }
            }
            for (column, &data_index) in missing.iter().enumerate() {
                matrix[row][column] = coefficients[p][data_index];
            }
            matrix[row][missing.len()] = rhs;
        }
        for column in 0..missing.len() {
            let pivot = (column..missing.len())
                .find(|&r| matrix[r][column] != 0)
                .ok_or_else(|| anyhow!("audio FEC coefficient matrix is singular"))?;
            matrix.swap(column, pivot);
            let inv = gf256_inv(matrix[column][column]);
            for c in column..=missing.len() {
                matrix[column][c] = gf256_mul(matrix[column][c], inv);
            }
            for row in 0..missing.len() {
                if row == column {
                    continue;
                }
                let factor = matrix[row][column];
                for c in column..=missing.len() {
                    matrix[row][c] ^= gf256_mul(factor, matrix[column][c]);
                }
            }
        }
        for (row, &data_index) in missing.iter().enumerate() {
            shards[data_index].get_or_insert_with(|| vec![0; shard_len])[byte_index] =
                matrix[row][missing.len()];
        }
    }
    Ok(std::array::from_fn(|i| shards[i].take().unwrap()))
}

#[derive(Default)]
pub struct AudioPacketBuffer {
    stream_id: Option<String>,
    session_id: Option<String>,
    codec: Option<String>,
    timebase_num: Option<u32>,
    timebase_den: Option<u32>,
    next_packet_id: Option<u64>,
    emitted_any: bool,
    packets: BTreeMap<u64, MuninnMediaAudioPacketRecord>,
}

impl AudioPacketBuffer {
    pub fn insert(&mut self, packet: MuninnMediaAudioPacketRecord) -> Result<()> {
        self.require_matching_audio_stream(&packet)?;
        if packet.payload.is_empty() {
            return Err(anyhow!("audio packet buffer payload must be non-empty"));
        }
        if self.packets.contains_key(&packet.packet_id) {
            return Err(anyhow!(
                "audio packet buffer has duplicate packet_id {}",
                packet.packet_id
            ));
        }
        match self.next_packet_id {
            None => self.next_packet_id = Some(packet.packet_id),
            Some(next_packet_id) if packet.packet_id < next_packet_id && !self.emitted_any => {
                self.next_packet_id = Some(packet.packet_id);
            }
            Some(next_packet_id) if packet.packet_id < next_packet_id => {
                return Err(anyhow!(
                    "audio packet buffer received stale packet_id {} before next expected {}",
                    packet.packet_id,
                    next_packet_id
                ));
            }
            _ => {}
        }
        self.packets.insert(packet.packet_id, packet);
        Ok(())
    }

    pub fn pop_ready_packets(&mut self) -> Vec<MuninnMediaAudioPacketRecord> {
        let Some(mut next_packet_id) = self.next_packet_id else {
            return Vec::new();
        };
        let mut ready = Vec::new();
        while let Some(packet) = self.packets.remove(&next_packet_id) {
            ready.push(packet);
            next_packet_id = next_packet_id.saturating_add(1);
        }
        if !ready.is_empty() {
            self.emitted_any = true;
        }
        self.next_packet_id = Some(next_packet_id);
        ready
    }

    pub fn expire_late_packets(&mut self, now_ticks: i64) -> Vec<u64> {
        let expired_ids = self
            .packets
            .iter()
            .filter_map(|(packet_id, packet)| {
                if packet.deadline_ticks <= now_ticks {
                    Some(*packet_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for packet_id in &expired_ids {
            self.packets.remove(packet_id);
        }
        if let Some(next_packet_id) = self.next_packet_id {
            if expired_ids.contains(&next_packet_id) {
                self.next_packet_id = self.packets.keys().next().copied();
            }
        }
        expired_ids
    }

    pub fn pending_packet_count(&self) -> usize {
        self.packets.len()
    }

    fn require_matching_audio_stream(
        &mut self,
        packet: &MuninnMediaAudioPacketRecord,
    ) -> Result<()> {
        if packet.stream_id.is_empty() {
            return Err(anyhow!("audio packet buffer stream_id must be non-empty"));
        }
        if packet.session_id.is_empty() {
            return Err(anyhow!("audio packet buffer session_id must be non-empty"));
        }
        if packet.codec.is_empty() {
            return Err(anyhow!("audio packet buffer codec must be non-empty"));
        }
        if packet.timebase_num == 0 || packet.timebase_den == 0 {
            return Err(anyhow!("audio packet buffer timebase must be non-zero"));
        }

        match (
            self.stream_id.as_deref(),
            self.session_id.as_deref(),
            self.codec.as_deref(),
            self.timebase_num,
            self.timebase_den,
        ) {
            (None, None, None, None, None) => {
                self.stream_id = Some(packet.stream_id.clone());
                self.session_id = Some(packet.session_id.clone());
                self.codec = Some(packet.codec.clone());
                self.timebase_num = Some(packet.timebase_num);
                self.timebase_den = Some(packet.timebase_den);
                Ok(())
            }
            (
                Some(stream_id),
                Some(session_id),
                Some(codec),
                Some(timebase_num),
                Some(timebase_den),
            ) if stream_id == packet.stream_id
                && session_id == packet.session_id
                && codec == packet.codec
                && timebase_num == packet.timebase_num
                && timebase_den == packet.timebase_den =>
            {
                Ok(())
            }
            _ => Err(anyhow!(
                "audio packet buffer received mixed stream metadata"
            )),
        }
    }
}

pub fn reassemble_video_access_unit(
    chunks: &[MuninnMediaVideoAccessUnitRecord],
) -> Result<VideoAccessUnit> {
    let first = chunks
        .first()
        .ok_or_else(|| anyhow!("video access unit requires at least one chunk"))?;
    if first.chunk_count == 0 {
        return Err(anyhow!("video access unit chunk_count must be non-zero"));
    }

    let mut by_index = BTreeMap::new();
    for chunk in chunks {
        if chunk.stream_id != first.stream_id {
            return Err(anyhow!(
                "video access unit chunks have mixed stream_id values"
            ));
        }
        if chunk.session_id != first.session_id {
            return Err(anyhow!(
                "video access unit chunks have mixed session_id values"
            ));
        }
        if chunk.frame_id != first.frame_id {
            return Err(anyhow!(
                "video access unit chunks have mixed frame_id values"
            ));
        }
        if chunk.codec != first.codec {
            return Err(anyhow!("video access unit chunks have mixed codec values"));
        }
        if chunk.pts_ticks != first.pts_ticks
            || chunk.duration_ticks != first.duration_ticks
            || chunk.timebase_num != first.timebase_num
            || chunk.timebase_den != first.timebase_den
            || chunk.deadline_ticks != first.deadline_ticks
        {
            return Err(anyhow!("video access unit chunks have mixed timing values"));
        }
        if chunk.keyframe != first.keyframe
            || chunk.dependency_frame_id != first.dependency_frame_id
        {
            return Err(anyhow!(
                "video access unit chunks have mixed dependency values"
            ));
        }
        if chunk.chunk_count != first.chunk_count {
            return Err(anyhow!(
                "video access unit chunks have mixed chunk_count values"
            ));
        }
        if chunk.chunk_index >= first.chunk_count {
            return Err(anyhow!(
                "video access unit chunk_index {} is outside chunk_count {}",
                chunk.chunk_index,
                first.chunk_count
            ));
        }
        if chunk.payload.is_empty() {
            return Err(anyhow!("video access unit chunk payload must be non-empty"));
        }
        if by_index
            .insert(chunk.chunk_index, chunk.payload.as_slice())
            .is_some()
        {
            return Err(anyhow!(
                "video access unit has duplicate chunk_index {}",
                chunk.chunk_index
            ));
        }
    }

    if by_index.len() != first.chunk_count as usize {
        return Err(anyhow!(
            "video access unit is missing chunks: expected {}, received {}",
            first.chunk_count,
            by_index.len()
        ));
    }

    let mut bytes = Vec::new();
    for index in 0..first.chunk_count {
        let payload = by_index
            .get(&index)
            .ok_or_else(|| anyhow!("video access unit is missing chunk_index {index}"))?;
        bytes.extend_from_slice(payload);
    }

    Ok(VideoAccessUnit {
        bytes,
        keyframe: first.keyframe,
    })
}

pub fn build_receiver_feedback(
    options: ReceiverFeedbackOptions<'_>,
) -> Result<MuninnMediaReceiverFeedbackRecord> {
    if options.stream_id.is_empty() {
        return Err(anyhow!("stream_id must be non-empty"));
    }
    if options.session_id.is_empty() {
        return Err(anyhow!("session_id must be non-empty"));
    }
    if options.receiver_id.is_empty() {
        return Err(anyhow!("receiver_id must be non-empty"));
    }
    if options.observed_at.is_empty() {
        return Err(anyhow!("observed_at must be non-empty"));
    }
    if options.jitter_us < 0 {
        return Err(anyhow!("jitter_us must be non-negative"));
    }
    if options.decode_queue_us < 0 {
        return Err(anyhow!("decode_queue_us must be non-negative"));
    }

    let mut missing_frame_ids = options.missing_frame_ids;
    missing_frame_ids.sort_unstable();
    missing_frame_ids.dedup();

    let missing_video_chunk_keys =
        normalize_video_chunk_feedback_keys(options.missing_video_chunk_keys)?;

    let mut late_frame_ids = options.late_frame_ids;
    late_frame_ids.sort_unstable();
    late_frame_ids.dedup();

    Ok(MuninnMediaReceiverFeedbackRecord {
        stream_id: options.stream_id.to_string(),
        session_id: options.session_id.to_string(),
        receiver_id: options.receiver_id.to_string(),
        highest_decodable_frame_id: options.highest_decodable_frame_id,
        missing_frame_ids,
        late_frame_ids,
        requested_keyframe: options.requested_keyframe,
        jitter_us: options.jitter_us,
        decode_queue_us: options.decode_queue_us,
        observed_at: options.observed_at.to_string(),
        missing_video_chunk_keys,
    })
}

pub fn build_feedback_for_expired_video_frames(
    expired: &[ExpiredVideoFrame],
    options: ExpiredVideoFrameFeedbackOptions<'_>,
) -> Result<Option<MuninnMediaReceiverFeedbackRecord>> {
    let Some(first) = expired.first() else {
        return Ok(None);
    };

    let mut late_frame_ids = Vec::with_capacity(expired.len());
    let mut missing_video_chunk_keys = Vec::new();
    let mut decode_chain_invalidated = false;
    for frame in expired {
        if frame.key.stream_id != first.key.stream_id
            || frame.key.session_id != first.key.session_id
        {
            return Err(anyhow!(
                "expired video frame feedback requires one stream_id/session_id"
            ));
        }
        late_frame_ids.push(frame.key.frame_id);
        missing_video_chunk_keys.extend(frame.missing_video_chunk_keys.iter().cloned());
        decode_chain_invalidated |= frame.decode_chain_invalidated;
    }

    build_receiver_feedback(ReceiverFeedbackOptions {
        stream_id: &first.key.stream_id,
        session_id: &first.key.session_id,
        receiver_id: options.receiver_id,
        highest_decodable_frame_id: first.key.frame_id.checked_sub(1),
        missing_frame_ids: Vec::new(),
        missing_video_chunk_keys,
        late_frame_ids,
        requested_keyframe: decode_chain_invalidated,
        jitter_us: options.jitter_us,
        decode_queue_us: options.decode_queue_us,
        observed_at: options.observed_at,
    })
    .map(Some)
}

pub fn encode_media_wire_record(
    record: &MuninnMediaWireRecord,
    stored_at: &str,
    source_runtime_id: &str,
    source_role: &str,
) -> Result<Vec<u8>> {
    if stored_at.is_empty() {
        return Err(anyhow!("stored_at must be non-empty"));
    }
    if source_runtime_id.is_empty() {
        return Err(anyhow!("source_runtime_id must be non-empty"));
    }
    if source_role.is_empty() {
        return Err(anyhow!("source_role must be non-empty"));
    }

    let (schema_id, record_key, payload) = match record {
        MuninnMediaWireRecord::Video(record) => (
            MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA,
            video_record_key(record),
            encode_video_record_payload(record)?,
        ),
        MuninnMediaWireRecord::VideoParity(record) => (
            MUNINN_MEDIA_VIDEO_PARITY_SHARD_SCHEMA,
            video_parity_record_key(record),
            encode_video_parity_record_payload(record)?,
        ),
        MuninnMediaWireRecord::Audio(record) => (
            MUNINN_MEDIA_AUDIO_PACKET_SCHEMA,
            audio_record_key(record),
            encode_audio_record_payload(record)?,
        ),
        MuninnMediaWireRecord::AudioParity(record) => (
            MUNINN_MEDIA_AUDIO_PARITY_SHARD_SCHEMA,
            audio_parity_record_key(record),
            encode_audio_parity_record_payload(record)?,
        ),
        MuninnMediaWireRecord::Feedback(record) => (
            MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA,
            feedback_record_key(record),
            encode_record_payload(record)?,
        ),
    };

    let message = CultNetMessage::DocumentPutRaw {
        message_id: format!(
            "muninn-media:{}:{}",
            schema_id,
            record_key.replace(':', "-")
        ),
        document: CultNetRawDocumentRecord {
            schema_id: schema_id.to_string(),
            record_key,
            stored_at: stored_at.to_string(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload,
            source_runtime_id: Some(source_runtime_id.to_string()),
            source_agent_id: None,
            source_role: Some(source_role.to_string()),
            tags: Some(vec!["muninn.media".to_string()]),
        },
    };

    encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
        .map_err(Into::into)
}

pub fn encode_media_wire_records(
    records: &[MuninnMediaWireRecord],
    stored_at: &str,
    source_runtime_id: &str,
    source_role: &str,
) -> Result<Vec<Vec<u8>>> {
    records
        .iter()
        .map(|record| encode_media_wire_record(record, stored_at, source_runtime_id, source_role))
        .collect()
}

pub fn encode_video_annex_b_stream_wire_records(
    options: VideoAnnexBStreamWireOptions<'_>,
    input: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let records = packetize_video_annex_b_stream(options.packetize, input)?;
    let wire_records = video_wire_records_with_parity(&records)?;
    encode_media_wire_records(
        &wire_records,
        options.stored_at,
        options.source_runtime_id,
        options.source_role,
    )
}

pub fn video_annex_b_stream_send_payloads(
    options: VideoAnnexBStreamWireOptions<'_>,
    input: &[u8],
) -> Result<Vec<MuninnMediaSendPayload>> {
    encode_video_annex_b_stream_wire_records(options, input).map(wire_payloads_to_media_send)
}

pub fn encode_audio_packet_wire_record(
    options: AudioPacketWireOptions<'_>,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let record = packetize_audio_packet(options.packetize, payload)?;
    encode_media_wire_record(
        &MuninnMediaWireRecord::Audio(record),
        options.stored_at,
        options.source_runtime_id,
        options.source_role,
    )
}

pub fn audio_packet_send_payload(
    options: AudioPacketWireOptions<'_>,
    payload: &[u8],
) -> Result<MuninnMediaSendPayload> {
    encode_audio_packet_wire_record(options, payload).map(wire_payload_to_media_send)
}

fn wire_payloads_to_media_send(payloads: Vec<Vec<u8>>) -> Vec<MuninnMediaSendPayload> {
    payloads
        .into_iter()
        .map(wire_payload_to_media_send)
        .collect()
}

fn wire_payload_to_media_send(payload: Vec<u8>) -> MuninnMediaSendPayload {
    MuninnMediaSendPayload {
        channel_id: MUNINN_MEDIA_RUDP_CHANNEL,
        payload,
    }
}

pub fn decode_media_wire_record(payload: &[u8]) -> Result<MuninnMediaWireRecord> {
    let message = decode_cultnet_message_from_slice(payload, CultNetWireContract::CultNetSchemaV0)?;
    let CultNetMessage::DocumentPutRaw { document, .. } = message else {
        return Err(anyhow!("expected cultnet.document_put_raw.v0"));
    };
    if document.payload_encoding != CultNetRawPayloadEncoding::Messagepack {
        return Err(anyhow!("Muninn media document payload must be MessagePack"));
    }

    match document.schema_id.as_str() {
        MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA => {
            let record: MuninnMediaVideoAccessUnitRecord =
                decode_record_payload(&document.payload)?;
            validate_video_record(&record)?;
            let expected_key = video_record_key(&record);
            if document.record_key != expected_key {
                return Err(anyhow!(
                    "Muninn video media record key mismatch: expected {}, received {}",
                    expected_key,
                    document.record_key
                ));
            }
            Ok(MuninnMediaWireRecord::Video(record))
        }
        MUNINN_MEDIA_VIDEO_PARITY_SHARD_SCHEMA => {
            let record: MuninnMediaVideoParityShardRecord =
                decode_record_payload(&document.payload)?;
            validate_video_parity_record(&record)?;
            let expected_key = video_parity_record_key(&record);
            if document.record_key != expected_key {
                return Err(anyhow!(
                    "Muninn video parity media record key mismatch: expected {}, received {}",
                    expected_key,
                    document.record_key
                ));
            }
            Ok(MuninnMediaWireRecord::VideoParity(record))
        }
        MUNINN_MEDIA_AUDIO_PACKET_SCHEMA => {
            let record: MuninnMediaAudioPacketRecord = decode_record_payload(&document.payload)?;
            validate_audio_record(&record)?;
            let expected_key = audio_record_key(&record);
            if document.record_key != expected_key {
                return Err(anyhow!(
                    "Muninn audio media record key mismatch: expected {}, received {}",
                    expected_key,
                    document.record_key
                ));
            }
            Ok(MuninnMediaWireRecord::Audio(record))
        }
        MUNINN_MEDIA_AUDIO_PARITY_SHARD_SCHEMA => {
            let record: MuninnMediaAudioParityShardRecord =
                decode_record_payload(&document.payload)?;
            validate_audio_parity_record(&record)?;
            let expected_key = audio_parity_record_key(&record);
            if document.record_key != expected_key {
                return Err(anyhow!(
                    "Muninn audio parity record key mismatch: expected {}, received {}",
                    expected_key,
                    document.record_key
                ));
            }
            Ok(MuninnMediaWireRecord::AudioParity(record))
        }
        MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA => {
            let record: MuninnMediaReceiverFeedbackRecord =
                decode_record_payload(&document.payload)?;
            validate_feedback_record(&record)?;
            let expected_key = feedback_record_key(&record);
            if document.record_key != expected_key {
                return Err(anyhow!(
                    "Muninn receiver feedback record key mismatch: expected {}, received {}",
                    expected_key,
                    document.record_key
                ));
            }
            Ok(MuninnMediaWireRecord::Feedback(record))
        }
        schema_id => Err(anyhow!("unsupported Muninn media schema {schema_id}")),
    }
}

fn encode_record_payload<T: Serialize>(record: &T) -> Result<Vec<u8>> {
    rmp_serde::to_vec(record).map_err(Into::into)
}

fn encode_video_record_payload(record: &MuninnMediaVideoAccessUnitRecord) -> Result<Vec<u8>> {
    encode_record_payload(&VideoAccessUnitWirePayload(
        &record.stream_id,
        &record.session_id,
        record.frame_id,
        &record.codec,
        record.pts_ticks,
        record.duration_ticks,
        record.timebase_num,
        record.timebase_den,
        record.keyframe,
        record.dependency_frame_id,
        record.deadline_ticks,
        record.chunk_index,
        record.chunk_count,
        &record.payload,
    ))
}

fn encode_video_parity_record_payload(
    record: &MuninnMediaVideoParityShardRecord,
) -> Result<Vec<u8>> {
    encode_record_payload(&VideoParityShardWirePayload(
        &record.stream_id,
        &record.session_id,
        record.frame_id,
        &record.codec,
        record.pts_ticks,
        record.duration_ticks,
        record.timebase_num,
        record.timebase_den,
        record.keyframe,
        record.dependency_frame_id,
        record.deadline_ticks,
        record.chunk_count,
        record.parity_index,
        record.parity_count,
        record.chunk_payload_bytes,
        record.last_chunk_payload_bytes,
        &record.payload,
    ))
}

fn encode_audio_record_payload(record: &MuninnMediaAudioPacketRecord) -> Result<Vec<u8>> {
    encode_record_payload(&AudioPacketWirePayload(
        &record.stream_id,
        &record.session_id,
        record.packet_id,
        &record.codec,
        record.pts_ticks,
        record.duration_ticks,
        record.timebase_num,
        record.timebase_den,
        record.deadline_ticks,
        &record.payload,
    ))
}

pub fn audio_fec_parity_send_payloads(
    data: &[MuninnMediaAudioPacketRecord],
    stored_at: &str,
    source_runtime_id: &str,
    source_role: &str,
) -> Result<Vec<MuninnMediaSendPayload>> {
    let parity = audio_fec_parity_records(data)?;
    parity
        .into_iter()
        .map(|record| {
            encode_media_wire_record(
                &MuninnMediaWireRecord::AudioParity(record),
                stored_at,
                source_runtime_id,
                source_role,
            )
            .map(wire_payload_to_media_send)
        })
        .collect()
}

fn encode_audio_parity_record_payload(
    record: &MuninnMediaAudioParityShardRecord,
) -> Result<Vec<u8>> {
    validate_audio_parity_record(record)?;
    encode_record_payload(&AudioParityShardWirePayload(
        &record.stream_id,
        &record.session_id,
        record.base_packet_id,
        &record.codec,
        record.base_pts_ticks,
        record.packet_duration_ticks,
        record.timebase_num,
        record.timebase_den,
        record.deadline_ticks,
        record.data_shard_count,
        record.parity_index,
        record.parity_shard_count,
        record.shard_payload_bytes,
        &record.payload,
    ))
}

fn decode_record_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T> {
    rmp_serde::from_slice(payload).map_err(Into::into)
}

fn validate_video_record(record: &MuninnMediaVideoAccessUnitRecord) -> Result<()> {
    if record.stream_id.is_empty() {
        return Err(anyhow!("video media record stream_id must be non-empty"));
    }
    if record.session_id.is_empty() {
        return Err(anyhow!("video media record session_id must be non-empty"));
    }
    if record.codec.is_empty() {
        return Err(anyhow!("video media record codec must be non-empty"));
    }
    if record.timebase_num == 0 || record.timebase_den == 0 {
        return Err(anyhow!("video media record timebase must be non-zero"));
    }
    if record.duration_ticks == 0 {
        return Err(anyhow!(
            "video media record duration_ticks must be greater than zero"
        ));
    }
    if record.deadline_ticks < record.pts_ticks {
        return Err(anyhow!(
            "video media record deadline_ticks must not precede pts_ticks"
        ));
    }
    if record.chunk_count == 0 {
        return Err(anyhow!("video media record chunk_count must be non-zero"));
    }
    if record.chunk_index >= record.chunk_count {
        return Err(anyhow!(
            "video media record chunk_index {} is outside chunk_count {}",
            record.chunk_index,
            record.chunk_count
        ));
    }
    if record.payload.is_empty() {
        return Err(anyhow!("video media record payload must be non-empty"));
    }
    Ok(())
}

fn validate_video_parity_record(record: &MuninnMediaVideoParityShardRecord) -> Result<()> {
    if record.stream_id.is_empty() {
        return Err(anyhow!(
            "video parity media record stream_id must be non-empty"
        ));
    }
    if record.session_id.is_empty() {
        return Err(anyhow!(
            "video parity media record session_id must be non-empty"
        ));
    }
    if record.codec.is_empty() {
        return Err(anyhow!("video parity media record codec must be non-empty"));
    }
    if record.timebase_num == 0 || record.timebase_den == 0 {
        return Err(anyhow!(
            "video parity media record timebase must be non-zero"
        ));
    }
    if record.duration_ticks == 0 {
        return Err(anyhow!(
            "video parity media record duration_ticks must be greater than zero"
        ));
    }
    if record.deadline_ticks < record.pts_ticks {
        return Err(anyhow!(
            "video parity media record deadline_ticks must not precede pts_ticks"
        ));
    }
    if record.chunk_count == 0 {
        return Err(anyhow!(
            "video parity media record chunk_count must be non-zero"
        ));
    }
    if record.parity_count == 0 || record.parity_index >= record.parity_count {
        return Err(anyhow!(
            "video parity media record has invalid parity stripe metadata"
        ));
    }
    if record.parity_count > record.chunk_count {
        return Err(anyhow!(
            "video parity media record parity_count exceeds chunk_count"
        ));
    }
    if record.chunk_payload_bytes == 0 || record.last_chunk_payload_bytes == 0 {
        return Err(anyhow!(
            "video parity media record chunk lengths must be non-zero"
        ));
    }
    if record.last_chunk_payload_bytes > record.chunk_payload_bytes {
        return Err(anyhow!(
            "video parity media record last chunk length exceeds regular chunk length"
        ));
    }
    if record.payload.is_empty() {
        return Err(anyhow!(
            "video parity media record payload must be non-empty"
        ));
    }
    if record.payload.len() > record.chunk_payload_bytes as usize {
        return Err(anyhow!(
            "video parity media record payload exceeds declared chunk length"
        ));
    }
    Ok(())
}

fn validate_audio_record(record: &MuninnMediaAudioPacketRecord) -> Result<()> {
    if record.stream_id.is_empty() {
        return Err(anyhow!("audio media record stream_id must be non-empty"));
    }
    if record.session_id.is_empty() {
        return Err(anyhow!("audio media record session_id must be non-empty"));
    }
    if record.codec.is_empty() {
        return Err(anyhow!("audio media record codec must be non-empty"));
    }
    if record.timebase_num == 0 || record.timebase_den == 0 {
        return Err(anyhow!("audio media record timebase must be non-zero"));
    }
    if record.duration_ticks == 0 {
        return Err(anyhow!(
            "audio media record duration_ticks must be greater than zero"
        ));
    }
    if record.deadline_ticks < record.pts_ticks {
        return Err(anyhow!(
            "audio media record deadline_ticks must not precede pts_ticks"
        ));
    }
    if record.payload.is_empty() {
        return Err(anyhow!("audio media record payload must be non-empty"));
    }
    Ok(())
}

fn validate_audio_parity_record(record: &MuninnMediaAudioParityShardRecord) -> Result<()> {
    if record.stream_id.is_empty() || record.session_id.is_empty() || record.codec.is_empty() {
        return Err(anyhow!("audio parity identity metadata must be non-empty"));
    }
    if record.timebase_num == 0 || record.timebase_den == 0 || record.packet_duration_ticks == 0 {
        return Err(anyhow!("audio parity timing metadata must be non-zero"));
    }
    if record.deadline_ticks < record.base_pts_ticks
        || record.data_shard_count != 4
        || record.parity_shard_count != 2
        || record.parity_index >= 2
    {
        return Err(anyhow!("audio parity stripe metadata is invalid"));
    }
    if record.shard_payload_bytes == 0
        || record.payload.len() != record.shard_payload_bytes as usize
    {
        return Err(anyhow!(
            "audio parity payload does not match declared shard size"
        ));
    }
    Ok(())
}

fn validate_feedback_record(record: &MuninnMediaReceiverFeedbackRecord) -> Result<()> {
    if record.stream_id.is_empty() {
        return Err(anyhow!(
            "receiver feedback media record stream_id must be non-empty"
        ));
    }
    if record.session_id.is_empty() {
        return Err(anyhow!(
            "receiver feedback media record session_id must be non-empty"
        ));
    }
    if record.receiver_id.is_empty() {
        return Err(anyhow!(
            "receiver feedback media record receiver_id must be non-empty"
        ));
    }
    if record.observed_at.is_empty() {
        return Err(anyhow!(
            "receiver feedback media record observed_at must be non-empty"
        ));
    }
    if record.jitter_us < 0 {
        return Err(anyhow!(
            "receiver feedback media record jitter_us must be non-negative"
        ));
    }
    if record.decode_queue_us < 0 {
        return Err(anyhow!(
            "receiver feedback media record decode_queue_us must be non-negative"
        ));
    }
    normalize_video_chunk_feedback_keys(record.missing_video_chunk_keys.clone())?;
    Ok(())
}

fn video_record_key(record: &MuninnMediaVideoAccessUnitRecord) -> String {
    format!(
        "{}:{}:video:{}:{}",
        record.stream_id, record.session_id, record.frame_id, record.chunk_index
    )
}

fn video_parity_record_key(record: &MuninnMediaVideoParityShardRecord) -> String {
    format!(
        "{}:{}:video-parity:{}:{}",
        record.stream_id, record.session_id, record.frame_id, record.parity_index
    )
}

fn audio_record_key(record: &MuninnMediaAudioPacketRecord) -> String {
    format!(
        "{}:{}:audio:{}",
        record.stream_id, record.session_id, record.packet_id
    )
}

fn audio_parity_record_key(record: &MuninnMediaAudioParityShardRecord) -> String {
    format!(
        "{}:{}:audio-fec:{}:{}",
        record.stream_id, record.session_id, record.base_packet_id, record.parity_index
    )
}

fn feedback_record_key(record: &MuninnMediaReceiverFeedbackRecord) -> String {
    format!(
        "{}:{}:feedback:{}",
        record.stream_id, record.session_id, record.receiver_id
    )
}

fn normalize_video_chunk_feedback_keys(keys: Vec<String>) -> Result<Vec<String>> {
    let mut parsed = keys
        .iter()
        .map(|key| VideoChunkKey::parse(key))
        .collect::<Result<Vec<_>>>()?;
    parsed.sort_unstable();
    parsed.dedup();
    Ok(parsed
        .into_iter()
        .map(|key| key.as_feedback_key())
        .collect())
}

fn normalized_video_codec(codec: &str) -> Option<&'static str> {
    match codec.trim().to_ascii_lowercase().as_str() {
        "h264" | "h.264" | "avc" | "avc1" | "video/avc" => Some("h264"),
        "h265" | "h.265" | "hevc" | "hev1" | "hvc1" | "video/hevc" => Some("h265"),
        "av1" | "av01" | "video/av1" => Some("av1"),
        _ => None,
    }
}

struct CompleteAdtsFrames {
    frames: Vec<Vec<u8>>,
    consumed_bytes: usize,
}

fn complete_adts_frames(input: &[u8]) -> Result<CompleteAdtsFrames> {
    let mut frames = Vec::new();
    let mut offset = 0_usize;

    while offset < input.len() {
        let remaining = &input[offset..];
        if remaining.len() < 2 {
            break;
        }
        if remaining[0] != 0xff || (remaining[1] & 0xf0) != 0xf0 {
            return Err(anyhow!("ADTS frame must start with sync word"));
        }
        if remaining.len() < 7 {
            break;
        }

        let protection_absent = (remaining[1] & 0x01) != 0;
        let header_len = if protection_absent { 7 } else { 9 };
        let frame_len = (((remaining[3] & 0x03) as usize) << 11)
            | ((remaining[4] as usize) << 3)
            | ((remaining[5] as usize) >> 5);
        if frame_len < header_len {
            return Err(anyhow!(
                "ADTS frame length {frame_len} is shorter than header length {header_len}"
            ));
        }
        if remaining.len() < frame_len {
            break;
        }

        frames.push(remaining[..frame_len].to_vec());
        offset = offset
            .checked_add(frame_len)
            .ok_or_else(|| anyhow!("ADTS consumed byte count overflow"))?;
    }

    Ok(CompleteAdtsFrames {
        frames,
        consumed_bytes: offset,
    })
}

fn annex_b_nal_units<'a>(
    input: &'a [u8],
    codec_name: &str,
    nal_type: fn(&[u8]) -> Option<u8>,
) -> Result<Vec<NalUnit<'a>>> {
    let mut starts = Vec::new();
    let mut offset = 0_usize;
    while let Some((start, prefix_len)) = find_annex_b_start_code(input, offset) {
        starts.push((start, prefix_len));
        offset = start + prefix_len;
    }

    if starts.is_empty() {
        return Err(anyhow!("{codec_name} Annex B stream has no start codes"));
    }

    let mut nal_units = Vec::new();
    for index in 0..starts.len() {
        let (start, prefix_len) = starts[index];
        let payload_start = start + prefix_len;
        let end = starts
            .get(index + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or(input.len());
        if payload_start >= end {
            continue;
        }
        let payload = &input[payload_start..end];
        if let Some(nal_type) = nal_type(payload) {
            nal_units.push(NalUnit {
                start,
                end,
                payload,
                nal_type,
            });
        }
    }

    if nal_units.is_empty() {
        return Err(anyhow!("{codec_name} Annex B stream has no NAL payloads"));
    }

    Ok(nal_units)
}

fn find_annex_b_start_code(input: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= input.len() {
        if input[index..].starts_with(&[0, 0, 1]) {
            return Some((index, 3));
        }
        if index + 4 <= input.len() && input[index..].starts_with(&[0, 0, 0, 1]) {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

fn is_h264_vcl_nal(nal_type: u8) -> bool {
    matches!(nal_type, 1..=5)
}

fn access_unit_references_previous_frame(codec: &str, access_unit: &VideoAccessUnit) -> bool {
    if access_unit.keyframe {
        return false;
    }
    match normalized_video_codec(codec).as_deref() {
        Some("h264") => h264_access_unit_has_reference_vcl(&access_unit.bytes).unwrap_or(true),
        _ => true,
    }
}

fn h264_access_unit_has_reference_vcl(input: &[u8]) -> Result<bool> {
    let nal_units = annex_b_nal_units(input, "H.264", h264_nal_type)?;
    Ok(nal_units
        .iter()
        .any(|nal| is_h264_vcl_nal(nal.nal_type) && h264_nal_ref_idc(nal.payload) > 0))
}

fn h264_nal_type(payload: &[u8]) -> Option<u8> {
    payload.first().map(|byte| byte & 0x1f)
}

fn h264_nal_ref_idc(payload: &[u8]) -> u8 {
    payload
        .first()
        .map(|byte| (byte >> 5) & 0x03)
        .unwrap_or_default()
}

fn h264_first_mb_in_slice(nal_payload: &[u8]) -> Option<u64> {
    if nal_payload.len() < 2 {
        return None;
    }
    let rbsp = h264_ebsp_to_rbsp(&nal_payload[1..]);
    read_unsigned_exp_golomb(&rbsp)
}

fn is_h265_vcl_nal(nal_type: u8) -> bool {
    nal_type <= 31
}

fn is_h265_irap_nal(nal_type: u8) -> bool {
    matches!(nal_type, 16..=21)
}

fn is_h265_pre_vcl_boundary_nal(nal_type: u8) -> bool {
    matches!(nal_type, 32..=34 | 39 | 40)
}

fn h265_nal_type(payload: &[u8]) -> Option<u8> {
    if payload.len() < 2 {
        return None;
    }
    Some((payload[0] >> 1) & 0x3f)
}

fn h265_first_slice_segment_in_pic(nal_payload: &[u8]) -> Option<bool> {
    if nal_payload.len() < 3 {
        return None;
    }
    let rbsp = h264_ebsp_to_rbsp(&nal_payload[2..]);
    rbsp.first().map(|byte| (byte & 0x80) != 0)
}

fn h264_ebsp_to_rbsp(payload: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut zero_count = 0_u8;
    for &byte in payload {
        if zero_count >= 2 && byte == 0x03 {
            zero_count = 0;
            continue;
        }
        rbsp.push(byte);
        zero_count = if byte == 0 { zero_count + 1 } else { 0 };
    }
    rbsp
}

fn read_unsigned_exp_golomb(payload: &[u8]) -> Option<u64> {
    let mut reader = BitReader::new(payload);
    let mut leading_zero_bits = 0_u32;
    while reader.read_bit()? == 0 {
        leading_zero_bits += 1;
        if leading_zero_bits > 63 {
            return None;
        }
    }

    let mut value = 1_u64;
    for _ in 0..leading_zero_bits {
        value = (value << 1) | u64::from(reader.read_bit()?);
    }
    Some(value - 1)
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_index: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_index: 0,
        }
    }

    fn read_bit(&mut self) -> Option<u8> {
        if self.bit_index >= self.bytes.len() * 8 {
            return None;
        }
        let byte = self.bytes[self.bit_index / 8];
        let shift = 7 - (self.bit_index % 8);
        self.bit_index += 1;
        Some((byte >> shift) & 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_code() -> [u8; 4] {
        [0, 0, 0, 1]
    }

    #[test]
    fn splits_h264_annex_b_access_units_on_aud_boundaries() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x09, 0xf0]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x67, 0x42, 0x00, 0x1f]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x09, 0xf0]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x41, 0x80]);

        let access_units = h264_annex_b_access_units(&stream)?;

        assert_eq!(access_units.len(), 2);
        assert!(access_units[0].keyframe);
        assert!(!access_units[1].keyframe);
        assert!(access_units[0].bytes.starts_with(&start_code()));
        assert!(access_units[1].bytes.starts_with(&start_code()));
        Ok(())
    }

    #[test]
    fn splits_h264_annex_b_access_units_on_new_slice_zero() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x41, 0x80]);

        let access_units = h264_annex_b_access_units(&stream)?;

        assert_eq!(access_units.len(), 2);
        assert!(access_units[0].keyframe);
        assert!(!access_units[1].keyframe);
        Ok(())
    }

    #[test]
    fn video_annex_b_dispatches_h264_aliases() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);

        let access_units = video_annex_b_access_units("AVC", &stream)?;

        assert_eq!(access_units.len(), 1);
        assert!(access_units[0].keyframe);
        Ok(())
    }

    fn h265_nal(nal_type: u8, slice_first: Option<bool>) -> Vec<u8> {
        let mut nal = vec![nal_type << 1, 0x01];
        if let Some(first) = slice_first {
            nal.push(if first { 0x80 } else { 0x00 });
        } else {
            nal.push(0x00);
        }
        nal
    }

    #[test]
    fn splits_h265_annex_b_access_units_on_aud_boundaries() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(35, None));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(32, None));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(33, None));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(34, None));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(19, Some(true)));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(35, None));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(1, Some(true)));

        let access_units = h265_annex_b_access_units(&stream)?;

        assert_eq!(access_units.len(), 2);
        assert!(access_units[0].keyframe);
        assert!(!access_units[1].keyframe);
        assert!(access_units[0].bytes.starts_with(&start_code()));
        assert!(access_units[1].bytes.starts_with(&start_code()));
        Ok(())
    }

    #[test]
    fn splits_h265_annex_b_access_units_on_first_slice_flag() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(19, Some(true)));
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(1, Some(true)));

        let access_units = h265_annex_b_access_units(&stream)?;

        assert_eq!(access_units.len(), 2);
        assert!(access_units[0].keyframe);
        assert!(!access_units[1].keyframe);
        Ok(())
    }

    #[test]
    fn video_annex_b_dispatches_hevc_aliases() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&h265_nal(19, Some(true)));

        let access_units = video_annex_b_access_units("HEVC", &stream)?;

        assert_eq!(access_units.len(), 1);
        assert!(access_units[0].keyframe);
        Ok(())
    }

    #[test]
    fn video_annex_b_rejects_av1_without_obu_packetizer() {
        let error = video_annex_b_access_units("av1", &[1, 2, 3]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("AV1 access unit splitting is not Annex B")
        );
    }

    #[test]
    fn video_annex_b_rejects_unknown_codec() {
        let error = video_annex_b_access_units("vp9", &[1, 2, 3]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported Annex B video codec vp9")
        );
    }

    #[test]
    fn packetizes_access_unit_into_typed_video_chunks() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: false,
        };

        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].chunk_index, 0);
        assert_eq!(records[0].chunk_count, 3);
        assert_eq!(records[0].dependency_frame_id, Some(8));
        assert_eq!(records[2].payload, vec![5]);
        Ok(())
    }

    #[test]
    fn builds_video_parity_shards_for_burst_loss_recovery() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: (1_u8..=40).collect(),
            keyframe: false,
        };
        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;

        let parity = build_video_parity_shards(&records)?;

        assert_eq!(records.len(), 20);
        assert_eq!(parity.len(), 16);
        assert!(
            parity.iter().enumerate().all(
                |(index, shard)| shard.parity_index == index as u16 && shard.parity_count == 16
            )
        );
        assert!(parity.iter().all(|shard| shard.chunk_payload_bytes == 2));

        let missing_index = 18_u16;
        let shard = &parity[7];
        let mut recovered = shard.payload.clone();
        for chunk in records.iter().filter(|chunk| chunk.chunk_index != missing_index) {
            let coefficient = video_fec_coefficient(
                shard.parity_index,
                shard.parity_count,
                chunk.chunk_index,
            );
            for (offset, byte) in chunk.payload.iter().enumerate() {
                recovered[offset] ^= gf256_mul(*byte, coefficient);
            }
        }
        let inverse = gf256_inv(video_fec_coefficient(
            shard.parity_index,
            shard.parity_count,
            missing_index,
        ));
        for byte in &mut recovered {
            *byte = gf256_mul(*byte, inverse);
        }
        recovered.truncate(records[missing_index as usize].payload.len());
        assert_eq!(recovered, records[missing_index as usize].payload);
        Ok(())
    }

    #[test]
    fn video_wire_bookends_data_with_independent_parity() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: (1_u8..=40).collect(),
            keyframe: true,
        };
        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 45_000,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;

        let wire = video_wire_records_with_parity(&records)?;
        assert_eq!(wire.len(), 36);
        assert!(wire[..8].iter().all(|record| matches!(record, MuninnMediaWireRecord::VideoParity(_))));
        assert!(wire[8..28].iter().all(|record| matches!(record, MuninnMediaWireRecord::Video(_))));
        assert!(wire[28..].iter().all(|record| matches!(record, MuninnMediaWireRecord::VideoParity(_))));
        Ok(())
    }

    #[test]
    fn packetizes_annex_b_stream_into_timed_video_records() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x41, 0x80]);

        let records = packetize_video_annex_b_stream(
            VideoAnnexBStreamPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "avc",
                first_frame_id: 9,
                first_pts_ticks: 27_000,
                frame_duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_delay_ticks: 1_800,
                max_payload_bytes: 16,
            },
            &stream,
        )?;

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].frame_id, 9);
        assert_eq!(records[0].pts_ticks, 27_000);
        assert_eq!(records[0].deadline_ticks, 28_800);
        assert!(records[0].keyframe);
        assert_eq!(records[1].frame_id, 10);
        assert_eq!(records[1].pts_ticks, 30_000);
        assert_eq!(records[1].deadline_ticks, 31_800);
        assert_eq!(records[1].dependency_frame_id, Some(9));
        assert_eq!(records[1].codec, "avc");
        Ok(())
    }

    #[test]
    fn packetizes_non_reference_h264_p_frames_without_dependency() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x01, 0x80]);

        let records = packetize_video_annex_b_stream(
            VideoAnnexBStreamPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                first_frame_id: 9,
                first_pts_ticks: 27_000,
                frame_duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_delay_ticks: 1_800,
                max_payload_bytes: 16,
            },
            &stream,
        )?;

        assert_eq!(records.len(), 2);
        assert!(records[0].keyframe);
        assert_eq!(records[0].dependency_frame_id, None);
        assert!(!records[1].keyframe);
        assert_eq!(records[1].dependency_frame_id, None);
        Ok(())
    }

    #[test]
    fn rejects_negative_annex_b_stream_deadline_delay() {
        let error = packetize_video_annex_b_stream(
            VideoAnnexBStreamPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                first_frame_id: 9,
                first_pts_ticks: 27_000,
                frame_duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_delay_ticks: -1,
                max_payload_bytes: 4,
            },
            &[0, 0, 0, 1, 0x65, 0x80],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("deadline_delay_ticks must be non-negative")
        );
    }

    #[test]
    fn rejects_zero_duration_video_access_units() {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3],
            keyframe: true,
        };

        let error = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 0,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("video duration_ticks must be greater than zero")
        );
    }

    #[test]
    fn rejects_video_deadline_before_pts() {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3],
            keyframe: true,
        };

        let error = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 26_999,
                max_payload_bytes: 2,
            },
            &access_unit,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("video deadline_ticks must not precede pts_ticks")
        );
    }

    #[test]
    fn video_chunk_key_round_trips_feedback_key() -> Result<()> {
        let key = VideoChunkKey::parse("42:7")?;

        assert_eq!(key, VideoChunkKey::new(42, 7));
        assert_eq!(key.as_feedback_key(), "42:7");
        assert_eq!(video_chunk_feedback_key(42, 7), "42:7");
        Ok(())
    }

    #[test]
    fn rejects_malformed_video_chunk_keys() {
        let error = VideoChunkKey::parse("42:7:extra").unwrap_err();

        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn packetizes_audio_payload_into_typed_packet() -> Result<()> {
        let record = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )?;

        assert_eq!(record.stream_id, "muninn.raven.av.rudp");
        assert_eq!(record.codec, "opus");
        assert_eq!(record.packet_id, 12);
        assert_eq!(record.payload, vec![0xf8, 0xff, 0xfe]);
        Ok(())
    }

    #[test]
    fn rejects_empty_audio_payloads() {
        let error = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[],
        )
        .unwrap_err();

        assert!(error.to_string().contains("payload must be non-empty"));
    }

    #[test]
    fn rejects_zero_duration_audio_packets() {
        let error = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 0,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("audio duration_ticks must be greater than zero")
        );
    }

    #[test]
    fn rejects_audio_deadline_before_pts() {
        let error = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 47_999,
            },
            &[0xf8, 0xff, 0xfe],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("audio deadline_ticks must not precede pts_ticks")
        );
    }

    fn audio_packet(packet_id: u64, deadline_ticks: i64) -> MuninnMediaAudioPacketRecord {
        packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id,
                pts_ticks: (packet_id as i64) * 960,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks,
            },
            &[0xf8, 0xff, packet_id as u8],
        )
        .unwrap()
    }

    #[test]
    fn audio_packet_buffer_emits_contiguous_packets_in_order() -> Result<()> {
        let mut buffer = AudioPacketBuffer::default();

        buffer.insert(audio_packet(7, 10_000))?;
        buffer.insert(audio_packet(9, 12_000))?;
        let ready = buffer.pop_ready_packets();

        assert_eq!(
            ready
                .iter()
                .map(|packet| packet.packet_id)
                .collect::<Vec<_>>(),
            vec![7]
        );
        assert_eq!(buffer.pending_packet_count(), 1);

        buffer.insert(audio_packet(8, 11_000))?;
        let ready = buffer.pop_ready_packets();

        assert_eq!(
            ready
                .iter()
                .map(|packet| packet.packet_id)
                .collect::<Vec<_>>(),
            vec![8, 9]
        );
        assert_eq!(buffer.pending_packet_count(), 0);
        Ok(())
    }

    #[test]
    fn audio_packet_buffer_tracks_lowest_packet_before_first_emit() -> Result<()> {
        let mut buffer = AudioPacketBuffer::default();

        buffer.insert(audio_packet(9, 12_000))?;
        buffer.insert(audio_packet(7, 10_000))?;
        buffer.insert(audio_packet(8, 11_000))?;

        let ready = buffer.pop_ready_packets();

        assert_eq!(
            ready
                .iter()
                .map(|packet| packet.packet_id)
                .collect::<Vec<_>>(),
            vec![7, 8, 9]
        );
        assert_eq!(buffer.pending_packet_count(), 0);
        Ok(())
    }

    #[test]
    fn audio_packet_buffer_rejects_stale_packets_after_emit() -> Result<()> {
        let mut buffer = AudioPacketBuffer::default();

        buffer.insert(audio_packet(7, 10_000))?;
        assert_eq!(buffer.pop_ready_packets().len(), 1);
        let error = buffer.insert(audio_packet(6, 9_000)).unwrap_err();

        assert!(error.to_string().contains("stale packet_id"));
        Ok(())
    }

    #[test]
    fn audio_packet_buffer_expires_late_packets() -> Result<()> {
        let mut buffer = AudioPacketBuffer::default();

        buffer.insert(audio_packet(7, 10_000))?;
        buffer.insert(audio_packet(8, 12_000))?;

        let expired = buffer.expire_late_packets(10_000);

        assert_eq!(expired, vec![7]);
        assert_eq!(buffer.pending_packet_count(), 1);
        assert_eq!(
            buffer
                .pop_ready_packets()
                .into_iter()
                .map(|packet| packet.packet_id)
                .collect::<Vec<_>>(),
            vec![8]
        );
        Ok(())
    }

    #[test]
    fn audio_packet_buffer_rejects_mixed_stream_metadata() -> Result<()> {
        let mut buffer = AudioPacketBuffer::default();
        let mut mixed = audio_packet(8, 12_000);
        mixed.session_id = "session-2".to_string();

        buffer.insert(audio_packet(7, 10_000))?;
        let error = buffer.insert(mixed).unwrap_err();

        assert!(error.to_string().contains("mixed stream metadata"));
        Ok(())
    }

    #[test]
    fn reassembles_video_chunks_in_chunk_index_order() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: true,
        };
        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        let shuffled = vec![records[2].clone(), records[0].clone(), records[1].clone()];

        let reassembled = reassemble_video_access_unit(&shuffled)?;

        assert_eq!(reassembled, access_unit);
        Ok(())
    }

    #[test]
    fn rejects_video_chunks_from_mixed_frames() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4],
            keyframe: false,
        };
        let mut records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        records[1].frame_id = 10;

        let error = reassemble_video_access_unit(&records).unwrap_err();

        assert!(error.to_string().contains("mixed frame_id"));
        Ok(())
    }

    #[test]
    fn rejects_incomplete_video_chunk_sets() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: false,
        };
        let mut records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        records.pop();

        let error = reassemble_video_access_unit(&records).unwrap_err();

        assert!(error.to_string().contains("missing chunks"));
        Ok(())
    }

    #[test]
    fn recoverable_receiver_damage_does_not_request_keyframe() -> Result<()> {
        let feedback = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: vec![43, 42, 43],
            missing_video_chunk_keys: vec![
                video_chunk_feedback_key(43, 2),
                video_chunk_feedback_key(42, 1),
                video_chunk_feedback_key(42, 1),
            ],
            late_frame_ids: vec![39, 39, 38],
            requested_keyframe: false,
            jitter_us: 700,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })?;

        assert_eq!(feedback.missing_frame_ids, vec![42, 43]);
        assert_eq!(feedback.missing_video_chunk_keys, vec!["42:1", "43:2"]);
        assert_eq!(feedback.late_frame_ids, vec![38, 39]);
        assert!(!feedback.requested_keyframe);
        Ok(())
    }

    #[test]
    fn explicit_decode_chain_invalidation_requests_keyframe() -> Result<()> {
        let feedback = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: vec![41],
            missing_video_chunk_keys: Vec::new(),
            late_frame_ids: vec![41],
            requested_keyframe: true,
            jitter_us: 700,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })?;

        assert!(feedback.requested_keyframe);
        Ok(())
    }

    #[test]
    fn rejects_negative_receiver_feedback_timing() {
        let error = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: Vec::new(),
            missing_video_chunk_keys: Vec::new(),
            late_frame_ids: Vec::new(),
            requested_keyframe: false,
            jitter_us: -1,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })
        .unwrap_err();

        assert!(error.to_string().contains("jitter_us"));
    }

    #[test]
    fn rejects_malformed_receiver_feedback_chunk_keys() {
        let error = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: Vec::new(),
            missing_video_chunk_keys: vec!["frame:chunk".to_string()],
            late_frame_ids: Vec::new(),
            requested_keyframe: false,
            jitter_us: 0,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })
        .unwrap_err();

        assert!(error.to_string().contains("frame_id"));
    }

    #[test]
    fn video_frame_assembly_reports_missing_chunks_and_reassembles() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: true,
        };
        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        let mut assembly = VideoFrameAssembly::new(records[0].clone())?;

        assert!(!assembly.is_complete());
        assert_eq!(
            assembly.missing_video_chunk_keys(),
            vec![
                video_chunk_feedback_key(9, 1),
                video_chunk_feedback_key(9, 2)
            ]
        );

        assembly.insert(records[2].clone())?;
        assert_eq!(
            assembly.missing_video_chunk_keys(),
            vec![video_chunk_feedback_key(9, 1)]
        );

        assembly.insert(records[1].clone())?;
        assert!(assembly.is_complete());
        assert_eq!(assembly.reassemble()?, access_unit);
        Ok(())
    }

    #[test]
    fn video_frame_assembly_rejects_mixed_metadata() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4],
            keyframe: false,
        };
        let mut records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        let mut assembly = VideoFrameAssembly::new(records[0].clone())?;
        records[1].pts_ticks += 1;

        let error = assembly.insert(records[1].clone()).unwrap_err();

        assert!(error.to_string().contains("metadata"));
        Ok(())
    }

    #[test]
    fn video_frame_assembly_set_handles_interleaved_frames() -> Result<()> {
        let frame_one = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: true,
        };
        let frame_two = VideoAccessUnit {
            bytes: vec![6, 7, 8, 9],
            keyframe: false,
        };
        let frame_one_chunks = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &frame_one,
        )?;
        let frame_two_chunks = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 10,
                pts_ticks: 30_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 31_800,
                max_payload_bytes: 2,
            },
            &frame_two,
        )?;
        let key_one = VideoFrameKey::from_chunk(&frame_one_chunks[0]);
        let mut assemblies = VideoFrameAssemblySet::default();

        assert!(
            assemblies
                .insert_chunk(frame_one_chunks[0].clone())?
                .is_none()
        );
        assert!(
            assemblies
                .insert_chunk(frame_two_chunks[1].clone())?
                .is_none()
        );
        assert_eq!(assemblies.pending_frame_count(), 2);
        assert_eq!(
            assemblies.missing_video_chunk_keys(&key_one),
            vec![
                video_chunk_feedback_key(9, 1),
                video_chunk_feedback_key(9, 2)
            ]
        );

        assert!(
            assemblies
                .insert_chunk(frame_one_chunks[2].clone())?
                .is_none()
        );
        let completed = assemblies
            .insert_chunk(frame_one_chunks[1].clone())?
            .expect("frame one should complete");

        assert_eq!(completed, frame_one);
        assert_eq!(assemblies.pending_frame_count(), 1);
        assert!(assemblies.missing_video_chunk_keys(&key_one).is_empty());
        Ok(())
    }

    #[test]
    fn video_frame_assembly_set_expires_late_incomplete_frames() -> Result<()> {
        let frame_one = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: true,
        };
        let frame_two = VideoAccessUnit {
            bytes: vec![6, 7, 8, 9],
            keyframe: false,
        };
        let frame_one_chunks = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &frame_one,
        )?;
        let frame_two_chunks = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 10,
                pts_ticks: 30_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 31_800,
                max_payload_bytes: 2,
            },
            &frame_two,
        )?;
        let key_one = VideoFrameKey::from_chunk(&frame_one_chunks[0]);
        let key_two = VideoFrameKey::from_chunk(&frame_two_chunks[0]);
        let mut assemblies = VideoFrameAssemblySet::default();

        assemblies.insert_chunk(frame_one_chunks[0].clone())?;
        assemblies.insert_chunk(frame_two_chunks[0].clone())?;

        let expired = assemblies.expire_late_frames(30_000);

        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].key, key_one);
        assert_eq!(
            expired[0].missing_video_chunk_keys,
            vec![
                video_chunk_feedback_key(9, 1),
                video_chunk_feedback_key(9, 2)
            ]
        );
        assert_eq!(assemblies.pending_frame_count(), 1);
        assert!(!assemblies.missing_video_chunk_keys(&key_two).is_empty());

        let feedback = build_feedback_for_expired_video_frames(
            &expired,
            ExpiredVideoFrameFeedbackOptions {
                receiver_id: "starfire.obs",
                jitter_us: 700,
                decode_queue_us: 2_000,
                observed_at: "2026-06-18T00:00:00Z",
            },
        )?
        .expect("expired frames should produce feedback");

        assert_eq!(feedback.late_frame_ids, vec![9]);
        assert_eq!(
            feedback.missing_video_chunk_keys,
            vec![
                video_chunk_feedback_key(9, 1),
                video_chunk_feedback_key(9, 2)
            ]
        );
        assert!(feedback.requested_keyframe);
        Ok(())
    }

    #[test]
    fn expired_video_feedback_rejects_mixed_streams() {
        let expired = vec![
            ExpiredVideoFrame {
                key: VideoFrameKey {
                    stream_id: "muninn.raven.av.rudp".to_string(),
                    session_id: "session-1".to_string(),
                    frame_id: 9,
                },
                deadline_ticks: 28_800,
                missing_video_chunk_keys: vec![video_chunk_feedback_key(9, 1)],
                decode_chain_invalidated: true,
            },
            ExpiredVideoFrame {
                key: VideoFrameKey {
                    stream_id: "muninn.nightwing.av.rudp".to_string(),
                    session_id: "session-1".to_string(),
                    frame_id: 10,
                },
                deadline_ticks: 31_800,
                missing_video_chunk_keys: vec![video_chunk_feedback_key(10, 1)],
                decode_chain_invalidated: true,
            },
        ];

        let error = build_feedback_for_expired_video_frames(
            &expired,
            ExpiredVideoFrameFeedbackOptions {
                receiver_id: "starfire.obs",
                jitter_us: 700,
                decode_queue_us: 2_000,
                observed_at: "2026-06-18T00:00:00Z",
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("one stream_id/session_id"));
    }

    #[test]
    fn expired_non_dependency_damage_does_not_request_keyframe() -> Result<()> {
        let expired = vec![ExpiredVideoFrame {
            key: VideoFrameKey {
                stream_id: "muninn.raven.av.rudp".to_string(),
                session_id: "session-1".to_string(),
                frame_id: 9,
            },
            deadline_ticks: 28_800,
            missing_video_chunk_keys: vec![video_chunk_feedback_key(9, 1)],
            decode_chain_invalidated: false,
        }];

        let feedback = build_feedback_for_expired_video_frames(
            &expired,
            ExpiredVideoFrameFeedbackOptions {
                receiver_id: "starfire.obs",
                jitter_us: 700,
                decode_queue_us: 2_000,
                observed_at: "2026-06-18T00:00:00Z",
            },
        )?
        .expect("expired damage should produce feedback");

        assert!(!feedback.requested_keyframe);
        Ok(())
    }

    #[test]
    fn media_wire_round_trips_video_document() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3],
            keyframe: true,
        };
        let record = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 4,
            },
            &access_unit,
        )?
        .remove(0);

        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Video(record.clone()),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        assert_eq!(
            decode_media_wire_record(&wire)?,
            MuninnMediaWireRecord::Video(record)
        );
        Ok(())
    }

    #[test]
    fn media_wire_round_trips_video_parity_document() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3, 4, 5],
            keyframe: false,
        };
        let records = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 2,
            },
            &access_unit,
        )?;
        let parity = build_video_parity_shards(&records)?
            .into_iter()
            .next()
            .expect("multi-chunk frame gets parity");

        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::VideoParity(parity.clone()),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        assert_eq!(
            decode_media_wire_record(&wire)?,
            MuninnMediaWireRecord::VideoParity(parity)
        );
        Ok(())
    }

    #[test]
    fn media_wire_batches_annex_b_video_records() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x41, 0x80]);
        let records = packetize_video_annex_b_stream(
            VideoAnnexBStreamPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                first_frame_id: 9,
                first_pts_ticks: 27_000,
                frame_duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_delay_ticks: 1_800,
                max_payload_bytes: 16,
            },
            &stream,
        )?;
        let wire_records = records
            .iter()
            .cloned()
            .map(MuninnMediaWireRecord::Video)
            .collect::<Vec<_>>();

        let wire = encode_media_wire_records(
            &wire_records,
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        assert_eq!(wire.len(), 2);
        assert_eq!(decode_media_wire_record(&wire[0])?, wire_records[0]);
        assert_eq!(decode_media_wire_record(&wire[1])?, wire_records[1]);
        Ok(())
    }

    #[test]
    fn media_wire_encodes_annex_b_stream_for_sender() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x41, 0x80]);

        let wire = encode_video_annex_b_stream_wire_records(
            VideoAnnexBStreamWireOptions {
                packetize: VideoAnnexBStreamPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "session-1",
                    codec: "h264",
                    first_frame_id: 9,
                    first_pts_ticks: 27_000,
                    frame_duration_ticks: 3_000,
                    timebase_num: 1,
                    timebase_den: 90_000,
                    deadline_delay_ticks: 1_800,
                    max_payload_bytes: 16,
                },
                stored_at: "2026-06-18T00:00:00Z",
                source_runtime_id: "muninn-test",
                source_role: "media-test",
            },
            &stream,
        )?;

        assert_eq!(wire.len(), 2);
        let first = decode_media_wire_record(&wire[0])?;
        let second = decode_media_wire_record(&wire[1])?;
        let MuninnMediaWireRecord::Video(first) = first else {
            panic!("expected video media record");
        };
        let MuninnMediaWireRecord::Video(second) = second else {
            panic!("expected video media record");
        };
        assert_eq!(first.frame_id, 9);
        assert_eq!(first.deadline_ticks, 28_800);
        assert_eq!(second.frame_id, 10);
        assert_eq!(second.deadline_ticks, 31_800);
        Ok(())
    }

    #[test]
    fn media_send_payloads_pin_video_to_media_channel() -> Result<()> {
        let mut stream = Vec::new();
        stream.extend_from_slice(&start_code());
        stream.extend_from_slice(&[0x65, 0x80]);

        let payloads = video_annex_b_stream_send_payloads(
            VideoAnnexBStreamWireOptions {
                packetize: VideoAnnexBStreamPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "session-1",
                    codec: "h264",
                    first_frame_id: 9,
                    first_pts_ticks: 27_000,
                    frame_duration_ticks: 3_000,
                    timebase_num: 1,
                    timebase_den: 90_000,
                    deadline_delay_ticks: 1_800,
                    max_payload_bytes: 16,
                },
                stored_at: "2026-06-18T00:00:00Z",
                source_runtime_id: "muninn-test",
                source_role: "media-test",
            },
            &stream,
        )?;

        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].channel_id, MUNINN_MEDIA_RUDP_CHANNEL);
        let MuninnMediaWireRecord::Video(record) = decode_media_wire_record(&payloads[0].payload)?
        else {
            panic!("expected video media record");
        };
        assert_eq!(record.frame_id, 9);
        Ok(())
    }

    fn stream_send_config() -> VideoAnnexBStreamSendConfig {
        VideoAnnexBStreamSendConfig {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "session-1".to_string(),
            codec: "h264".to_string(),
            first_frame_id: 9,
            first_pts_ticks: 27_000,
            frame_duration_ticks: 3_000,
            timebase_num: 1,
            timebase_den: 90_000,
            deadline_delay_ticks: 1_800,
            max_payload_bytes: 16,
            max_pending_bytes: 64,
            source_runtime_id: "muninn-test".to_string(),
            source_role: "media-test".to_string(),
        }
    }

    fn adts_stream_send_config() -> AudioAdtsStreamSendConfig {
        AudioAdtsStreamSendConfig {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "session-1".to_string(),
            codec: "aac-adts".to_string(),
            first_packet_id: 12,
            first_pts_ticks: 48_000,
            packet_duration_ticks: 1_024,
            timebase_num: 1,
            timebase_den: 48_000,
            deadline_delay_ticks: 1_024,
            max_pending_bytes: 128,
            source_runtime_id: "muninn-test".to_string(),
            source_role: "media-test".to_string(),
        }
    }

    fn adts_frame(payload: &[u8]) -> Vec<u8> {
        let frame_length = 7 + payload.len();
        let mut frame = vec![
            0xff,
            0xf1,
            0x4c,
            0x80 | (((frame_length >> 11) & 0x03) as u8),
            ((frame_length >> 3) & 0xff) as u8,
            (((frame_length & 0x07) << 5) as u8) | 0x1f,
            0xfc,
        ];
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn annex_b_stream_send_state_emits_only_completed_frames() -> Result<()> {
        let mut first_frame = Vec::new();
        first_frame.extend_from_slice(&start_code());
        first_frame.extend_from_slice(&[0x65, 0x80]);
        let mut second_frame = Vec::new();
        second_frame.extend_from_slice(&start_code());
        second_frame.extend_from_slice(&[0x41, 0x80]);
        let mut sender = VideoAnnexBStreamSendState::new(stream_send_config())?;

        let first_push = sender.push("2026-06-18T00:00:00Z", &first_frame)?;

        assert!(first_push.is_empty());
        assert_eq!(sender.next_frame_id(), 9);
        assert_eq!(sender.pending_bytes(), first_frame.len());

        let second_push = sender.push("2026-06-18T00:00:00Z", &second_frame)?;

        assert_eq!(second_push.len(), 1);
        assert_eq!(second_push[0].channel_id, MUNINN_MEDIA_RUDP_CHANNEL);
        let MuninnMediaWireRecord::Video(first) =
            decode_media_wire_record(&second_push[0].payload)?
        else {
            panic!("expected video media record");
        };
        assert_eq!(first.frame_id, 9);
        assert_eq!(first.pts_ticks, 27_000);
        assert!(first.keyframe);
        assert_eq!(sender.next_frame_id(), 10);
        assert_eq!(sender.pending_bytes(), second_frame.len());

        let tail = sender.finish("2026-06-18T00:00:00Z")?;

        assert_eq!(tail.len(), 1);
        let MuninnMediaWireRecord::Video(second) = decode_media_wire_record(&tail[0].payload)?
        else {
            panic!("expected video media record");
        };
        assert_eq!(second.frame_id, 10);
        assert_eq!(second.pts_ticks, 30_000);
        assert_eq!(second.dependency_frame_id, Some(9));
        assert_eq!(sender.pending_bytes(), 0);
        assert_eq!(sender.next_frame_id(), 11);
        Ok(())
    }

    #[test]
    fn annex_b_stream_send_state_rejects_non_annex_b_codecs() {
        let mut config = stream_send_config();
        config.codec = "av1".to_string();

        let error = match VideoAnnexBStreamSendState::new(config) {
            Ok(_) => panic!("AV1 must not create an Annex B stream sender"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("Annex B stream sender requires H.264/AVC or H.265/HEVC codec")
        );
    }

    #[test]
    fn annex_b_stream_send_state_rejects_unbounded_pending_bytes() -> Result<()> {
        let mut config = stream_send_config();
        config.max_pending_bytes = 4;
        let mut sender = VideoAnnexBStreamSendState::new(config)?;

        let error = sender
            .push("2026-06-18T00:00:00Z", &[0x47, 0x40, 0x00, 0x10, 0x00])
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("pending buffer exceeded 4 bytes")
        );
        Ok(())
    }

    #[test]
    fn adts_stream_send_state_emits_complete_packets() -> Result<()> {
        let first_frame = adts_frame(&[0x11, 0x22, 0x33]);
        let second_frame = adts_frame(&[0x44, 0x55]);
        let mut sender = AudioAdtsStreamSendState::new(adts_stream_send_config())?;

        let first_push = sender.push("2026-06-18T00:00:00Z", &first_frame[..4])?;

        assert!(first_push.is_empty());
        assert_eq!(sender.pending_bytes(), 4);
        assert_eq!(sender.next_packet_id(), 12);

        let mut tail = first_frame[4..].to_vec();
        tail.extend_from_slice(&second_frame);
        let second_push = sender.push("2026-06-18T00:00:00Z", &tail)?;

        assert_eq!(second_push.len(), 2);
        assert_eq!(second_push[0].channel_id, MUNINN_MEDIA_RUDP_CHANNEL);
        assert_eq!(second_push[1].channel_id, MUNINN_MEDIA_RUDP_CHANNEL);

        let MuninnMediaWireRecord::Audio(first) =
            decode_media_wire_record(&second_push[0].payload)?
        else {
            panic!("expected audio media record");
        };
        assert_eq!(first.packet_id, 12);
        assert_eq!(first.pts_ticks, 48_000);
        assert_eq!(first.deadline_ticks, 49_024);
        assert_eq!(first.codec, "aac-adts");
        assert_eq!(first.payload, first_frame);

        let MuninnMediaWireRecord::Audio(second) =
            decode_media_wire_record(&second_push[1].payload)?
        else {
            panic!("expected audio media record");
        };
        assert_eq!(second.packet_id, 13);
        assert_eq!(second.pts_ticks, 49_024);
        assert_eq!(second.deadline_ticks, 50_048);
        assert_eq!(second.payload, second_frame);
        assert_eq!(sender.pending_bytes(), 0);
        assert_eq!(sender.next_packet_id(), 14);
        Ok(())
    }

    #[test]
    fn adts_stream_send_state_rejects_wrong_sync() -> Result<()> {
        let mut sender = AudioAdtsStreamSendState::new(adts_stream_send_config())?;

        let error = sender
            .push("2026-06-18T00:00:00Z", &[0x47, 0x40, 0x00, 0x10])
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("ADTS frame must start with sync word")
        );
        Ok(())
    }

    #[test]
    fn adts_stream_send_state_rejects_trailing_bytes_on_finish() -> Result<()> {
        let mut sender = AudioAdtsStreamSendState::new(adts_stream_send_config())?;

        let push = sender.push("2026-06-18T00:00:00Z", &[0xff])?;
        assert!(push.is_empty());

        let error = sender.finish("2026-06-18T00:00:00Z").unwrap_err();

        assert!(error.to_string().contains("trailing bytes"));
        Ok(())
    }

    #[test]
    fn adts_stream_send_state_rejects_non_adts_codecs() {
        let mut config = adts_stream_send_config();
        config.codec = "opus".to_string();

        let error = match AudioAdtsStreamSendState::new(config) {
            Ok(_) => panic!("Opus must not create an ADTS stream sender"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("ADTS stream sender requires AAC/ADTS codec")
        );
    }

    #[test]
    fn media_wire_encodes_audio_packet_for_sender() -> Result<()> {
        let wire = encode_audio_packet_wire_record(
            AudioPacketWireOptions {
                packetize: AudioPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "session-1",
                    codec: "opus",
                    packet_id: 12,
                    pts_ticks: 48_000,
                    duration_ticks: 960,
                    timebase_num: 1,
                    timebase_den: 48_000,
                    deadline_ticks: 48_960,
                },
                stored_at: "2026-06-18T00:00:00Z",
                source_runtime_id: "muninn-test",
                source_role: "media-test",
            },
            &[0xf8, 0xff, 0xfe],
        )?;

        let decoded = decode_media_wire_record(&wire)?;
        let MuninnMediaWireRecord::Audio(decoded) = decoded else {
            panic!("expected audio media record");
        };
        assert_eq!(decoded.packet_id, 12);
        assert_eq!(decoded.codec, "opus");
        assert_eq!(decoded.payload, vec![0xf8, 0xff, 0xfe]);
        Ok(())
    }

    #[test]
    fn media_send_payload_pins_audio_to_media_channel() -> Result<()> {
        let payload = audio_packet_send_payload(
            AudioPacketWireOptions {
                packetize: AudioPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "session-1",
                    codec: "opus",
                    packet_id: 12,
                    pts_ticks: 48_000,
                    duration_ticks: 960,
                    timebase_num: 1,
                    timebase_den: 48_000,
                    deadline_ticks: 48_960,
                },
                stored_at: "2026-06-18T00:00:00Z",
                source_runtime_id: "muninn-test",
                source_role: "media-test",
            },
            &[0xf8, 0xff, 0xfe],
        )?;

        assert_eq!(payload.channel_id, MUNINN_MEDIA_RUDP_CHANNEL);
        let MuninnMediaWireRecord::Audio(record) = decode_media_wire_record(&payload.payload)?
        else {
            panic!("expected audio media record");
        };
        assert_eq!(record.packet_id, 12);
        Ok(())
    }

    #[test]
    fn media_wire_round_trips_audio_document() -> Result<()> {
        let record = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )?;

        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Audio(record.clone()),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        assert_eq!(
            decode_media_wire_record(&wire)?,
            MuninnMediaWireRecord::Audio(record)
        );
        Ok(())
    }

    #[test]
    fn media_wire_round_trips_feedback_document() -> Result<()> {
        let record = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: vec![42],
            missing_video_chunk_keys: vec![video_chunk_feedback_key(42, 2)],
            late_frame_ids: vec![39],
            requested_keyframe: true,
            jitter_us: 700,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })?;

        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Feedback(record.clone()),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        assert_eq!(
            decode_media_wire_record(&wire)?,
            MuninnMediaWireRecord::Feedback(record)
        );
        Ok(())
    }

    #[test]
    fn media_wire_decodes_obs_bridge_receiver_feedback_fixture() -> Result<()> {
        let wire = hex_fixture(
            "83ad736368656d6156657273696f6ebb63756c746e65742e646f63756d656e745f7075745f7261772e7630a96d6573736167654964d96a6d756e696e6e2d6d656469613a6d756e696e6e2e6d656469615f72656365697665725f666565646261636b2e76313a6d756e696e6e2e726176656e2e61762e727564703a726176656e3a746573743a766964656f3a666565646261636b3a73746172666972652e6f6273a8646f63756d656e7489a8736368656d614964d9216d756e696e6e2e6d656469615f72656365697665725f666565646261636b2e7631a97265636f72644b6579d93b6d756e696e6e2e726176656e2e61762e727564703a726176656e3a746573743a766964656f3a666565646261636b3a73746172666972652e6f6273a873746f7265644174ac756e69782d6d733a31303030af7061796c6f6164456e636f64696e67ab6d6573736167657061636ba77061796c6f6164c4539bb46d756e696e6e2e726176656e2e61762e72756470b0726176656e3a746573743a766964656fac73746172666972652e6f62732990912ac30000ac756e69782d6d733a3130303092a434323a31a434323a33af736f7572636552756e74696d654964a87374617266697265ad736f757263654167656e744964c0aa736f75726365526f6c65a96d696d69722e6f6273a47461677391ac6d756e696e6e2e6d65646961",
        )?;

        let MuninnMediaWireRecord::Feedback(feedback) = decode_media_wire_record(&wire)? else {
            panic!("expected receiver feedback");
        };

        assert_eq!(feedback.stream_id, "muninn.raven.av.rudp");
        assert_eq!(feedback.session_id, "raven:test:video");
        assert_eq!(feedback.receiver_id, "starfire.obs");
        assert_eq!(feedback.highest_decodable_frame_id, Some(41));
        assert!(feedback.missing_frame_ids.is_empty());
        assert_eq!(feedback.late_frame_ids, vec![42]);
        assert!(feedback.requested_keyframe);
        assert_eq!(feedback.jitter_us, 0);
        assert_eq!(feedback.decode_queue_us, 0);
        assert_eq!(feedback.observed_at, "unix-ms:1000");
        assert_eq!(feedback.missing_video_chunk_keys, vec!["42:1", "42:3"]);
        Ok(())
    }

    #[test]
    fn media_wire_decodes_obs_early_repair_as_live_not_late() -> Result<()> {
        let wire = hex_fixture(
            "83ad736368656d6156657273696f6ebb63756c746e65742e646f63756d656e745f7075745f7261772e7630a96d6573736167654964d96d6d756e696e6e2d6d656469613a6d756e696e6e2e6d656469615f72656365697665725f666565646261636b2e76313a6d756e696e6e2e726176656e2e61762e727564703a726176656e3a73657373696f6e3a766964656f3a666565646261636b3a73746172666972652e6f6273a8646f63756d656e7489a8736368656d614964d9216d756e696e6e2e6d656469615f72656365697665725f666565646261636b2e7631a97265636f72644b6579d93e6d756e696e6e2e726176656e2e61762e727564703a726176656e3a73657373696f6e3a766964656f3a666565646261636b3a73746172666972652e6f6273a873746f7265644174ac756e69782d6d733a31303030af7061796c6f6164456e636f64696e67ab6d6573736167657061636ba77061796c6f6164c4509bb46d756e696e6e2e726176656e2e61762e72756470b3726176656e3a73657373696f6e3a766964656fac73746172666972652e6f6273299090c20000ac756e69782d6d733a3130303091a434323a33af736f7572636552756e74696d654964a87374617266697265ad736f757263654167656e744964c0aa736f75726365526f6c65a96d696d69722e6f6273a47461677391ac6d756e696e6e2e6d65646961",
        )?;

        let MuninnMediaWireRecord::Feedback(feedback) = decode_media_wire_record(&wire)? else {
            panic!("expected receiver feedback");
        };

        assert_eq!(feedback.stream_id, "muninn.raven.av.rudp");
        assert_eq!(feedback.session_id, "raven:session:video");
        assert_eq!(feedback.highest_decodable_frame_id, Some(41));
        assert!(feedback.late_frame_ids.is_empty());
        assert!(!feedback.requested_keyframe);
        assert_eq!(feedback.missing_video_chunk_keys, vec!["42:3"]);
        Ok(())
    }

    #[test]
    fn media_wire_rejects_invalid_video_record_timing() -> Result<()> {
        let access_unit = VideoAccessUnit {
            bytes: vec![1, 2, 3],
            keyframe: true,
        };
        let mut record = packetize_video_access_unit(
            VideoFramePacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "h264",
                frame_id: 9,
                pts_ticks: 27_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                deadline_ticks: 28_800,
                max_payload_bytes: 4,
            },
            &access_unit,
        )?
        .remove(0);
        record.deadline_ticks = 26_999;
        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Video(record),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        let error = decode_media_wire_record(&wire).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("video media record deadline_ticks must not precede pts_ticks")
        );
        Ok(())
    }

    #[test]
    fn media_wire_rejects_invalid_audio_record_payload() -> Result<()> {
        let mut record = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )?;
        record.payload.clear();
        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Audio(record),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        let error = decode_media_wire_record(&wire).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("audio media record payload must be non-empty")
        );
        Ok(())
    }

    #[test]
    fn media_wire_rejects_invalid_feedback_record_pressure() -> Result<()> {
        let mut record = build_receiver_feedback(ReceiverFeedbackOptions {
            stream_id: "muninn.raven.av.rudp",
            session_id: "session-1",
            receiver_id: "starfire.obs",
            highest_decodable_frame_id: Some(40),
            missing_frame_ids: vec![42],
            missing_video_chunk_keys: vec![video_chunk_feedback_key(42, 2)],
            late_frame_ids: vec![39],
            requested_keyframe: true,
            jitter_us: 700,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z",
        })?;
        record.decode_queue_us = -1;
        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Feedback(record),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;

        let error = decode_media_wire_record(&wire).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("receiver feedback media record decode_queue_us must be non-negative")
        );
        Ok(())
    }

    #[test]
    fn feedback_payload_decodes_legacy_record_without_chunk_keys() -> Result<()> {
        #[derive(serde::Serialize)]
        struct LegacyFeedbackRecord {
            stream_id: String,
            session_id: String,
            receiver_id: String,
            highest_decodable_frame_id: Option<u64>,
            missing_frame_ids: Vec<u64>,
            late_frame_ids: Vec<u64>,
            requested_keyframe: bool,
            jitter_us: i64,
            decode_queue_us: i64,
            observed_at: String,
        }

        let payload = encode_record_payload(&LegacyFeedbackRecord {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "session-1".to_string(),
            receiver_id: "starfire.obs".to_string(),
            highest_decodable_frame_id: Some(41),
            missing_frame_ids: vec![42],
            late_frame_ids: vec![40],
            requested_keyframe: true,
            jitter_us: 750,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z".to_string(),
        })?;

        let decoded: MuninnMediaReceiverFeedbackRecord = decode_record_payload(&payload)?;

        assert_eq!(decoded.missing_video_chunk_keys, Vec::<String>::new());
        assert_eq!(decoded.missing_frame_ids, vec![42]);
        assert!(decoded.requested_keyframe);
        Ok(())
    }

    #[test]
    fn media_wire_rejects_mismatched_record_key() -> Result<()> {
        let record = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )?;
        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Audio(record),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;
        let CultNetMessage::DocumentPutRaw {
            message_id,
            mut document,
        } = decode_cultnet_message_from_slice(&wire, CultNetWireContract::CultNetSchemaV0)?
        else {
            panic!("expected raw document put");
        };
        document.record_key = "wrong:key".to_string();
        let tampered = encode_cultnet_message_to_vec(
            &CultNetMessage::DocumentPutRaw {
                message_id,
                document,
            },
            CultNetWireContract::CultNetSchemaV0,
        )?;

        let error = decode_media_wire_record(&tampered).unwrap_err();

        assert!(error.to_string().contains("record key mismatch"));
        Ok(())
    }

    #[test]
    fn media_wire_rejects_unsupported_schema() -> Result<()> {
        let record = packetize_audio_packet(
            AudioPacketizeOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "session-1",
                codec: "opus",
                packet_id: 12,
                pts_ticks: 48_000,
                duration_ticks: 960,
                timebase_num: 1,
                timebase_den: 48_000,
                deadline_ticks: 48_960,
            },
            &[0xf8, 0xff, 0xfe],
        )?;
        let wire = encode_media_wire_record(
            &MuninnMediaWireRecord::Audio(record),
            "2026-06-18T00:00:00Z",
            "muninn-test",
            "media-test",
        )?;
        let CultNetMessage::DocumentPutRaw {
            message_id,
            mut document,
        } = decode_cultnet_message_from_slice(&wire, CultNetWireContract::CultNetSchemaV0)?
        else {
            panic!("expected raw document put");
        };
        document.schema_id = "muninn.media_unknown.v1".to_string();
        let tampered = encode_cultnet_message_to_vec(
            &CultNetMessage::DocumentPutRaw {
                message_id,
                document,
            },
            CultNetWireContract::CultNetSchemaV0,
        )?;

        let error = decode_media_wire_record(&tampered).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported Muninn media schema")
        );
        Ok(())
    }

    #[test]
    fn media_wire_rejects_non_raw_document_messages() -> Result<()> {
        let wire = encode_cultnet_message_to_vec(
            &CultNetMessage::DocumentPut {
                message_id: "not-raw-media".to_string(),
                document: cultnet_rs::CultNetDocumentRecord {
                    schema_id: MUNINN_MEDIA_AUDIO_PACKET_SCHEMA.to_string(),
                    record_key: "muninn.raven.av.rudp:session-1:audio:12".to_string(),
                    stored_at: "2026-06-18T00:00:00Z".to_string(),
                    payload: serde_json::json!({ "packet_id": 12 }),
                    source_runtime_id: Some("muninn-test".to_string()),
                    source_agent_id: None,
                    source_role: Some("media-test".to_string()),
                    tags: Some(vec!["muninn.media".to_string()]),
                },
            },
            CultNetWireContract::CultNetSchemaV0,
        )?;

        let error = decode_media_wire_record(&wire).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("expected cultnet.document_put_raw.v0")
        );
        Ok(())
    }

    #[test]
    fn audio_fec_recovers_every_one_and_two_erasure_permutation() -> Result<()> {
        let data: [Vec<u8>; 4] = std::array::from_fn(|i| {
            (0..37)
                .map(|b| (b as u8).wrapping_mul(17).wrapping_add(i as u8 * 41))
                .collect()
        });
        let records: Vec<_> = data
            .iter()
            .enumerate()
            .map(|(i, payload)| {
                packetize_audio_packet(
                    AudioPacketizeOptions {
                        stream_id: "audio",
                        session_id: "s",
                        codec: "pcm",
                        packet_id: 10 + i as u64,
                        pts_ticks: 480 * i as i64,
                        duration_ticks: 480,
                        timebase_num: 1,
                        timebase_den: 48_000,
                        deadline_ticks: 480 * i as i64 + 2400,
                    },
                    payload,
                )
            })
            .collect::<Result<_>>()?;
        let parity = audio_fec_parity_records(&records)?;
        let all: [Vec<u8>; 6] = std::array::from_fn(|i| {
            if i < 4 {
                data[i].clone()
            } else {
                parity[i - 4].payload.clone()
            }
        });
        for first in 0..6 {
            let mut shards = std::array::from_fn(|i| Some(all[i].clone()));
            shards[first] = None;
            assert_eq!(recover_audio_fec_data(shards)?, data);
            for second in first + 1..6 {
                let mut shards = std::array::from_fn(|i| Some(all[i].clone()));
                shards[first] = None;
                shards[second] = None;
                assert_eq!(
                    recover_audio_fec_data(shards)?,
                    data,
                    "erasures {first},{second}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn audio_fec_rejects_mixed_metadata_and_round_trips_parity_wire() -> Result<()> {
        let mut records: Vec<_> = (0..4)
            .map(|i| {
                packetize_audio_packet(
                    AudioPacketizeOptions {
                        stream_id: "audio",
                        session_id: "s",
                        codec: "pcm",
                        packet_id: i,
                        pts_ticks: i as i64 * 480,
                        duration_ticks: 480,
                        timebase_num: 1,
                        timebase_den: 48_000,
                        deadline_ticks: 5000,
                    },
                    &[i as u8; 16],
                )
            })
            .collect::<Result<_>>()?;
        records[2].codec = "opus".into();
        assert!(audio_fec_parity_records(&records).is_err());
        records[2].codec = "pcm".into();
        let parity = audio_fec_parity_records(&records)?;
        for record in parity {
            let wire = encode_media_wire_record(
                &MuninnMediaWireRecord::AudioParity(record.clone()),
                "now",
                "muninn",
                "sender",
            )?;
            assert_eq!(
                decode_media_wire_record(&wire)?,
                MuninnMediaWireRecord::AudioParity(record)
            );
        }
        let mut invalid = audio_fec_parity_records(&records)?[0].clone();
        invalid.data_shard_count = 3;
        assert!(
            encode_media_wire_record(
                &MuninnMediaWireRecord::AudioParity(invalid),
                "now",
                "muninn",
                "sender"
            )
            .is_err()
        );
        Ok(())
    }

    fn hex_fixture(value: &str) -> Result<Vec<u8>> {
        if value.len() % 2 != 0 {
            return Err(anyhow!("hex fixture must have an even length"));
        }
        let mut bytes = Vec::with_capacity(value.len() / 2);
        for index in (0..value.len()).step_by(2) {
            let byte = u8::from_str_radix(&value[index..index + 2], 16)
                .with_context(|| format!("parsing hex fixture byte at offset {index}"))?;
            bytes.push(byte);
        }
        Ok(bytes)
    }
}
