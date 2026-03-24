use crate::config::Config;
use crate::error::{Result, SnagError};
use crate::protocol::*;
use std::path::PathBuf;
use tokio::net::UnixStream;

#[allow(dead_code)]
pub struct DaemonClient {
    stream: UnixStream,
}

#[allow(dead_code)]
impl DaemonClient {
    pub async fn connect(config: &Config) -> Result<Self> {
        let socket_path = config.socket_path();

        // Try to connect
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => Ok(Self { stream }),
            Err(_) => {
                // Auto-start daemon
                start_daemon(config)?;
                // Retry connection
                let stream = retry_connect(&socket_path).await?;
                Ok(Self { stream })
            }
        }
    }

    pub async fn request(&mut self, req: &Request) -> Result<Response> {
        let frame = encode_request(req)?;
        write_frame(&mut self.stream, &frame).await?;
        self.read_response().await
    }

    pub async fn read_response(&mut self) -> Result<Response> {
        match read_frame(&mut self.stream).await? {
            Some((msg_type, payload)) => decode_response(msg_type, &payload),
            None => Err(SnagError::ConnectionLost),
        }
    }

    pub async fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        let req = Request::PtyInput(data.to_vec());
        let frame = encode_request(&req)?;
        write_frame(&mut self.stream, &frame).await
    }

    pub async fn send_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let req = Request::Resize { cols, rows };
        let frame = encode_request(&req)?;
        write_frame(&mut self.stream, &frame).await
    }

    pub async fn send_detach(&mut self) -> Result<()> {
        let req = Request::SessionDetach;
        let frame = encode_request(&req)?;
        write_frame(&mut self.stream, &frame).await
    }

    pub fn into_stream(self) -> UnixStream {
        self.stream
    }
}

fn start_daemon(config: &Config) -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| SnagError::DaemonStartFailed(format!("cannot find snag executable: {e}")))?;

    let socket_path = config.socket_path();

    // Ensure socket directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Fork and exec daemon
    match unsafe { nix::unistd::fork()? } {
        nix::unistd::ForkResult::Child => {
            // Detach from terminal
            let _ = nix::unistd::setsid();

            // Redirect stdio to /dev/null
            let devnull = std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/null")
                .expect("cannot open /dev/null");
            use std::os::fd::AsRawFd;
            let _ = nix::unistd::dup2(devnull.as_raw_fd(), nix::libc::STDIN_FILENO);
            let _ = nix::unistd::dup2(devnull.as_raw_fd(), nix::libc::STDOUT_FILENO);
            // Keep stderr for daemon logging
            let log_path = socket_path.parent().unwrap().join("snagd.log");
            if let Ok(log) = std::fs::File::create(&log_path) {
                let _ = nix::unistd::dup2(log.as_raw_fd(), nix::libc::STDERR_FILENO);
            }

            let exe_cstr =
                std::ffi::CString::new(exe.to_string_lossy().as_ref()).expect("invalid exe path");
            let arg_daemon = std::ffi::CString::new("daemon").unwrap();
            let arg_start = std::ffi::CString::new("start").unwrap();
            let arg_socket = std::ffi::CString::new("--socket").unwrap();
            let socket_str = std::ffi::CString::new(socket_path.to_string_lossy().as_ref())
                .expect("invalid socket path");
            let args = [
                exe_cstr.clone(),
                arg_daemon,
                arg_start,
                arg_socket,
                socket_str,
            ];
            let _ = nix::unistd::execvp(&exe_cstr, &args);
            unsafe { nix::libc::_exit(1) };
        }
        nix::unistd::ForkResult::Parent { .. } => {
            // Wait for socket to appear
            Ok(())
        }
    }
}

async fn retry_connect(socket_path: &PathBuf) -> Result<UnixStream> {
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(stream) = UnixStream::connect(socket_path).await {
            return Ok(stream);
        }
    }
    Err(SnagError::DaemonStartFailed(
        "timeout waiting for daemon to start".to_string(),
    ))
}
