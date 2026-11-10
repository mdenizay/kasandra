//! Remote files over FTP, FTPS and SFTP, behind one interface the file browser tab uses.
//!
//! FTP and FTPS go through suppaftp on rustls, the same TLS stack as the database drivers.
//! FTPS is explicit (`AUTH TLS` on the usual port), and like `sslmode=require` it encrypts
//! without validating the certificate — see `tls.rs`.
//!
//! SFTP does not use an SSH library. The system `ssh` is started with `-s … sftp`, and the
//! SFTP protocol is spoken over its stdin and stdout. That is the same choice the tunnels
//! make, for the same reasons: `~/.ssh/config`, the agent, hardware keys and jump hosts all
//! work, and host keys are checked by OpenSSH against `known_hosts`. A host seen for the
//! first time is accepted and remembered (`accept-new`), because there is no terminal to ask
//! in; a key that has *changed* is still refused.

use crate::error::{DbError, DbResult};
use crate::{ssh, storage, tls};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use suppaftp::list::{ListParser, PosixPexQuery};
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::types::FileType as FtpFileType;
use suppaftp::FtpError;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// FTP servers drop idle control connections, typically after five minutes. A session idle
/// for longer than this is checked with NOOP before use, and reopened if it has gone.
const IDLE_CHECK: Duration = Duration::from_secs(30);
const CHUNK: usize = 256 * 1024;

// MARK: - Sites

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ftp,
    Ftps,
    Sftp,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
        }
    }
}

/// A saved server. The password, when kept, lives in the OS credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSite {
    pub id: Uuid,
    #[serde(default)]
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    /// 0 means the protocol's usual port.
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    /// SFTP only.
    #[serde(default)]
    pub key_path: String,
    /// Where the browser starts; the login directory when empty.
    #[serde(default)]
    pub initial_path: String,
    #[serde(default)]
    pub save_password: bool,
    /// FTP only. Passive is what works behind NAT, so it is the default.
    #[serde(default)]
    pub active_mode: bool,
}

impl FileSite {
    pub fn credential_account(&self) -> String {
        format!("site:{}", self.id)
    }

    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            self.protocol.default_port()
        } else {
            self.port
        }
    }

    pub fn title(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.username.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.username, self.host)
        }
    }
}

/// What a file browser tab connects to.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileTarget {
    /// A site from the site manager, saved or not. A password typed into the dialog wins
    /// over a saved one.
    #[serde(rename_all = "camelCase")]
    Site {
        site: FileSite,
        password: Option<String>,
    },
    /// SFTP to the SSH host of a database connection, with its saved SSH credentials.
    #[serde(rename_all = "camelCase")]
    Profile { profile_id: Uuid },
}

fn resolve(target: &FileTarget) -> DbResult<(FileSite, Option<String>)> {
    match target {
        FileTarget::Site { site, password } => {
            if site.host.trim().is_empty() {
                return Err(DbError::new("Enter a host to connect to."));
            }
            let password = password.clone().filter(|p| !p.is_empty()).or_else(|| {
                site.save_password
                    .then(|| storage::load_password(&site.credential_account()))
                    .flatten()
            });
            let mut site = site.clone();
            site.host = site.host.trim().to_string();
            Ok((site, password))
        }
        FileTarget::Profile { profile_id } => {
            let profile = storage::load_profiles()
                .into_iter()
                .find(|p| p.id == *profile_id)
                .ok_or_else(|| DbError::new("That connection no longer exists."))?;
            if !profile.ssh_enabled || profile.ssh_host.trim().is_empty() {
                return Err(DbError::new(format!(
                    "{} has no SSH host configured.",
                    profile.name
                )));
            }
            let password =
                storage::load_password(&profile.ssh_credential_account()).filter(|p| !p.is_empty());
            let site = FileSite {
                id: profile.id,
                name: String::new(),
                protocol: Protocol::Sftp,
                host: profile.ssh_host.trim().to_string(),
                port: profile.ssh_port,
                username: profile.ssh_username.clone(),
                key_path: profile.ssh_key_path.clone(),
                initial_path: String::new(),
                save_password: false,
                active_mode: false,
            };
            Ok((site, password))
        }
    }
}

// MARK: - Listing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    /// A symbolic link whose target could not be looked at (FTP never says).
    Link,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    /// True for a symbolic link, whatever `kind` it resolved to.
    pub link: bool,
    pub size: u64,
    /// Milliseconds since the epoch.
    pub modified: Option<i64>,
    /// `rwxr-xr-x`, when the server says.
    pub permissions: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSession {
    pub home: String,
    pub title: String,
    pub protocol: Protocol,
}

pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub fn mode_string(mode: u32) -> String {
    const LETTERS: [char; 3] = ['r', 'w', 'x'];
    (0..9)
        .map(|i| {
            if mode & (1 << (8 - i)) != 0 {
                LETTERS[i % 3]
            } else {
                '-'
            }
        })
        .collect()
}

fn millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
        .filter(|ms| *ms > 0)
}

/// Turns FTP listing lines into entries. MLSD is machine-readable and preferred; LIST output
/// is whatever the server's `ls` prints, so both the Unix and the DOS shapes are tried.
pub fn parse_ftp_listing(dir: &str, lines: &[String], mlsd: bool) -> Vec<RemoteEntry> {
    let mut entries = Vec::new();
    for line in lines {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.starts_with("total ") {
            continue;
        }
        let (file, posix) = if mlsd {
            let lower = line.to_ascii_lowercase();
            // The directory itself and its parent, which MLSD lists alongside the content.
            if lower.contains("type=cdir") || lower.contains("type=pdir") {
                continue;
            }
            match ListParser::parse_mlsd(line) {
                Ok(file) => (file, false),
                Err(_) => continue,
            }
        } else {
            match ListParser::parse_posix(line) {
                Ok(file) => (file, true),
                Err(_) => match ListParser::parse_dos(line) {
                    Ok(file) => (file, false),
                    Err(_) => continue,
                },
            }
        };
        let name = file.name().to_string();
        if name == "." || name == ".." || name.is_empty() || name.contains('/') {
            continue;
        }
        let kind = if file.is_directory() {
            EntryKind::Dir
        } else if file.is_symlink() {
            EntryKind::Link
        } else {
            EntryKind::File
        };
        let permissions = posix.then(|| {
            let mut text = String::with_capacity(9);
            for who in [
                PosixPexQuery::Owner,
                PosixPexQuery::Group,
                PosixPexQuery::Others,
            ] {
                text.push(if file.can_read(who) { 'r' } else { '-' });
                text.push(if file.can_write(who) { 'w' } else { '-' });
                text.push(if file.can_execute(who) { 'x' } else { '-' });
            }
            text
        });
        entries.push(RemoteEntry {
            path: join_remote(dir, &name),
            name,
            kind,
            link: file.is_symlink(),
            size: if kind == EntryKind::Dir {
                0
            } else {
                file.size() as u64
            },
            modified: millis(file.modified()),
            permissions,
        });
    }
    entries
}

// MARK: - Sessions

/// The ssh process behind an SFTP session; killed when the session goes.
struct SshProcess {
    child: tokio::process::Child,
}

impl Drop for SshProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

enum Client {
    Ftp {
        ftp: Box<AsyncRustlsFtpStream>,
        mlsd: bool,
    },
    Sftp {
        sftp: Arc<SftpSession>,
        process: SshProcess,
    },
}

struct Session {
    site: FileSite,
    password: Option<String>,
    client: Client,
    last_used: Instant,
}

fn ftp_err(error: FtpError) -> DbError {
    match error {
        FtpError::UnexpectedResponse(response) => {
            let body = String::from_utf8_lossy(&response.body).trim().to_string();
            DbError::new(if body.is_empty() {
                format!("The server refused: {}", response.status.code())
            } else {
                body
            })
        }
        other => DbError::new(other.to_string()),
    }
}

fn sftp_err(error: russh_sftp::client::error::Error) -> DbError {
    DbError::new(error.to_string())
}

fn io_err(error: std::io::Error) -> DbError {
    DbError::new(error.to_string())
}

async fn connect(site: &FileSite, password: Option<&str>) -> DbResult<Client> {
    match site.protocol {
        Protocol::Ftp | Protocol::Ftps => connect_ftp(site, password).await,
        Protocol::Sftp => connect_sftp(site, password).await,
    }
}

async fn connect_ftp(site: &FileSite, password: Option<&str>) -> DbResult<Client> {
    let port = site.effective_port();
    let address = tokio::net::lookup_host((site.host.as_str(), port))
        .await
        .map_err(|e| DbError::new(format!("Could not resolve {}: {e}", site.host)))?
        .next()
        .ok_or_else(|| DbError::new(format!("Could not resolve {}.", site.host)))?;

    let mut ftp = AsyncRustlsFtpStream::connect_timeout(address, CONNECT_TIMEOUT)
        .await
        .map_err(|e| {
            DbError::new(format!("Could not connect to {}:{port}.", site.host))
                .with_detail(e.to_string())
        })?;

    if site.protocol == Protocol::Ftps {
        let connector = AsyncRustlsConnector::from(suppaftp::tokio_rustls::TlsConnector::from(
            tls::client_config()?,
        ));
        ftp = ftp.into_secure(connector, &site.host).await.map_err(|e| {
            DbError::new("The server did not accept FTPS (AUTH TLS).").with_detail(e.to_string())
        })?;
    }

    // A server behind NAT often announces its private address for passive data
    // connections; using the control connection's address instead is what every FTP client
    // ends up doing.
    ftp.set_passive_nat_workaround(true);
    if site.active_mode {
        ftp = ftp.active_mode(Duration::from_secs(30));
    }

    let (user, pass) = if site.username.trim().is_empty() {
        (
            "anonymous".to_string(),
            password.unwrap_or("anonymous@").to_string(),
        )
    } else {
        (
            site.username.trim().to_string(),
            password.unwrap_or_default().to_string(),
        )
    };
    ftp.login(user.as_str(), pass.as_str()).await.map_err(|e| {
        DbError::new("The FTP server refused the login.").with_detail(ftp_err(e).message)
    })?;

    // Without this, servers that support UTF-8 names may still send them in a legacy code page.
    let _ = ftp.opts("UTF8", Some("ON")).await;
    ftp.transfer_type(FtpFileType::Binary)
        .await
        .map_err(ftp_err)?;
    let mlsd = ftp
        .feat()
        .await
        .map(|features| features.keys().any(|k| k.eq_ignore_ascii_case("MLSD")))
        .unwrap_or(false);

    Ok(Client::Ftp {
        ftp: Box::new(ftp),
        mlsd,
    })
}

async fn connect_sftp(site: &FileSite, password: Option<&str>) -> DbResult<Client> {
    if ssh::which_ssh().is_none() {
        return Err(
            DbError::new("No ssh client was found on this computer.").with_detail(
                "SFTP runs through the system ssh command. macOS and Linux include one; on \
             Windows, install the OpenSSH Client from Optional Features.",
            ),
        );
    }

    let mut command = tokio::process::Command::new("ssh");
    command
        .arg("-T")
        .args(["-o", "ServerAliveInterval=30"])
        .args([
            "-o",
            &format!("ConnectTimeout={}", CONNECT_TIMEOUT.as_secs()),
        ])
        .args(["-o", "StrictHostKeyChecking=accept-new"]);
    let port = site.effective_port();
    if port != 22 {
        command.arg("-p").arg(port.to_string());
    }
    if !site.key_path.trim().is_empty() {
        command
            .arg("-i")
            .arg(site.key_path.trim())
            .args(["-o", "IdentitiesOnly=yes"]);
    }

    let askpass = match password.filter(|p| !p.is_empty()) {
        Some(password) => {
            let path = ssh::write_askpass_helper(password)?;
            command
                .env("SSH_ASKPASS", &path)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", ":0");
            Some(path)
        }
        None => {
            command.args(["-o", "BatchMode=yes"]);
            None
        }
    };

    let destination = if site.username.trim().is_empty() {
        site.host.clone()
    } else {
        format!("{}@{}", site.username.trim(), site.host)
    };
    command.arg("-s").arg(&destination).arg("sftp");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let spawned = command.spawn();
    let remove_askpass = || {
        if let Some(path) = &askpass {
            let _ = std::fs::remove_file(path);
        }
    };
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            remove_askpass();
            return Err(DbError::new(format!("Could not start ssh: {e}")));
        }
    };

    // Collected in the background: if ssh fails, what it printed is the explanation.
    let stderr_text = Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let sink = stderr_text.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut text = sink.lock().unwrap();
                if text.len() < 8192 {
                    text.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
    }

    let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
        remove_askpass();
        return Err(DbError::new("Could not talk to ssh."));
    };
    let started = tokio::time::timeout(
        CONNECT_TIMEOUT + Duration::from_secs(10),
        SftpSession::new(tokio::io::join(stdout, stdin)),
    )
    .await;
    // Authentication is over either way, so the password has done its job.
    remove_askpass();

    match started {
        Ok(Ok(sftp)) => Ok(Client::Sftp {
            sftp: Arc::new(sftp),
            process: SshProcess { child },
        }),
        failed => {
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = child.start_kill();
            // Give the stderr reader a moment to drain what ssh wrote on its way out.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let printed = stderr_text.lock().unwrap().trim().to_string();
            let detail = match failed {
                Ok(Err(e)) if printed.is_empty() => e.to_string(),
                Err(_) if printed.is_empty() => "No answer from the server.".to_string(),
                _ => printed,
            };
            Err(DbError::new(format!("Could not open SFTP to {destination}.")).with_detail(detail))
        }
    }
}

impl Session {
    /// Reopens a connection the server has dropped, so an idle tab keeps working.
    async fn ensure_alive(&mut self) -> DbResult<()> {
        let alive = match &mut self.client {
            Client::Ftp { ftp, .. } => {
                self.last_used.elapsed() < IDLE_CHECK || ftp.noop().await.is_ok()
            }
            Client::Sftp { process, .. } => matches!(process.child.try_wait(), Ok(None)),
        };
        if !alive {
            self.client = connect(&self.site, self.password.as_deref()).await?;
        }
        self.last_used = Instant::now();
        Ok(())
    }

    async fn home(&mut self) -> DbResult<String> {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.pwd().await.map_err(ftp_err),
            Client::Sftp { sftp, .. } => sftp.canonicalize(".").await.map_err(sftp_err),
        }
    }

    async fn list(&mut self, path: &str) -> DbResult<Vec<RemoteEntry>> {
        match &mut self.client {
            Client::Ftp { ftp, mlsd } => {
                // Changing into the directory and listing "here" survives names with
                // spaces, which a path argument to LIST does not on every server.
                ftp.cwd(path).await.map_err(ftp_err)?;
                if *mlsd {
                    match ftp.mlsd(None).await {
                        Ok(lines) => return Ok(parse_ftp_listing(path, &lines, true)),
                        // Advertised but broken happens; LIST is the fallback.
                        Err(_) => *mlsd = false,
                    }
                }
                let lines = ftp.list(None).await.map_err(ftp_err)?;
                Ok(parse_ftp_listing(path, &lines, false))
            }
            Client::Sftp { sftp, .. } => {
                let sftp = sftp.clone();
                sftp_list(&sftp, path).await
            }
        }
    }

    async fn size(&mut self, path: &str) -> u64 {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.size(path).await.map(|s| s as u64).unwrap_or(0),
            Client::Sftp { sftp, .. } => sftp
                .metadata(path)
                .await
                .ok()
                .and_then(|m| m.size)
                .unwrap_or(0),
        }
    }

    /// Every entry under `root`, parents before children. Links are not followed, so a
    /// recursive delete removes the link rather than what it points at.
    async fn walk(&mut self, root: &str) -> DbResult<Vec<RemoteEntry>> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_string()];
        while let Some(dir) = pending.pop() {
            for entry in self.list(&dir).await? {
                if entry.kind == EntryKind::Dir && !entry.link {
                    pending.push(entry.path.clone());
                }
                found.push(entry);
            }
        }
        // Breadth of the stack aside, sort by path so a parent always precedes its children.
        found.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(found)
    }

    async fn mkdir(&mut self, path: &str) -> DbResult<()> {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.mkdir(path).await.map_err(ftp_err),
            Client::Sftp { sftp, .. } => sftp.create_dir(path).await.map_err(sftp_err),
        }
    }

    async fn rename(&mut self, from: &str, to: &str) -> DbResult<()> {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.rename(from, to).await.map_err(ftp_err),
            Client::Sftp { sftp, .. } => sftp.rename(from, to).await.map_err(sftp_err),
        }
    }

    async fn remove_file(&mut self, path: &str) -> DbResult<()> {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.rm(path).await.map_err(ftp_err),
            Client::Sftp { sftp, .. } => sftp.remove_file(path).await.map_err(sftp_err),
        }
    }

    async fn remove_dir(&mut self, path: &str) -> DbResult<()> {
        match &mut self.client {
            Client::Ftp { ftp, .. } => ftp.rmdir(path).await.map_err(ftp_err),
            Client::Sftp { sftp, .. } => sftp.remove_dir(path).await.map_err(sftp_err),
        }
    }
}

async fn sftp_list(sftp: &SftpSession, path: &str) -> DbResult<Vec<RemoteEntry>> {
    let listing = sftp.read_dir(path).await.map_err(sftp_err)?;
    let mut entries = Vec::new();
    for entry in listing {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata();
        let full = join_remote(path, &name);
        let link = meta.file_type().is_symlink();
        let mut kind = if meta.file_type().is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        if link {
            // Follow it once, so a link to a directory opens like one.
            kind = match sftp.metadata(full.as_str()).await {
                Ok(target) if target.file_type().is_dir() => EntryKind::Dir,
                Ok(_) => EntryKind::File,
                Err(_) => EntryKind::Link,
            };
        }
        entries.push(RemoteEntry {
            name,
            path: full,
            kind,
            link,
            size: if kind == EntryKind::Dir {
                0
            } else {
                meta.size.unwrap_or(0)
            },
            modified: meta.mtime.map(|t| i64::from(t) * 1000),
            permissions: meta.permissions.map(mode_string),
        });
    }
    Ok(entries)
}

// MARK: - Transfers

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent<'a> {
    transfer_id: &'a str,
    done: u64,
    total: u64,
    file: &'a str,
}

struct Progress<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    id: &'a str,
    cancelled: &'a std::sync::Mutex<HashSet<String>>,
    done: u64,
    total: u64,
    file: String,
    last_emit: Instant,
}

impl<R: Runtime> Progress<'_, R> {
    fn advance(&mut self, bytes: usize) -> DbResult<()> {
        self.done += bytes as u64;
        if self.cancelled.lock().unwrap().contains(self.id) {
            return Err(DbError::with_code("Transfer cancelled.", "cancelled"));
        }
        if self.last_emit.elapsed() > Duration::from_millis(120) {
            self.emit();
        }
        Ok(())
    }

    fn emit(&mut self) {
        self.last_emit = Instant::now();
        let _ = self.app.emit(
            "files:progress",
            ProgressEvent {
                transfer_id: self.id,
                done: self.done,
                total: self.total.max(self.done),
                file: &self.file,
            },
        );
    }
}

async fn pump<Rd, Wr, R>(
    reader: &mut Rd,
    writer: &mut Wr,
    progress: &mut Progress<'_, R>,
) -> DbResult<()>
where
    Rd: AsyncRead + Unpin,
    Wr: AsyncWrite + Unpin,
    R: Runtime,
{
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).await.map_err(io_err)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await.map_err(io_err)?;
        progress.advance(n)?;
    }
    writer.flush().await.map_err(io_err)
}

/// One file to copy, and how big it is for the progress bar.
struct Item {
    remote: String,
    local: PathBuf,
    size: u64,
}

fn local_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Walks a local folder for upload: the remote directories to create, then the files.
fn plan_local(root: &Path, remote_root: &str) -> DbResult<(Vec<String>, Vec<Item>)> {
    let mut dirs = vec![remote_root.to_string()];
    let mut files = Vec::new();
    let mut pending = vec![(root.to_path_buf(), remote_root.to_string())];
    while let Some((local, remote)) = pending.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&local)
            .map_err(|e| DbError::new(format!("Could not read {}: {e}", local.display())))?
            .filter_map(Result::ok)
            .collect();
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().to_string();
            let remote_child = join_remote(&remote, &name);
            let meta = match std::fs::metadata(child.path()) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if meta.is_dir() {
                dirs.push(remote_child.clone());
                pending.push((child.path(), remote_child));
            } else if meta.is_file() {
                files.push(Item {
                    remote: remote_child,
                    local: child.path(),
                    size: meta.len(),
                });
            }
        }
    }
    dirs.sort();
    Ok((dirs, files))
}

#[derive(Default)]
pub struct FileManager {
    sessions: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
    cancelled: std::sync::Mutex<HashSet<String>>,
}

impl FileManager {
    pub async fn open(&self, id: String, target: FileTarget) -> DbResult<OpenedSession> {
        let (site, password) = resolve(&target)?;
        let client = connect(&site, password.as_deref()).await?;
        let mut session = Session {
            site,
            password,
            client,
            last_used: Instant::now(),
        };
        let home = if session.site.initial_path.trim().is_empty() {
            session.home().await.unwrap_or_else(|_| "/".to_string())
        } else {
            session.site.initial_path.trim().to_string()
        };
        let opened = OpenedSession {
            home,
            title: session.site.title(),
            protocol: session.site.protocol,
        };
        self.sessions
            .lock()
            .await
            .insert(id, Arc::new(Mutex::new(session)));
        Ok(opened)
    }

    async fn session(&self, id: &str) -> DbResult<Arc<Mutex<Session>>> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| DbError::new("This file session has been closed."))
    }

    pub async fn list(&self, id: &str, path: &str) -> DbResult<Vec<RemoteEntry>> {
        let session = self.session(id).await?;
        let mut session = session.lock().await;
        session.ensure_alive().await?;
        session.list(path).await
    }

    pub async fn mkdir(&self, id: &str, path: &str) -> DbResult<()> {
        let session = self.session(id).await?;
        let mut session = session.lock().await;
        session.ensure_alive().await?;
        session.mkdir(path).await
    }

    pub async fn rename(&self, id: &str, from: &str, to: &str) -> DbResult<()> {
        let session = self.session(id).await?;
        let mut session = session.lock().await;
        session.ensure_alive().await?;
        session.rename(from, to).await
    }

    pub async fn delete(&self, id: &str, path: &str, is_dir: bool) -> DbResult<()> {
        let session = self.session(id).await?;
        let mut session = session.lock().await;
        session.ensure_alive().await?;
        if !is_dir {
            return session.remove_file(path).await;
        }
        // Deepest first, so every directory is empty by the time it is removed.
        let entries = session.walk(path).await?;
        for entry in entries.iter().rev() {
            if entry.kind == EntryKind::Dir && !entry.link {
                session.remove_dir(&entry.path).await?;
            } else {
                session.remove_file(&entry.path).await?;
            }
        }
        session.remove_dir(path).await
    }

    pub fn cancel(&self, transfer_id: &str) {
        self.cancelled
            .lock()
            .unwrap()
            .insert(transfer_id.to_string());
    }

    /// Copies a remote file, or a folder and everything in it, to `local`.
    pub async fn download<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: &str,
        transfer_id: &str,
        remote: &str,
        is_dir: bool,
        local: &Path,
    ) -> DbResult<()> {
        let result = self
            .download_inner(app, id, transfer_id, remote, is_dir, local)
            .await;
        self.cancelled.lock().unwrap().remove(transfer_id);
        result
    }

    async fn download_inner<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: &str,
        transfer_id: &str,
        remote: &str,
        is_dir: bool,
        local: &Path,
    ) -> DbResult<()> {
        let session = self.session(id).await?;
        let mut guard = session.lock().await;
        guard.ensure_alive().await?;

        let mut dirs = Vec::new();
        let mut items = Vec::new();
        if is_dir {
            dirs.push(local.to_path_buf());
            for entry in guard.walk(remote).await? {
                let relative = entry.path[remote.len()..].trim_start_matches('/');
                let target = relative
                    .split('/')
                    .fold(local.to_path_buf(), |path, part| path.join(part));
                if entry.kind == EntryKind::Dir && !entry.link {
                    dirs.push(target);
                } else if entry.kind != EntryKind::Dir {
                    items.push(Item {
                        remote: entry.path,
                        local: target,
                        size: entry.size,
                    });
                }
            }
        } else {
            let size = guard.size(remote).await;
            items.push(Item {
                remote: remote.to_string(),
                local: local.to_path_buf(),
                size,
            });
        }
        for dir in &dirs {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| DbError::new(format!("Could not create {}: {e}", dir.display())))?;
        }

        let mut progress = Progress {
            app,
            id: transfer_id,
            cancelled: &self.cancelled,
            done: 0,
            total: items.iter().map(|i| i.size).sum(),
            file: String::new(),
            last_emit: Instant::now(),
        };
        progress.emit();

        // SFTP requests can run alongside each other, so the session is released and the
        // tab stays browsable while a download runs. FTP has one control connection.
        let sftp = match &guard.client {
            Client::Sftp { sftp, .. } => Some(sftp.clone()),
            Client::Ftp { .. } => None,
        };
        if let Some(sftp) = sftp {
            drop(guard);
            for item in &items {
                progress.file = local_name(&item.remote);
                let mut source = sftp.open(item.remote.as_str()).await.map_err(sftp_err)?;
                let mut target = tokio::fs::File::create(&item.local).await.map_err(|e| {
                    DbError::new(format!("Could not create {}: {e}", item.local.display()))
                })?;
                if let Err(e) = pump(&mut source, &mut target, &mut progress).await {
                    drop(target);
                    let _ = tokio::fs::remove_file(&item.local).await;
                    return Err(e);
                }
            }
        } else {
            let Client::Ftp { ftp, .. } = &mut guard.client else {
                unreachable!()
            };
            for item in &items {
                progress.file = local_name(&item.remote);
                let mut source = ftp.retr_as_stream(&item.remote).await.map_err(ftp_err)?;
                let mut target = tokio::fs::File::create(&item.local).await.map_err(|e| {
                    DbError::new(format!("Could not create {}: {e}", item.local.display()))
                })?;
                match pump(&mut source, &mut target, &mut progress).await {
                    Ok(()) => source.finish().await.map_err(ftp_err)?,
                    Err(e) => {
                        let _ = ftp.abort(source).await;
                        drop(target);
                        let _ = tokio::fs::remove_file(&item.local).await;
                        return Err(e);
                    }
                }
            }
        }
        progress.emit();
        Ok(())
    }

    /// Copies a local file or folder into `remote_dir`; returns the remote path it made.
    pub async fn upload<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: &str,
        transfer_id: &str,
        local: &Path,
        remote_dir: &str,
    ) -> DbResult<String> {
        let result = self
            .upload_inner(app, id, transfer_id, local, remote_dir)
            .await;
        self.cancelled.lock().unwrap().remove(transfer_id);
        result
    }

    async fn upload_inner<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        id: &str,
        transfer_id: &str,
        local: &Path,
        remote_dir: &str,
    ) -> DbResult<String> {
        let meta = std::fs::metadata(local)
            .map_err(|e| DbError::new(format!("Could not read {}: {e}", local.display())))?;
        let name = local
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| DbError::new("That path has no file name."))?;
        let remote_root = join_remote(remote_dir, &name);
        let (dirs, items) = if meta.is_dir() {
            plan_local(local, &remote_root)?
        } else {
            (
                Vec::new(),
                vec![Item {
                    remote: remote_root.clone(),
                    local: local.to_path_buf(),
                    size: meta.len(),
                }],
            )
        };

        let session = self.session(id).await?;
        let mut guard = session.lock().await;
        guard.ensure_alive().await?;
        for dir in &dirs {
            // Already there is fine: uploading a folder again merges into it.
            let _ = guard.mkdir(dir).await;
        }

        let mut progress = Progress {
            app,
            id: transfer_id,
            cancelled: &self.cancelled,
            done: 0,
            total: items.iter().map(|i| i.size).sum(),
            file: String::new(),
            last_emit: Instant::now(),
        };
        progress.emit();

        let sftp = match &guard.client {
            Client::Sftp { sftp, .. } => Some(sftp.clone()),
            Client::Ftp { .. } => None,
        };
        match sftp {
            Some(sftp) => {
                drop(guard);
                for item in &items {
                    progress.file = local_name(&item.local.to_string_lossy().replace('\\', "/"));
                    let mut source = tokio::fs::File::open(&item.local).await.map_err(io_err)?;
                    let mut target = sftp.create(item.remote.as_str()).await.map_err(sftp_err)?;
                    let copied = pump(&mut source, &mut target, &mut progress).await;
                    let closed = target.shutdown().await.map_err(io_err);
                    if let Err(e) = copied.and(closed) {
                        let _ = sftp.remove_file(item.remote.as_str()).await;
                        return Err(e);
                    }
                }
            }
            None => {
                let Client::Ftp { ftp, .. } = &mut guard.client else {
                    unreachable!()
                };
                for item in &items {
                    progress.file = local_name(&item.local.to_string_lossy().replace('\\', "/"));
                    let mut source = tokio::fs::File::open(&item.local).await.map_err(io_err)?;
                    let mut target = ftp.put_with_stream(&item.remote).await.map_err(ftp_err)?;
                    match pump(&mut source, &mut target, &mut progress).await {
                        Ok(()) => target.finish().await.map_err(ftp_err)?,
                        Err(e) => {
                            let _ = ftp.abort(target).await;
                            let _ = ftp.rm(&item.remote).await;
                            return Err(e);
                        }
                    }
                }
            }
        }
        progress.emit();
        Ok(remote_root)
    }

    pub async fn close(&self, id: &str) {
        let session = self.sessions.lock().await.remove(id);
        if let Some(session) = session {
            let mut session = session.lock().await;
            if let Client::Ftp { ftp, .. } = &mut session.client {
                let _ = tokio::time::timeout(Duration::from_secs(2), ftp.quit()).await;
            }
        }
    }

    /// Called as the window goes away; dropping the sessions kills their ssh processes.
    pub fn close_all(&self) {
        if let Ok(mut sessions) = self.sessions.try_lock() {
            sessions.clear();
        }
    }
}
