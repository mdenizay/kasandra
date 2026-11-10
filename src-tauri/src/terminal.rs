//! Terminal tabs: a real pseudo-terminal per tab, streamed to the webview.
//!
//! A pty rather than a pipe is what makes vim, htop, less and tmux behave, and what lets
//! ssh ask for a host key confirmation or a password the way it would in any terminal.
//! portable-pty gives the same API over ConPTY on Windows, where the child is attached to
//! the pseudo console and so never opens a console window of its own.
//!
//! SSH goes through the system client, for the same reasons as the tunnels in `ssh.rs`:
//! `~/.ssh/config`, the agent, hardware keys and known_hosts all just work.
//!
//! Output is forwarded as base64. A pty emits bytes, and a read can end in the middle of
//! a UTF-8 or escape sequence; decoding here would corrupt exactly the multi-byte output
//! people notice. xterm.js receives the bytes and does its own decoding.

use crate::error::{DbError, DbResult};
use crate::storage;
use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

/// What a terminal tab connects to.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalTarget {
    /// The user's own shell on this computer.
    Local,
    /// The SSH host configured on a saved connection.
    #[serde(rename_all = "camelCase")]
    Profile { profile_id: Uuid },
    /// Any host, typed in by hand.
    #[serde(rename_all = "camelCase")]
    Host {
        host: String,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        username: String,
        #[serde(default)]
        key_path: String,
    },
}

#[derive(Clone, Serialize)]
struct DataEvent {
    id: String,
    /// base64 of the raw bytes; see the module note.
    data: String,
}

#[derive(Clone, Serialize)]
struct ExitEvent {
    id: String,
    code: u32,
}

/// Only the login is answered automatically; a prompt later in the session belongs to
/// something else (sudo, another ssh hop) and is the user's to answer.
const PASSWORD_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

struct Session {
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// How a terminal is started, before it has a pty.
struct Launch {
    command: CommandBuilder,
    title: String,
    /// A saved SSH password, typed once when ssh asks for it. SSH_ASKPASS would be tidier
    /// but ssh routes every prompt through it, host key confirmations included, so the
    /// first visit to a new server would answer "yes/no" with the password.
    password: Option<String>,
}

/// True when a chunk of output ends at ssh's password prompt. Key passphrases and sudo
/// prompts are deliberately not matched: the saved secret is the login password only.
pub fn is_password_prompt(chunk: &[u8]) -> bool {
    let text = String::from_utf8_lossy(chunk).to_lowercase();
    let tail = text.trim_end();
    tail.ends_with("password:") && !tail.contains("password for")
}

#[derive(Default)]
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, Session>>,
}

/// The ssh arguments for a terminal, without the program itself. Public for the tests.
pub fn ssh_args(host: &str, port: u16, username: &str, key_path: &str) -> Vec<String> {
    let mut args = vec![
        // A terminal is the point, even when ssh's own heuristics would decide otherwise.
        "-t".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
    ];
    if port != 22 {
        args.push("-p".to_string());
        args.push(port.to_string());
    }
    if !key_path.trim().is_empty() {
        args.push("-i".to_string());
        args.push(key_path.trim().to_string());
    }
    let host = host.trim();
    args.push(if username.trim().is_empty() {
        host.to_string()
    } else {
        format!("{}@{}", username.trim(), host)
    });
    args
}

/// Splits `user@host:port` into its parts. Anything missing comes back empty or None, so
/// ssh_config can still fill it in.
pub fn parse_destination(input: &str) -> (String, String, Option<u16>) {
    let input = input.trim().trim_start_matches("ssh://");
    let (user, rest) = match input.rsplit_once('@') {
        Some((user, rest)) => (user.to_string(), rest),
        None => (String::new(), input),
    };
    // A bracketed IPv6 address keeps its colons; otherwise the last colon is the port.
    if let Some(stripped) = rest.strip_prefix('[') {
        if let Some((host, tail)) = stripped.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
            return (user, host.to_string(), port);
        }
    }
    match rest.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (user, host.to_string(), Some(port)),
            Err(_) => (user, rest.to_string(), None),
        },
        _ => (user, rest.to_string(), None),
    }
}

impl TerminalManager {
    /// Starts the session and returns a title for its tab.
    pub fn open<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: String,
        target: TerminalTarget,
        cols: u16,
        rows: u16,
    ) -> DbResult<String> {
        let Launch {
            mut command,
            title,
            mut password,
        } = build_command(&target)?;
        // xterm-256color is what xterm.js emulates; claiming anything else invites escape
        // sequences the frontend would render as garbage.
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| {
                DbError::new("Could not open a terminal device.").with_detail(e.to_string())
            })?;

        let mut child = pair.slave.spawn_command(command).map_err(|e| {
            DbError::new("Could not start the terminal session.").with_detail(e.to_string())
        })?;
        // The slave handle keeps a descriptor open; while it lives the reader never sees
        // EOF after the child exits, and the tab would look alive forever.
        drop(pair.slave);

        let killer = child.clone_killer();
        let writer: SharedWriter =
            Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| {
                DbError::new("Could not write to the terminal device.").with_detail(e.to_string())
            })?));
        let mut reader = pair.master.try_clone_reader().map_err(|e| {
            DbError::new("Could not read from the terminal device.").with_detail(e.to_string())
        })?;

        // Reading a pty blocks, so it gets a thread rather than an async task that would
        // pin a runtime worker for the life of the session.
        {
            let app = app.clone();
            let id = id.clone();
            let writer = writer.clone();
            let started = std::time::Instant::now();
            std::thread::spawn(move || {
                let engine = base64::engine::general_purpose::STANDARD;
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            // Offered once only: a second prompt means it was wrong, and
                            // the person at the keyboard should take over.
                            if password.is_some() && started.elapsed() > PASSWORD_WINDOW {
                                password = None;
                            }
                            if password.is_some() && is_password_prompt(&buffer[..n]) {
                                if let (Some(secret), Ok(mut w)) = (password.take(), writer.lock())
                                {
                                    let _ = w.write_all(format!("{secret}\r").as_bytes());
                                    let _ = w.flush();
                                }
                            }
                            let event = DataEvent {
                                id: id.clone(),
                                data: engine.encode(&buffer[..n]),
                            };
                            if app.emit("terminal:data", event).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }

        // A second thread reaps the child, so the tab can say how it ended rather than
        // just going quiet.
        {
            let app = app.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
                if let Some(manager) = app.try_state::<TerminalManager>() {
                    if let Ok(mut sessions) = manager.sessions.lock() {
                        sessions.remove(&id);
                    }
                }
                let _ = app.emit("terminal:exit", ExitEvent { id, code });
            });
        }

        self.lock()?.insert(
            id,
            Session {
                writer,
                master: pair.master,
                killer,
            },
        );
        Ok(title)
    }

    pub fn write(&self, id: &str, data: &str) -> DbResult<()> {
        let mut sessions = self.lock()?;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| DbError::new("That terminal session is no longer open."))?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| DbError::new("The terminal is in an inconsistent state."))?;
        writer
            .write_all(data.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|e| {
                DbError::new("Could not send input to the terminal.").with_detail(e.to_string())
            })
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> DbResult<()> {
        let sessions = self.lock()?;
        // Resizing a tab that has just closed is a race, not an error worth showing.
        let Some(session) = sessions.get(id) else {
            return Ok(());
        };
        session
            .master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DbError::new("Could not resize the terminal.").with_detail(e.to_string()))
    }

    pub fn close(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(mut session) = sessions.remove(id) {
                let _ = session.killer.kill();
            }
        }
    }

    /// Ends every session, so no ssh process outlives the window.
    pub fn close_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (_, mut session) in sessions.drain() {
                let _ = session.killer.kill();
            }
        }
    }

    fn lock(&self) -> DbResult<std::sync::MutexGuard<'_, HashMap<String, Session>>> {
        self.sessions
            .lock()
            .map_err(|_| DbError::new("The terminal session list is in an inconsistent state."))
    }
}

fn build_command(target: &TerminalTarget) -> DbResult<Launch> {
    match target {
        TerminalTarget::Local => {
            // The login shell on unix, ComSpec on Windows: what "a terminal" means there.
            let mut command = CommandBuilder::new_default_prog();
            if let Some(home) = dirs::home_dir() {
                command.cwd(home);
            }
            Ok(Launch {
                command,
                title: "Terminal".to_string(),
                password: None,
            })
        }
        TerminalTarget::Profile { profile_id } => {
            let profile = storage::load_profiles()
                .into_iter()
                .find(|p| p.id == *profile_id)
                .ok_or_else(|| DbError::new("That connection no longer exists."))?;
            if !profile.ssh_enabled || profile.ssh_host.trim().is_empty() {
                return Err(DbError::new(format!(
                    "{} has no SSH host configured.",
                    profile.name
                ))
                .with_detail("Set one on the SSH tab of the connection, or open an SSH session by address."));
            }
            let args = ssh_args(
                &profile.ssh_host,
                profile.ssh_port,
                &profile.ssh_username,
                &profile.ssh_key_path,
            );
            let command = ssh_command(&args)?;
            let password =
                storage::load_password(&profile.ssh_credential_account()).filter(|p| !p.is_empty());
            let title = if profile.ssh_username.is_empty() {
                profile.ssh_host.clone()
            } else {
                format!("{}@{}", profile.ssh_username, profile.ssh_host)
            };
            Ok(Launch {
                command,
                title,
                password,
            })
        }
        TerminalTarget::Host {
            host,
            port,
            username,
            key_path,
        } => {
            let (parsed_user, parsed_host, parsed_port) = parse_destination(host);
            if parsed_host.is_empty() {
                return Err(DbError::new("Enter a host to connect to."));
            }
            let username = if username.trim().is_empty() {
                parsed_user
            } else {
                username.trim().to_string()
            };
            let port = port.or(parsed_port).unwrap_or(22);
            let args = ssh_args(&parsed_host, port, &username, key_path);
            let title = args.last().cloned().unwrap_or_default();
            Ok(Launch {
                command: ssh_command(&args)?,
                title,
                password: None,
            })
        }
    }
}

fn ssh_command(args: &[String]) -> DbResult<CommandBuilder> {
    let program = crate::ssh::which_ssh().ok_or_else(|| {
        DbError::new("No ssh client was found on this computer.").with_detail(
            "Kasandra opens SSH sessions with the system ssh command. macOS and Linux include \
             one; on Windows, install the OpenSSH Client from Optional Features.",
        )
    })?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    if let Some(home) = dirs::home_dir() {
        command.cwd(home);
    }
    Ok(command)
}
