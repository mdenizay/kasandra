use miesql_lib::files::{
    join_remote, mode_string, parse_ftp_listing, EntryKind, FileTarget, Protocol,
};
#[cfg(unix)]
use miesql_lib::files::{FileManager, FileSite};

#[test]
fn remote_paths() {
    assert_eq!(join_remote("/", "etc"), "/etc");
    assert_eq!(join_remote("/var/www", "index.html"), "/var/www/index.html");
    assert_eq!(join_remote("/var/www/", "a b"), "/var/www/a b");
    assert_eq!(join_remote("", "rel"), "rel");
}

#[test]
fn permission_strings() {
    assert_eq!(mode_string(0o755), "rwxr-xr-x");
    assert_eq!(mode_string(0o100644), "rw-r--r--");
    assert_eq!(mode_string(0o040700), "rwx------");
    assert_eq!(mode_string(0), "---------");
}

#[test]
fn unix_listing() {
    let lines: Vec<String> = [
        "total 12",
        "drwxr-xr-x   2 www  www      4096 Nov 10 12:00 .",
        "drwxr-xr-x   5 www  www      4096 Nov 10 12:00 ..",
        "drwxr-xr-x   2 www  www      4096 Nov 10 12:00 assets",
        "-rw-r--r--   1 www  www      1234 Jan  3  2025 index.html",
        "lrwxrwxrwx   1 www  www        11 Nov 10 12:00 current -> releases/42",
        "-rw-r--r--   1 www  www         7 Nov 10 12:00 with space.txt",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let entries = parse_ftp_listing("/site", &lines, false);
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["assets", "index.html", "current", "with space.txt"]);
    assert_eq!(entries[0].kind, EntryKind::Dir);
    assert_eq!(entries[0].path, "/site/assets");
    assert_eq!(entries[1].size, 1234);
    assert_eq!(entries[1].permissions.as_deref(), Some("rw-r--r--"));
    assert!(entries[1].modified.is_some());
    assert!(entries[2].link);
    assert_eq!(entries[2].kind, EntryKind::Link);
}

#[test]
fn dos_listing() {
    let lines: Vec<String> = [
        "04-27-25  09:09PM       <DIR>          logs",
        "04-27-25  09:09PM                 2048 web.config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let entries = parse_ftp_listing("/", &lines, false);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].kind, EntryKind::Dir);
    assert_eq!(entries[1].size, 2048);
    assert_eq!(entries[1].path, "/web.config");
}

#[test]
fn mlsd_listing() {
    let lines: Vec<String> = [
        "type=cdir;modify=20251110120000; /site",
        "type=pdir;modify=20251110120000; /",
        "type=dir;modify=20251110120000;perm=flcdmpe; assets",
        "type=file;size=42;modify=20251110120000;perm=adfrw; notes.txt",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let entries = parse_ftp_listing("/site", &lines, true);
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["assets", "notes.txt"]);
    assert_eq!(entries[1].size, 42);
}

#[test]
fn site_targets_deserialise() {
    let target: FileTarget = serde_json::from_value(serde_json::json!({
        "kind": "site",
        "site": {
            "id": "6f1e0a52-6d0e-4a7e-9f43-6d1b8c0a1f11",
            "protocol": "sftp",
            "host": "example.com",
        },
        "password": null,
    }))
    .unwrap();
    match target {
        FileTarget::Site { site, .. } => {
            assert_eq!(site.protocol, Protocol::Sftp);
            assert_eq!(site.effective_port(), 22);
            assert_eq!(site.title(), "example.com");
        }
        _ => panic!("wrong variant"),
    }
}

/// Exercises a real server end to end: list, mkdir, upload a folder, download it back,
/// rename, delete. Runs only when pointed at one, for example
/// `KASANDRA_TEST_SITE='ftp://user:pass@127.0.0.1:2121/'` or
/// `KASANDRA_TEST_SITE='sftp://me@127.0.0.1:2222/tmp/x?key=/path/to/key'`.
// Unix only, like the terminal test: a test binary that links the Tauri runtime does not
// start on Windows CI (STATUS_ENTRYPOINT_NOT_FOUND).
#[cfg(unix)]
#[test]
fn live_round_trip() {
    let Ok(url) = std::env::var("KASANDRA_TEST_SITE") else {
        eprintln!("KASANDRA_TEST_SITE not set; skipping");
        return;
    };
    let (site, password) = site_from_url(&url);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    runtime.block_on(async move {
        let manager = FileManager::default();
        let opened = manager
            .open("s".into(), FileTarget::Site { site, password })
            .await
            .expect("open");
        let base = join_remote(
            &opened.home,
            &format!("kasandra-test-{}", std::process::id()),
        );
        manager.mkdir("s", &base).await.expect("mkdir");

        let scratch = std::env::temp_dir().join(format!("kasandra-files-{}", std::process::id()));
        let source = scratch.join("upload");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(source.join("big.bin"), &big).unwrap();
        std::fs::write(source.join("nested").join("ünïcode name.txt"), "merhaba").unwrap();

        let uploaded = manager
            .upload(&handle, "s", "u1", &source, &base)
            .await
            .expect("upload");
        assert_eq!(uploaded, join_remote(&base, "upload"));

        let listing = manager.list("s", &uploaded).await.expect("list");
        let mut names: Vec<_> = listing.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["big.bin", "nested"]);
        let big_entry = listing.iter().find(|e| e.name == "big.bin").unwrap();
        assert_eq!(big_entry.size, big.len() as u64);

        let back = scratch.join("download");
        manager
            .download(&handle, "s", "d1", &uploaded, true, &back)
            .await
            .expect("download folder");
        assert_eq!(std::fs::read(back.join("big.bin")).unwrap(), big);
        assert_eq!(
            std::fs::read_to_string(back.join("nested").join("ünïcode name.txt")).unwrap(),
            "merhaba"
        );

        let single = scratch.join("single.bin");
        manager
            .download(
                &handle,
                "s",
                "d2",
                &join_remote(&uploaded, "big.bin"),
                false,
                &single,
            )
            .await
            .expect("download file");
        assert_eq!(std::fs::read(&single).unwrap(), big);

        // A cancelled transfer stops and leaves nothing half-written behind.
        manager.cancel("d3");
        let cancelled = manager
            .download(
                &handle,
                "s",
                "d3",
                &join_remote(&uploaded, "big.bin"),
                false,
                &scratch.join("c.bin"),
            )
            .await;
        assert_eq!(cancelled.unwrap_err().code.as_deref(), Some("cancelled"));
        assert!(!scratch.join("c.bin").exists());

        let renamed = join_remote(&base, "renamed");
        manager
            .rename("s", &uploaded, &renamed)
            .await
            .expect("rename");
        manager.delete("s", &base, true).await.expect("delete tree");
        let parent = manager.list("s", &opened.home).await.unwrap();
        assert!(!parent.iter().any(|e| e.path == base));

        manager.close("s").await;
        let _ = std::fs::remove_dir_all(&scratch);
    });
}

#[cfg(unix)]
fn site_from_url(url: &str) -> (FileSite, Option<String>) {
    let (scheme, rest) = url.split_once("://").unwrap();
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, "0"));
    let key = query.strip_prefix("key=").unwrap_or("");
    let site = FileSite {
        id: uuid::Uuid::new_v4(),
        name: String::new(),
        protocol: match scheme {
            "ftp" => Protocol::Ftp,
            "ftps" => Protocol::Ftps,
            _ => Protocol::Sftp,
        },
        host: host.into(),
        port: port.parse().unwrap(),
        username: user.into(),
        key_path: key.into(),
        initial_path: if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        },
        save_password: false,
        active_mode: false,
    };
    (site, (!password.is_empty()).then(|| password.to_string()))
}
