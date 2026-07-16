use std::net::UdpSocket;
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::{
    CultNetMessage, CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions,
};

pub const CULTNET_OPERATION_CONNECTION_ID: u32 = 0x4355_4c54;

pub struct CultNetOperationServer {
    transport: CultNetRudpSocketTransportConnection,
    endpoint: String,
}

impl CultNetOperationServer {
    pub fn bind(runtime_id: impl Into<String>, host: &str, port: u16) -> Result<Self> {
        let socket = UdpSocket::bind((host, port))?;
        socket.set_read_timeout(Some(Duration::from_millis(25)))?;
        let address = socket.local_addr()?;
        let mut options = CultNetRudpSocketTransportOptions::server(
            runtime_id,
            socket,
            CULTNET_OPERATION_CONNECTION_ID,
        );
        options.max_fragment_bytes = Some(2048);
        Ok(Self {
            transport: CultNetRudpSocketTransportConnection::new(options)?,
            endpoint: format!("rudp://{}:{}", address.ip(), address.port()),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn serve_once(
        &mut self,
        mut handler: impl FnMut(CultNetMessage) -> Result<CultNetMessage>,
    ) -> Result<bool> {
        let Some(request) = self.transport.receive_schema_message_once()? else {
            self.transport.poll_resends()?;
            return Ok(false);
        };
        if !matches!(request, CultNetMessage::OperationRequest { .. }) {
            return Err(anyhow!(
                "CultNet operation service received a non-operation message"
            ));
        }
        let response = handler(request)?;
        if !matches!(response, CultNetMessage::OperationResponse { .. }) {
            return Err(anyhow!(
                "CultNet operation handler returned a non-operation response"
            ));
        }
        self.transport.send_schema_message(&response)?;
        Ok(true)
    }
}
