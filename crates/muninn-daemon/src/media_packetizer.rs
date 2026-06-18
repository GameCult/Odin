use anyhow::{Result, anyhow};
use odin_core::{MuninnMediaAudioPacketRecord, MuninnMediaVideoAccessUnitRecord};

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
}
