use crate::{
    CultNetMessage, CultNetWireContract, decode_cultnet_message_from_slice,
    encode_cultnet_message_to_vec,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use uuid::Uuid;

pub const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
pub const DEFAULT_CULTNET_RUDP_MAX_DATAGRAM_BYTES: usize = 60_000;
pub const DEFAULT_CULTNET_RUDP_RETRIES: usize = 5;
pub const DEFAULT_CULTNET_RUDP_ACK_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpOptions {
    pub wire_contract: CultNetWireContract,
    pub max_datagram_bytes: usize,
    pub retries: usize,
    pub ack_timeout: Duration,
}

impl Default for CultNetRudpOptions {
    fn default() -> Self {
        Self {
            wire_contract: CultNetWireContract::CultNetSchemaV0,
            max_datagram_bytes: DEFAULT_CULTNET_RUDP_MAX_DATAGRAM_BYTES,
            retries: DEFAULT_CULTNET_RUDP_RETRIES,
            ack_timeout: DEFAULT_CULTNET_RUDP_ACK_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "packetType", rename_all = "camelCase")]
pub enum CultNetRudpPacket {
    #[serde(rename = "cultnet.rudp.data.v0", rename_all = "camelCase")]
    Data {
        protocol_id: String,
        transfer_id: String,
        sequence: u64,
        wire_contract: CultNetWireContract,
        #[serde(with = "serde_bytes")]
        payload: Vec<u8>,
    },
    #[serde(rename = "cultnet.rudp.ack.v0", rename_all = "camelCase")]
    Ack {
        protocol_id: String,
        transfer_id: String,
        sequence: u64,
    },
}

pub fn encode_cultnet_rudp_packet(packet: &CultNetRudpPacket) -> Result<Vec<u8>> {
    Ok(rmp_serde::to_vec(packet)?)
}

pub fn decode_cultnet_rudp_packet(bytes: &[u8]) -> Result<CultNetRudpPacket> {
    let packet: CultNetRudpPacket = rmp_serde::from_slice(bytes)?;
    match &packet {
        CultNetRudpPacket::Data { protocol_id, .. }
        | CultNetRudpPacket::Ack { protocol_id, .. } => {
            if protocol_id != CULTNET_RUDP_PROTOCOL_ID {
                return Err(anyhow!(
                    "unsupported CultNet RUDP protocol id: {protocol_id}"
                ));
            }
        }
    }
    Ok(packet)
}

pub fn create_cultnet_rudp_data_packet(
    transfer_id: impl Into<String>,
    sequence: u64,
    message: &CultNetMessage,
    wire_contract: CultNetWireContract,
) -> Result<CultNetRudpPacket> {
    Ok(CultNetRudpPacket::Data {
        protocol_id: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        transfer_id: transfer_id.into(),
        sequence,
        wire_contract,
        payload: encode_cultnet_message_to_vec(message, wire_contract)?,
    })
}

pub fn create_cultnet_rudp_ack_packet(
    transfer_id: impl Into<String>,
    sequence: u64,
) -> CultNetRudpPacket {
    CultNetRudpPacket::Ack {
        protocol_id: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        transfer_id: transfer_id.into(),
        sequence,
    }
}

pub fn send_cultnet_message_rudp(
    socket: &UdpSocket,
    target: SocketAddr,
    message: &CultNetMessage,
    options: &CultNetRudpOptions,
) -> Result<String> {
    let transfer_id = Uuid::new_v4().to_string();
    send_cultnet_message_rudp_with_transfer_id(socket, target, message, options, &transfer_id)?;
    Ok(transfer_id)
}

pub fn send_cultnet_message_rudp_with_transfer_id(
    socket: &UdpSocket,
    target: SocketAddr,
    message: &CultNetMessage,
    options: &CultNetRudpOptions,
    transfer_id: &str,
) -> Result<()> {
    if options.retries == 0 {
        return Err(anyhow!("CultNet RUDP retries must be greater than zero"));
    }

    let data_packet =
        create_cultnet_rudp_data_packet(transfer_id, 0, message, options.wire_contract)?;
    let data_bytes = encode_cultnet_rudp_packet(&data_packet)?;
    if data_bytes.len() > options.max_datagram_bytes {
        return Err(anyhow!(
            "CultNet RUDP packet is {} bytes, exceeding max datagram size {}",
            data_bytes.len(),
            options.max_datagram_bytes
        ));
    }

    let previous_timeout = socket.read_timeout()?;
    socket.set_read_timeout(Some(options.ack_timeout))?;
    let mut buffer = vec![0_u8; options.max_datagram_bytes];
    let mut last_error = None;

    for _ in 0..options.retries {
        socket.send_to(&data_bytes, target)?;
        match socket.recv_from(&mut buffer) {
            Ok((size, source)) if source == target => {
                match decode_cultnet_rudp_packet(&buffer[..size]) {
                    Ok(CultNetRudpPacket::Ack {
                        transfer_id: ack_transfer_id,
                        sequence,
                        ..
                    }) if ack_transfer_id == transfer_id && sequence == 0 => {
                        socket.set_read_timeout(previous_timeout)?;
                        return Ok(());
                    }
                    Ok(_) => {}
                    Err(error) => last_error = Some(error),
                }
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                socket.set_read_timeout(previous_timeout)?;
                return Err(error.into());
            }
        }
    }

    socket.set_read_timeout(previous_timeout)?;
    match last_error {
        Some(error) => Err(anyhow!("CultNet RUDP acknowledgement failed: {error}")),
        None => Err(anyhow!(
            "CultNet RUDP acknowledgement timed out for transfer {transfer_id}"
        )),
    }
}

pub fn recv_cultnet_message_rudp(
    socket: &UdpSocket,
    options: &CultNetRudpOptions,
) -> Result<(CultNetMessage, SocketAddr, String)> {
    let mut buffer = vec![0_u8; options.max_datagram_bytes];
    loop {
        let (size, source) = socket.recv_from(&mut buffer)?;
        let packet = decode_cultnet_rudp_packet(&buffer[..size])?;
        let CultNetRudpPacket::Data {
            transfer_id,
            sequence,
            wire_contract,
            payload,
            ..
        } = packet
        else {
            continue;
        };
        let message = decode_cultnet_message_from_slice(&payload, wire_contract)?;
        let ack = create_cultnet_rudp_ack_packet(transfer_id.clone(), sequence);
        let ack_bytes = encode_cultnet_rudp_packet(&ack)?;
        socket.send_to(&ack_bytes, source)?;
        return Ok((message, source, transfer_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_message(runtime_id: &str) -> CultNetMessage {
        CultNetMessage::Hello {
            runtime_id: runtime_id.to_string(),
            runtime_kind: "test".to_string(),
            agent_id: None,
            role: None,
            display_name: None,
            supported_document_types: None,
            supported_mutation_contracts: None,
            supported_message_versions: None,
            supports_schema_catalog: Some(true),
        }
    }

    #[test]
    fn rudp_packet_round_trips_cultnet_message() -> Result<()> {
        let message = hello_message("sender");
        let packet = create_cultnet_rudp_data_packet(
            "transfer-1",
            0,
            &message,
            CultNetWireContract::CultNetSchemaV0,
        )?;
        let encoded = encode_cultnet_rudp_packet(&packet)?;
        let decoded = decode_cultnet_rudp_packet(&encoded)?;

        let CultNetRudpPacket::Data {
            payload,
            wire_contract,
            ..
        } = decoded
        else {
            return Err(anyhow!("expected data packet"));
        };
        assert_eq!(
            decode_cultnet_message_from_slice(&payload, wire_contract)?,
            message
        );
        Ok(())
    }

    #[test]
    fn rudp_socket_send_receives_acknowledged_message() -> Result<()> {
        let sender = UdpSocket::bind("127.0.0.1:0")?;
        let receiver = UdpSocket::bind("127.0.0.1:0")?;
        let receiver_addr = receiver.local_addr()?;
        let options = CultNetRudpOptions {
            ack_timeout: Duration::from_millis(100),
            retries: 3,
            ..CultNetRudpOptions::default()
        };
        let receiver_options = options.clone();

        let handle = std::thread::spawn(move || -> Result<(CultNetMessage, String)> {
            let (message, _source, transfer_id) =
                recv_cultnet_message_rudp(&receiver, &receiver_options)?;
            Ok((message, transfer_id))
        });

        let message = hello_message("sender");
        let transfer_id = send_cultnet_message_rudp(&sender, receiver_addr, &message, &options)?;
        let (received, received_transfer_id) = handle.join().expect("receiver thread panicked")?;

        assert_eq!(received, message);
        assert_eq!(received_transfer_id, transfer_id);
        Ok(())
    }

    #[test]
    fn rudp_sender_retries_until_ack_arrives() -> Result<()> {
        let sender = UdpSocket::bind("127.0.0.1:0")?;
        let receiver = UdpSocket::bind("127.0.0.1:0")?;
        let receiver_addr = receiver.local_addr()?;
        let options = CultNetRudpOptions {
            ack_timeout: Duration::from_millis(50),
            retries: 3,
            ..CultNetRudpOptions::default()
        };

        let handle = std::thread::spawn(move || -> Result<()> {
            let mut buffer = [0_u8; DEFAULT_CULTNET_RUDP_MAX_DATAGRAM_BYTES];
            let (_first_size, _source) = receiver.recv_from(&mut buffer)?;
            let (second_size, source) = receiver.recv_from(&mut buffer)?;
            let CultNetRudpPacket::Data {
                transfer_id,
                sequence,
                ..
            } = decode_cultnet_rudp_packet(&buffer[..second_size])?
            else {
                return Err(anyhow!("expected retried data packet"));
            };
            let ack = create_cultnet_rudp_ack_packet(transfer_id, sequence);
            receiver.send_to(&encode_cultnet_rudp_packet(&ack)?, source)?;
            Ok(())
        });

        send_cultnet_message_rudp(&sender, receiver_addr, &hello_message("sender"), &options)?;
        handle.join().expect("receiver thread panicked")?;
        Ok(())
    }
}
