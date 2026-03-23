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
        Request::SessionList => MSG_SESSION_LIST,
        Request::SessionInfo { .. } => MSG_SESSION_INFO,
        Request::SessionAttach { .. } => MSG_SESSION_ATTACH,
        Request::SessionDetach => MSG_SESSION_DETACH,
        Request::SessionSend { .. } => MSG_SESSION_SEND,
        Request::SessionOutput { .. } => MSG_SESSION_OUTPUT,
        Request::SessionCwd { .. } => MSG_SESSION_CWD,
        Request::SessionPs { .. } => MSG_SESSION_PS,
        Request::SessionRegister { .. } => MSG_SESSION_REGISTER,
        Request::SessionUnregister { .. } => MSG_SESSION_UNREGISTER,
        Request::SessionGrep { .. } => MSG_SESSION_GREP,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_roundtrip_session_new() {
        let req = Request::SessionNew {
            shell: Some("/bin/zsh".to_string()),
            name: Some("dev".to_string()),
            cwd: Some("/home/user".to_string()),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_NEW);
        let len = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
        assert_eq!(frame.len(), 5 + len);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionNew { shell, name, cwd } => {
                assert_eq!(shell.as_deref(), Some("/bin/zsh"));
                assert_eq!(name.as_deref(), Some("dev"));
                assert_eq!(cwd.as_deref(), Some("/home/user"));
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_kill() {
        let req = Request::SessionKill {
            target: "dev".to_string(),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_KILL);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionKill { target } => assert_eq!(target, "dev"),
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_list() {
        let req = Request::SessionList;
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_LIST);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        assert!(matches!(decoded, Request::SessionList));
    }

    #[test]
    fn test_request_roundtrip_session_attach() {
        let req = Request::SessionAttach {
            target: "abc123".to_string(),
            read_only: true,
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_ATTACH);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionAttach { target, read_only } => {
                assert_eq!(target, "abc123");
                assert!(read_only);
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_send() {
        let req = Request::SessionSend {
            target: "dev".to_string(),
            input: "cargo test".to_string(),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_SEND);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionSend { target, input } => {
                assert_eq!(target, "dev");
                assert_eq!(input, "cargo test");
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_resize() {
        let req = Request::Resize {
            cols: 120,
            rows: 40,
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_RESIZE);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::Resize { cols, rows } => {
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_pty_input() {
        let data = b"hello world\r\n".to_vec();
        let req = Request::PtyInput(data.clone());
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_PTY_INPUT);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::PtyInput(d) => assert_eq!(d, data),
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_daemon_status() {
        let req = Request::DaemonStatus;
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_DAEMON_STATUS);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        assert!(matches!(decoded, Request::DaemonStatus));
    }

    #[test]
    fn test_request_roundtrip_session_rename() {
        let req = Request::SessionRename {
            target: "abc".to_string(),
            new_name: "dev".to_string(),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_RENAME);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionRename { target, new_name } => {
                assert_eq!(target, "abc");
                assert_eq!(new_name, "dev");
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_output() {
        let req = Request::SessionOutput {
            target: "dev".to_string(),
            lines: Some(10),
            follow: true,
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_OUTPUT);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionOutput {
                target,
                lines,
                follow,
            } => {
                assert_eq!(target, "dev");
                assert_eq!(lines, Some(10));
                assert!(follow);
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_register() {
        let req = Request::SessionRegister {
            pts: "/dev/pts/3".to_string(),
            name: Some("my-shell".to_string()),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_REGISTER);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionRegister { pts, name } => {
                assert_eq!(pts, "/dev/pts/3");
                assert_eq!(name.as_deref(), Some("my-shell"));
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_unregister() {
        let req = Request::SessionUnregister {
            target: "foo".to_string(),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_UNREGISTER);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionUnregister { target } => {
                assert_eq!(target, "foo");
            }
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_request_roundtrip_session_grep() {
        let req = Request::SessionGrep {
            pattern: "Hello".to_string(),
        };
        let frame = encode_request(&req).unwrap();
        assert_eq!(frame[0], MSG_SESSION_GREP);
        let decoded = decode_request(frame[0], &frame[5..]).unwrap();
        match decoded {
            Request::SessionGrep { pattern } => assert_eq!(pattern, "Hello"),
            _ => panic!("wrong request type"),
        }
    }

    #[test]
    fn test_response_roundtrip_ok_session_created() {
        let resp = Response::Ok(ResponseData::SessionCreated {
            id: "abcdef1234567890".to_string(),
        });
        let frame = encode_response(&resp).unwrap();
        assert_eq!(frame[0], MSG_OK);
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::Ok(ResponseData::SessionCreated { id }) => {
                assert_eq!(id, "abcdef1234567890");
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_ok_session_registered() {
        let resp = Response::Ok(ResponseData::SessionRegistered {
            id: "abc123".to_string(),
            capture_path: "/run/user/1000/snag/capture-abc123".to_string(),
        });
        let frame = encode_response(&resp).unwrap();
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::Ok(ResponseData::SessionRegistered { id, capture_path }) => {
                assert_eq!(id, "abc123");
                assert_eq!(capture_path, "/run/user/1000/snag/capture-abc123");
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_ok_session_list() {
        let resp = Response::Ok(ResponseData::SessionList(vec![SessionInfo {
            id: "abc123".to_string(),
            name: Some("dev".to_string()),
            shell: "/bin/zsh".to_string(),
            cwd: "/home/user".to_string(),
            state: "running".to_string(),
            fg_process: Some("cargo".to_string()),
            attached: 1,
            registered: false,
            created_at: "2026-03-22T10:00:00Z".to_string(),
        }]));
        let frame = encode_response(&resp).unwrap();
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::Ok(ResponseData::SessionList(sessions)) => {
                assert_eq!(sessions.len(), 1);
                assert_eq!(sessions[0].id, "abc123");
                assert_eq!(sessions[0].name.as_deref(), Some("dev"));
                assert_eq!(sessions[0].attached, 1);
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_error() {
        let resp = Response::Error {
            code: 42,
            message: "session not found".to_string(),
        };
        let frame = encode_response(&resp).unwrap();
        assert_eq!(frame[0], MSG_ERROR);
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::Error { code, message } => {
                assert_eq!(code, 42);
                assert_eq!(message, "session not found");
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_pty_output() {
        let data = b"\x1b[32mhello\x1b[0m\r\n".to_vec();
        let resp = Response::PtyOutput(data.clone());
        let frame = encode_response(&resp).unwrap();
        assert_eq!(frame[0], MSG_PTY_OUTPUT);
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::PtyOutput(d) => assert_eq!(d, data),
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_session_event() {
        let resp = Response::SessionEvent {
            event: "exited".to_string(),
            session_id: "abc123".to_string(),
        };
        let frame = encode_response(&resp).unwrap();
        assert_eq!(frame[0], MSG_SESSION_EVENT);
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::SessionEvent { event, session_id } => {
                assert_eq!(event, "exited");
                assert_eq!(session_id, "abc123");
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_response_roundtrip_daemon_status() {
        let resp = Response::Ok(ResponseData::DaemonStatus {
            pid: 12345,
            uptime_secs: 3600,
            session_count: 3,
        });
        let frame = encode_response(&resp).unwrap();
        let decoded = decode_response(frame[0], &frame[5..]).unwrap();
        match decoded {
            Response::Ok(ResponseData::DaemonStatus {
                pid,
                uptime_secs,
                session_count,
            }) => {
                assert_eq!(pid, 12345);
                assert_eq!(uptime_secs, 3600);
                assert_eq!(session_count, 3);
            }
            _ => panic!("wrong response type"),
        }
    }

    #[test]
    fn test_frame_header_size() {
        let req = Request::SessionDetach;
        let frame = encode_request(&req).unwrap();
        assert!(frame.len() >= 5); // minimum: 5-byte header
        let len = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
        assert_eq!(frame.len(), 5 + len);
    }

    #[tokio::test]
    async fn test_read_write_frame_roundtrip() {
        let req = Request::SessionNew {
            shell: Some("/bin/bash".to_string()),
            name: Some("test".to_string()),
            cwd: None,
        };
        let frame = encode_request(&req).unwrap();

        // Write to a buffer and read back
        let mut buf = Vec::new();
        write_frame(&mut buf, &frame).await.unwrap();

        let mut reader = &buf[..];
        let result = read_frame(&mut reader).await.unwrap();
        assert!(result.is_some());
        let (msg_type, payload) = result.unwrap();
        assert_eq!(msg_type, MSG_SESSION_NEW);

        let decoded = decode_request(msg_type, &payload).unwrap();
        match decoded {
            Request::SessionNew { shell, name, cwd } => {
                assert_eq!(shell.as_deref(), Some("/bin/bash"));
                assert_eq!(name.as_deref(), Some("test"));
                assert!(cwd.is_none());
            }
            _ => panic!("wrong request type"),
        }
    }

    #[tokio::test]
    async fn test_read_frame_eof() {
        let buf: &[u8] = &[];
        let result = read_frame(&mut &*buf).await.unwrap();
        assert!(result.is_none());
    }
}
