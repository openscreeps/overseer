//! Tests of the native storage. They use a temporary directory and an in-memory credential
//! store, never the configuration directory or the OS credential store of the user.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use super::{
    APP_IDENTIFIER, PROFILES_FILE, Storage, deleted, found, memory, secrets_error, service,
};
use crate::Error;
use crate::profiles::{OFFICIAL_URL, Profiles, Server, ServerKind};

/// A temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("overseer-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Self(dir)
    }

    /// A storage in this directory, with the secrets in memory under `service`.
    fn storage(&self, service: &str) -> Storage {
        memory::install();
        Storage::at(self.0.clone(), service.to_owned())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private(name: &str) -> Server {
    Server::new(
        name.to_owned(),
        format!("http://{name}.example/"),
        ServerKind::Private,
    )
}

#[test]
fn opens_the_config_dir_of_tauri() {
    let storage = Storage::open(APP_IDENTIFIER).unwrap();
    assert_eq!(
        storage.dir(),
        dirs::config_dir().unwrap().join(APP_IDENTIFIER)
    );
    assert_eq!(storage.profiles_path(), storage.dir().join(PROFILES_FILE));
    assert_eq!(storage.service(), "overseer");
}

#[test]
fn other_builds_never_share_the_apps_credentials() {
    assert_eq!(service(APP_IDENTIFIER), "overseer");
    assert_eq!(
        service("com.openscreeps.overseer.verify"),
        "overseer:com.openscreeps.overseer.verify"
    );
    assert_ne!(
        service("com.openscreeps.overseer.verify"),
        service(APP_IDENTIFIER)
    );
}

#[test]
fn a_missing_file_gives_the_defaults() {
    let dir = TempDir::new("missing");
    let storage = dir.storage("missing");
    assert_eq!(storage.load().unwrap(), Profiles::default());
    assert!(!dir.0.exists(), "loading creates nothing");
}

#[test]
fn updates_round_trip() {
    let dir = TempDir::new("round-trip");
    let storage = dir.storage("round-trip");
    let url = storage
        .update(|profiles| profiles.add(private("local")))
        .unwrap()
        .unwrap();
    assert_eq!(url, "http://local.example/");
    let loaded = storage.load().unwrap();
    assert_eq!(loaded.servers.len(), 2);
    assert_eq!(loaded.servers[0].url, OFFICIAL_URL);
    assert_eq!(loaded.server(&url).unwrap().name, "local");
    let text = fs::read_to_string(storage.profiles_path()).unwrap();
    assert_eq!(Profiles::parse(Some(&text)), loaded);
    assert_eq!(text, loaded.to_json());
}

#[test]
fn a_corrupt_file_gives_the_defaults() {
    let dir = TempDir::new("corrupt");
    let storage = dir.storage("corrupt");
    fs::create_dir_all(&dir.0).unwrap();
    for corrupt in [&b"{\"servers\": [tr"[..], &[0xff, 0xfe, 0x00]] {
        fs::write(storage.profiles_path(), corrupt).unwrap();
        assert_eq!(storage.load().unwrap(), Profiles::default());
    }
    storage
        .update(|profiles| profiles.add(private("after")))
        .unwrap()
        .unwrap();
    assert_eq!(storage.load().unwrap().servers.len(), 2);
}

#[test]
fn concurrent_updates_keep_every_change() {
    const PER_THREAD: usize = 25;
    let dir = TempDir::new("concurrent");
    let start = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|thread_name| {
            // Each thread has its own storage, as two processes would.
            let storage = dir.storage("concurrent");
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                for index in 0..PER_THREAD {
                    storage
                        .update(|profiles| profiles.add(private(&format!("{thread_name}{index}"))))
                        .unwrap()
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let profiles = dir.storage("concurrent").load().unwrap();
    assert_eq!(profiles.servers.len(), 1 + 2 * PER_THREAD);
}

#[test]
fn secrets_set_get_and_delete() {
    let dir = TempDir::new("secrets");
    let storage = dir.storage("overseer:secrets-test");
    let other = dir.storage("overseer:secrets-test-other");
    let key = "token:http://local.example/";
    assert_eq!(storage.secret(key).unwrap(), None);
    storage.set_secret(key, "first").unwrap();
    storage.set_secret(key, "second").unwrap();
    assert_eq!(storage.secret(key).unwrap().as_deref(), Some("second"));
    assert_eq!(other.secret(key).unwrap(), None, "services are separate");
    storage.delete_secret(key).unwrap();
    assert_eq!(storage.secret(key).unwrap(), None);
    storage.delete_secret(key).unwrap();
    assert!(!dir.0.exists(), "secrets are not files");
}

#[test]
fn missing_secrets_are_not_errors() {
    assert_eq!(found(Ok("t".to_owned())).unwrap(), Some("t".to_owned()));
    assert_eq!(found(Err(keyring::Error::NoEntry)).unwrap(), None);
    assert!(found(Err(keyring::Error::TooLong("user".to_owned(), 1))).is_err());
    deleted(Err(keyring::Error::NoEntry)).unwrap();
    assert!(deleted(Err(keyring::Error::Invalid("k".to_owned(), "r".to_owned()))).is_err());
}

#[test]
fn errors_never_contain_the_secret() {
    let error = secrets_error("token:x", &keyring::Error::BadEncoding(b"hunter2".to_vec()));
    assert!(matches!(&error, Error::Secrets { key, .. } if key == "token:x"));
    assert!(!error.to_string().contains("hunter2"));
    assert!(!format!("{error:?}").contains("hunter2"));
    assert!(
        !format!("{error:?}").contains("104, 117"),
        "no bytes of the secret"
    );
}
