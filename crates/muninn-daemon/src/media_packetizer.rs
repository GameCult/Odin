use anyhow::{Result, anyhow};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding, CultNetWireContract,
    decode_cultnet_message_from_slice, encode_cultnet_message_to_vec,
};
use odin_core::{
    MUNINN_MEDIA_AUDIO_PACKET_SCHEMA, MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA,
    MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA, MuninnMediaAudioPacketRecord,
    MuninnMediaReceiverFeedbackRecord, MuninnMediaVideoAccessUnitRecord,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

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

pub fn h264_annex_b_access_units(input: &[u8]) -> Result<Vec<VideoAccessUnit>> {
    let nal_units = h264_annex_b_nal_units(input)?;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MuninnMediaWireRecord {
    Video(MuninnMediaVideoAccessUnitRecord),
    Audio(MuninnMediaAudioPacketRecord),
    Feedback(MuninnMediaReceiverFeedbackRecord),
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
            dependency_frame_id: if access_unit.keyframe {
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

    let requested_keyframe = options.requested_keyframe
        || !missing_frame_ids.is_empty()
        || !missing_video_chunk_keys.is_empty();

    Ok(MuninnMediaReceiverFeedbackRecord {
        stream_id: options.stream_id.to_string(),
        session_id: options.session_id.to_string(),
        receiver_id: options.receiver_id.to_string(),
        highest_decodable_frame_id: options.highest_decodable_frame_id,
        missing_frame_ids,
        late_frame_ids,
        requested_keyframe,
        jitter_us: options.jitter_us,
        decode_queue_us: options.decode_queue_us,
        observed_at: options.observed_at.to_string(),
        missing_video_chunk_keys,
    })
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
            encode_record_payload(record)?,
        ),
        MuninnMediaWireRecord::Audio(record) => (
            MUNINN_MEDIA_AUDIO_PACKET_SCHEMA,
            audio_record_key(record),
            encode_record_payload(record)?,
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
        MUNINN_MEDIA_AUDIO_PACKET_SCHEMA => {
            let record: MuninnMediaAudioPacketRecord = decode_record_payload(&document.payload)?;
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
        MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA => {
            let record: MuninnMediaReceiverFeedbackRecord =
                decode_record_payload(&document.payload)?;
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

fn decode_record_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T> {
    rmp_serde::from_slice(payload).map_err(Into::into)
}

fn video_record_key(record: &MuninnMediaVideoAccessUnitRecord) -> String {
    format!(
        "{}:{}:video:{}:{}",
        record.stream_id, record.session_id, record.frame_id, record.chunk_index
    )
}

fn audio_record_key(record: &MuninnMediaAudioPacketRecord) -> String {
    format!(
        "{}:{}:audio:{}",
        record.stream_id, record.session_id, record.packet_id
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

fn h264_annex_b_nal_units(input: &[u8]) -> Result<Vec<NalUnit<'_>>> {
    let mut starts = Vec::new();
    let mut offset = 0_usize;
    while let Some((start, prefix_len)) = find_annex_b_start_code(input, offset) {
        starts.push((start, prefix_len));
        offset = start + prefix_len;
    }

    if starts.is_empty() {
        return Err(anyhow!("H.264 Annex B stream has no start codes"));
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
        let nal_type = payload[0] & 0x1f;
        nal_units.push(NalUnit {
            start,
            end,
            payload,
            nal_type,
        });
    }

    if nal_units.is_empty() {
        return Err(anyhow!("H.264 Annex B stream has no NAL payloads"));
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

fn h264_first_mb_in_slice(nal_payload: &[u8]) -> Option<u64> {
    if nal_payload.len() < 2 {
        return None;
    }
    let rbsp = h264_ebsp_to_rbsp(&nal_payload[1..]);
    read_unsigned_exp_golomb(&rbsp)
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
    fn builds_receiver_feedback_with_sorted_unique_damage_lists() -> Result<()> {
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
}
