use crate::error::{Result, SnagError};
use crate::protocol::types::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const HEADER_SIZE: usize = 5;
const MAX_PAYLOAD_SIZE: u32 = 16 * 1024 * 1024; // 16 MB

fn msg_type_for_request(req: &Request) -> u8 {
    match req {
        Request::SessionNew { .. } => MSG_SESSION_NEW,
        Request::SessionKill { .. } => MSG_SESSION_KILL,
        Request::SessionRename { .. } => MSG_SESSION_RENAME,
        Request::SessionList { .. } => MSG_SESSION_LIST,
        Request::SessionInfo { .. } => MSG_SESSION_INFO,
        Request::SessionAttach { .. } => MSG_SESSION_ATTACH,
        Request::SessionDetach => MSG_SESSION_DETACH,
        Request::SessionSend { .. } => MSG_SESSION_SEND,
        Request::SessionOutput { .. } => MSG_SESSION_OUTPUT,
        Request::SessionCwd { .. } => MSG_SESSION_CWD,
        Request::SessionPs { .. } => MSG_SESSION_PS,
        Request::SessionScan => MSG_SESSION_SCAN,
        Request::SessionAdopt { .. } => MSG_SESSION_ADOPT,
        Request::Resize { .. } => MSG_RESIZE,
        Request::PtyInput(_) => MSG_PTY_INPUT,
        Request::DaemonStatus => MSG_DAEMON_STATUS,
        Request::DaemonStop => MSG_DAEMON_STOP,
    }
}

fn msg_type_for_response(resp: &Response) -> u8 {
    match resp {
        Response::Ok(_) => MSG_OK,
        Response::Error { .. } => MSG_ERROR,
        Response::PtyOutput(_) => MSG_PTY_OUTPUT,
        Response::SessionEvent { .. } => MSG_SESSION_EVENT,
    }
}

pub fn encode_request(req: &Request) -> Result<Vec<u8>> {
    let msg_type = msg_type_for_request(req);
    let payload = match req {
        Request::PtyInput(data) => data.clone(),
        other => rmp_serde::to_vec(other)
            .map_err(|e| SnagError::ProtocolError(format!("encode error: {e}")))?,
    };
    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.push(msg_type);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn encode_response(resp: &Response) -> Result<Vec<u8>> {
    let msg_type = msg_type_for_response(resp);
    let payload = match resp {
        Response::PtyOutput(data) => data.clone(),
        other => rmp_serde::to_vec(other)
            .map_err(|e| SnagError::ProtocolError(format!("encode error: {e}")))?,
    };
    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.push(msg_type);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, data: &[u8]) -> Result<()> {
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; HEADER_SIZE];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let msg_type = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);

    if len > MAX_PAYLOAD_SIZE {
        return Err(SnagError::ProtocolError(format!(
            "payload too large: {len} bytes"
        )));
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;

    Ok(Some((msg_type, payload)))
}

pub fn decode_request(msg_type: u8, payload: &[u8]) -> Result<Request> {
    match msg_type {
        MSG_PTY_INPUT => Ok(Request::PtyInput(payload.to_vec())),
        _ => rmp_serde::from_slice(payload)
            .map_err(|e| SnagError::ProtocolError(format!("decode error: {e}"))),
    }
}

pub fn decode_response(msg_type: u8, payload: &[u8]) -> Result<Response> {
    match msg_type {
        MSG_PTY_OUTPUT => Ok(Response::PtyOutput(payload.to_vec())),
        _ => rmp_serde::from_slice(payload)
            .map_err(|e| SnagError::ProtocolError(format!("decode error: {e}"))),
    }
}
