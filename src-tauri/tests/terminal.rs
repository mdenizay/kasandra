use miesql_lib::terminal::{parse_destination, ssh_args};

#[test]
fn destination_parsing() {
    assert_eq!(
        parse_destination("root@example.com"),
        ("root".into(), "example.com".into(), None)
    );
    assert_eq!(
        parse_destination("deploy@10.0.0.5:2222"),
        ("deploy".into(), "10.0.0.5".into(), Some(2222))
    );
    assert_eq!(
        parse_destination("myalias"),
        ("".into(), "myalias".into(), None)
    );
    assert_eq!(
        parse_destination("ssh://u@h:22"),
        ("u".into(), "h".into(), Some(22))
    );
    assert_eq!(
        parse_destination("me@[::1]:2200"),
        ("me".into(), "::1".into(), Some(2200))
    );
    assert_eq!(
        parse_destination("fe80::1"),
        ("".into(), "fe80::1".into(), None)
    );
}

#[test]
fn ssh_arguments() {
    assert_eq!(
        ssh_args("db.internal", 22, "", ""),
        vec!["-t", "-o", "ServerAliveInterval=30", "db.internal"]
    );
    assert_eq!(
        ssh_args("h", 2222, "u", "~/.ssh/id_ed25519"),
        vec![
            "-t",
            "-o",
            "ServerAliveInterval=30",
            "-p",
            "2222",
            "-i",
            "~/.ssh/id_ed25519",
            "u@h"
        ]
    );
}

#[test]
fn password_prompts() {
    use miesql_lib::terminal::is_password_prompt;
    assert!(is_password_prompt(b"root@10.0.0.5's password: "));
    assert!(is_password_prompt(b"(deploy@host) Password:"));
    assert!(!is_password_prompt(
        b"Enter passphrase for key '/home/u/.ssh/id_ed25519': "
    ));
    assert!(!is_password_prompt(b"[sudo] password for deniz: "));
    assert!(!is_password_prompt(
        b"Are you sure you want to continue connecting (yes/no/[fingerprint])? "
    ));
    assert!(!is_password_prompt(b"Last login: Mon Nov 10\r\n$ "));
}

/// Opens a real pty with the user's shell, types into it and reads the answer back
/// through the same events the webview listens to.
#[cfg(unix)]
#[test]
fn local_shell_round_trip() {
    use miesql_lib::terminal::{TerminalManager, TerminalTarget};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::{Listener, Manager};

    let app = tauri::test::mock_app();
    app.manage(TerminalManager::default());
    let handle = app.handle().clone();

    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let exited = Arc::new(Mutex::new(None::<u32>));
    {
        let output = output.clone();
        handle.listen("terminal:data", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(payload["data"].as_str().unwrap())
                .unwrap();
            output.lock().unwrap().extend(bytes);
        });
        let exited = exited.clone();
        handle.listen("terminal:exit", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            *exited.lock().unwrap() = payload["code"].as_u64().map(|c| c as u32);
        });
    }

    let manager = handle.state::<TerminalManager>();
    manager
        .open(&handle, "t1".into(), TerminalTarget::Local, 80, 24)
        .expect("open a local shell");
    manager.resize("t1", 100, 30).unwrap();
    manager.write("t1", "echo kasandra-$((40+2))\r").unwrap();

    let wait_for = |done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        done()
    };
    assert!(
        wait_for(&|| String::from_utf8_lossy(&output.lock().unwrap()).contains("kasandra-42")),
        "shell output never arrived: {:?}",
        String::from_utf8_lossy(&output.lock().unwrap())
    );

    manager.write("t1", "exit 3\r").unwrap();
    assert!(
        wait_for(&|| exited.lock().unwrap().is_some()),
        "no exit event"
    );
    assert_eq!(*exited.lock().unwrap(), Some(3));
    // The session is gone once the shell has exited.
    assert!(manager.write("t1", "x").is_err());
}
