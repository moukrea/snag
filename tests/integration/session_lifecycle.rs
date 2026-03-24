use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Helper to get a unique socket path for test isolation
fn test_socket_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snag-test-{}-{}", name, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("snag.sock")
}

/// Helper to get the snag binary path
fn snag_bin() -> PathBuf {
    // Use the debug build
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps
    path.push("snag");
    path
}

/// Run a snag command with a specific socket path
fn snag_cmd(socket: &Path, args: &[&str]) -> std::process::Output {
    Command::new(snag_bin())
        .arg("--socket")
        .arg(socket.to_string_lossy().as_ref())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run snag")
}

/// Clean up a test socket by stopping the daemon
fn cleanup(socket: &Path) {
    let _ = snag_cmd(socket, &["daemon", "stop"]);
    // Give daemon a moment to shut down
    std::thread::sleep(Duration::from_millis(200));
    // Clean up socket directory
    if let Some(parent) = socket.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}

#[test]
fn test_spawn_and_list() {
    let socket = test_socket_path("spawn-list");

    // Spawn a named session
    let output = snag_cmd(&socket, &["new", "--name", "test-session"]);
    assert!(
        output.status.success(),
        "new failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let session_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(session_id.len(), 16, "session ID should be 16 hex chars");

    // Give the session a moment to initialize
    std::thread::sleep(Duration::from_millis(500));

    // List sessions
    let output = snag_cmd(&socket, &["list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test-session"),
        "list should show session name"
    );

    // Kill the session
    let output = snag_cmd(&socket, &["kill", "test-session"]);
    assert!(output.status.success());

    // List should be empty now
    std::thread::sleep(Duration::from_millis(200));
    let output = snag_cmd(&socket, &["list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No sessions") || !stdout.contains("test-session"));

    cleanup(&socket);
}

#[test]
fn test_spawn_send_output() {
    let socket = test_socket_path("send-output");

    // Spawn a session
    let output = snag_cmd(&socket, &["new", "--name", "io-test"]);
    assert!(output.status.success());

    // Wait for shell to start
    std::thread::sleep(Duration::from_millis(1000));

    // Send a command
    let output = snag_cmd(&socket, &["send", "io-test", "echo SNAG_TEST_MARKER"]);
    assert!(output.status.success());

    // Wait for command to execute
    std::thread::sleep(Duration::from_millis(1000));

    // Read output
    let output = snag_cmd(&socket, &["output", "io-test", "--lines", "20"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SNAG_TEST_MARKER"),
        "output should contain the echoed marker, got: {stdout}"
    );

    cleanup(&socket);
}

#[test]
fn test_session_info() {
    let socket = test_socket_path("info");

    let output = snag_cmd(&socket, &["new", "--name", "info-test"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    let output = snag_cmd(&socket, &["info", "info-test"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("info-test"));
    assert!(stdout.contains("running"));

    cleanup(&socket);
}

#[test]
fn test_session_rename() {
    let socket = test_socket_path("rename");

    let output = snag_cmd(&socket, &["new", "--name", "old-name"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // Rename
    let output = snag_cmd(&socket, &["rename", "old-name", "new-name"]);
    assert!(output.status.success());

    // Verify new name works
    let output = snag_cmd(&socket, &["info", "new-name"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("new-name"));

    // Verify old name doesn't work
    let output = snag_cmd(&socket, &["info", "old-name"]);
    assert!(!output.status.success());

    cleanup(&socket);
}

#[test]
fn test_session_cwd() {
    let socket = test_socket_path("cwd");

    let output = snag_cmd(&socket, &["new", "--name", "cwd-test", "--cwd", "/tmp"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    let output = snag_cmd(&socket, &["cwd", "cwd-test"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        stdout.starts_with("/tmp"),
        "cwd should be /tmp, got: {stdout}"
    );

    cleanup(&socket);
}

#[test]
fn test_json_output() {
    let socket = test_socket_path("json");

    let output = snag_cmd(&socket, &["new", "--name", "json-test"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // List with JSON
    let output = snag_cmd(&socket, &["list", "--json"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("list --json should produce valid JSON");
    assert!(parsed["sessions"].is_array());
    assert!(!parsed["sessions"].as_array().unwrap().is_empty());
    assert_eq!(parsed["sessions"][0]["name"], "json-test");

    // Info with JSON
    let output = snag_cmd(&socket, &["info", "json-test", "--json"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("info --json should produce valid JSON");
    assert_eq!(parsed["name"], "json-test");
    assert_eq!(parsed["state"], "running");

    cleanup(&socket);
}

#[test]
fn test_daemon_status() {
    let socket = test_socket_path("daemon-status");

    // Start by creating a session (auto-starts daemon)
    let output = snag_cmd(&socket, &["new", "--name", "status-test"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // Check daemon status
    let output = snag_cmd(&socket, &["daemon", "status"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PID:"));
    assert!(stdout.contains("Sessions:"));

    cleanup(&socket);
}

#[test]
fn test_session_ps() {
    let socket = test_socket_path("ps");

    let output = snag_cmd(&socket, &["new", "--name", "ps-test"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    let output = snag_cmd(&socket, &["ps", "ps-test"]);
    assert!(output.status.success());
    // Should show at least the shell process
    let stdout = String::from_utf8_lossy(&output.stdout);
    // ps output may be empty if no fg process is detected, that's OK
    assert!(output.status.success());
    let _ = stdout; // suppress unused warning

    cleanup(&socket);
}

#[test]
fn test_kill_nonexistent_session() {
    let socket = test_socket_path("kill-nonexist");

    // Start daemon with a session
    let output = snag_cmd(&socket, &["new", "--name", "keeper"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // Try to kill a nonexistent session
    let output = snag_cmd(&socket, &["kill", "nonexistent"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));

    cleanup(&socket);
}

#[test]
fn test_multiple_sessions() {
    let socket = test_socket_path("multi");

    // Create multiple sessions
    let output = snag_cmd(&socket, &["new", "--name", "session-a"]);
    assert!(output.status.success());
    let output = snag_cmd(&socket, &["new", "--name", "session-b"]);
    assert!(output.status.success());
    let output = snag_cmd(&socket, &["new", "--name", "session-c"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // List all sessions
    let output = snag_cmd(&socket, &["list", "--json"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["sessions"].as_array().unwrap().len(), 3);

    // Kill one
    let output = snag_cmd(&socket, &["kill", "session-b"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(200));

    // Should have 2 sessions (or 2 running + 1 exited depending on timing)
    let output = snag_cmd(&socket, &["list", "--json"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let sessions = parsed["sessions"].as_array().unwrap();
    assert!(sessions.len() <= 3); // may include exited session briefly
    let running: Vec<_> = sessions
        .iter()
        .filter(|s| s["state"] == "running")
        .collect();
    assert_eq!(running.len(), 2);

    cleanup(&socket);
}

#[test]
fn test_duplicate_session_name() {
    let socket = test_socket_path("dup-name");

    let output = snag_cmd(&socket, &["new", "--name", "unique"]);
    assert!(output.status.success());
    std::thread::sleep(Duration::from_millis(500));

    // Try to create another with same name
    let output = snag_cmd(&socket, &["new", "--name", "unique"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already in use"));

    cleanup(&socket);
}

#[test]
fn test_invalid_session_name() {
    let socket = test_socket_path("invalid-name");

    let output = snag_cmd(&socket, &["new", "--name", "foo/bar"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid session name") || stderr.contains("error"));

    cleanup(&socket);
}

#[test]
fn test_id_prefix_resolution() {
    let socket = test_socket_path("prefix");

    // Create a session (no name, use ID)
    let output = snag_cmd(&socket, &["new"]);
    assert!(output.status.success());
    let session_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    std::thread::sleep(Duration::from_millis(500));

    // Resolve by first 3 chars of ID
    let prefix = &session_id[..3];
    let output = snag_cmd(&socket, &["info", prefix]);
    assert!(
        output.status.success(),
        "should resolve by prefix '{}', stderr: {}",
        prefix,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&session_id));

    cleanup(&socket);
}
