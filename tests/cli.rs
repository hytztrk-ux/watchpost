//! Runs the real `watchpost` binary.

use std::process::Command;

fn watchpost() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchpost"));
    c.env_remove("WATCHPOST_CONFIG").env("NO_COLOR", "1");
    c
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn init_writes_a_valid_config_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let out = watchpost()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let written = std::fs::read_to_string(dir.path().join("watchpost.toml")).unwrap();
    assert_eq!(written, watchpost::config::EXAMPLE_CONFIG);

    let out = watchpost()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already exists"));

    let out = watchpost()
        .args(["init", "--force"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = watchpost()
        .args(["check", "--config-only"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Configuration OK: 3 monitor(s)"));
}

#[test]
fn check_prints_a_table_and_fails_when_something_is_down() {
    let dir = tempfile::tempdir().unwrap();
    let up = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let up_port = up.local_addr().unwrap().port();
    let down_port = free_port();
    let path = dir.path().join("cfg.toml");
    std::fs::write(
        &path,
        format!(
            "[[monitor]]\nname=\"alive\"\ntype=\"tcp\"\nhost=\"127.0.0.1\"\nport={up_port}\n\n\
             [[monitor]]\nname=\"dead\"\ntype=\"tcp\"\nhost=\"127.0.0.1\"\nport={down_port}\n"
        ),
    )
    .unwrap();

    let out = watchpost()
        .arg("--config")
        .arg(&path)
        .arg("check")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("NAME"), "{stdout}");
    let alive = stdout.lines().find(|l| l.starts_with("alive")).unwrap();
    assert!(alive.contains("UP"), "{alive}");
    let dead = stdout.lines().find(|l| l.starts_with("dead")).unwrap();
    assert!(
        dead.contains("DOWN") && dead.contains("connection refused"),
        "{dead}"
    );
    assert!(stdout.contains("1 of 2 monitor(s) are down"));
    drop(up);
}

#[test]
fn invalid_config_exits_with_code_2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(
        &path,
        "[[monitor]]\nname=\"x\"\ntype=\"http\"\nurl=\"nope\"\n",
    )
    .unwrap();
    let out = watchpost()
        .arg("-c")
        .arg(&path)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid configuration"), "{stderr}");
    assert!(stderr.contains("url `nope`"), "{stderr}");
}

#[test]
fn missing_config_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let out = watchpost()
        .arg("check")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read config file"));
}

#[test]
fn test_notify_without_notifiers_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg.toml");
    std::fs::write(
        &path,
        "[[monitor]]\nname=\"x\"\ntype=\"tcp\"\nhost=\"h\"\nport=1\n",
    )
    .unwrap();
    let out = watchpost()
        .arg("-c")
        .arg(&path)
        .arg("test-notify")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no notifiers"));
}

#[test]
fn version_flag() {
    let out = watchpost().arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}
