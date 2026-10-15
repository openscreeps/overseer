//! The demo session; every task runs on the browser's event loop.

use std::collections::BTreeMap;

use overseer::net::rt;
use overseer::prelude::*;
use tokio::sync::broadcast::error::RecvError;
use wasm_bindgen::prelude::*;

/// Connects to `server` and starts the demo. Resolves once the session is set up (and the
/// console expression sent), rejects with the error otherwise; the status, notices, room and
/// console keep being logged while the page is open.
///
/// A non-empty `token` is used before `email`/`password`; with neither the session is
/// anonymous (no console). An empty `room` or `expression` skips that part.
///
/// # Errors
///
/// Rejects when the URL is invalid, connecting or signing in fails, the room cannot be
/// watched or the console expression cannot be sent.
#[wasm_bindgen]
pub async fn run(
    server: String,
    email: String,
    password: String,
    token: String,
    room: String,
    expression: String,
) -> Result<(), JsError> {
    let url = ServerUrl::parse(&server)?;
    let auth = if !token.is_empty() {
        Auth::Token(Token::new(token))
    } else if !email.is_empty() {
        Auth::Credentials { email, password }
    } else {
        Auth::Anonymous
    };
    log(&format!("connecting to {server}"));
    let profile = ServerProfile {
        url,
        auth,
        server_password: None,
        default_shard: None,
    };
    let overseer = Overseer::connect(profile).await?;
    log(&format!(
        "connected: shards {:?}, user {}",
        overseer.world().shards(),
        overseer.user_id().unwrap_or("(anonymous)")
    ));
    follow_status(&overseer);
    follow_notices(&overseer);
    if !room.is_empty() {
        watch_room(&overseer, &room).await?;
    }
    if overseer.user_id().is_none() {
        log("anonymous session: no console");
        return Ok(());
    }
    follow_console(&overseer)?;
    if !expression.is_empty() {
        overseer.console().execute(&expression, None).await?;
        log(&format!("sent console expression `{expression}`"));
    }
    Ok(())
}

/// Logs every connection status. The task holds a clone of the session, which keeps it open
/// while the page lives.
fn follow_status(overseer: &Overseer) {
    let session = overseer.clone();
    let mut status = overseer.status();
    rt::spawn(async move {
        loop {
            let current = *status.borrow_and_update();
            log(&format!("status: {current:?}"));
            if current == ConnectionStatus::Closed || status.changed().await.is_err() {
                break;
            }
        }
        drop(session);
    });
}

/// Logs every notice.
fn follow_notices(overseer: &Overseer) {
    let mut notices = overseer.notices();
    rt::spawn(async move {
        loop {
            match notices.recv().await {
                Ok(notice) => log(&format!("notice: {notice:?}")),
                Err(RecvError::Lagged(skipped)) => {
                    log(&format!("notice: {skipped} notices skipped"));
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Logs the object counts by type of every update of `room`.
async fn watch_room(overseer: &Overseer, room: &str) -> Result<(), Error> {
    let mut watch = overseer.rooms().watch(room).await?;
    log(&format!("watching room {room}"));
    rt::spawn(async move {
        while let Ok(snapshot) = watch.changed().await {
            if let Some(error) = &snapshot.subscription_error {
                log(&format!("room {}: {error}", snapshot.room));
                continue;
            }
            let mut counts = BTreeMap::<&str, usize>::new();
            for object in &snapshot.objects {
                *counts.entry(object.kind()).or_default() += 1;
            }
            let counts: Vec<_> = counts
                .iter()
                .map(|(kind, count)| format!("{kind} {count}"))
                .collect();
            log(&format!(
                "room {} tick {}: {} objects ({})",
                snapshot.room,
                snapshot
                    .game_time
                    .map_or_else(|| "?".to_owned(), |time| time.to_string()),
                snapshot.objects.len(),
                counts.join(", ")
            ));
        }
    });
    Ok(())
}

/// Logs every console line with its receive time.
fn follow_console(overseer: &Overseer) -> Result<(), Error> {
    let mut feed = overseer.console().feed()?;
    rt::spawn(async move {
        while let Ok(entry) = feed.recv().await {
            let received = entry
                .received
                .duration_since(rt::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            log(&format!(
                "console [{received}] {:?}: {}",
                entry.kind,
                entry.plain_text()
            ));
        }
    });
    Ok(())
}

/// Appends `line` to the page's `#log` element and the browser console.
fn log(line: &str) {
    web_sys::console::log_1(&line.into());
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let (Some(log), Ok(entry)) = (
        document.get_element_by_id("log"),
        document.create_element("div"),
    ) else {
        return;
    };
    entry.set_text_content(Some(line));
    // Only fails for nodes that cannot hold children; `#log` is a block element.
    let _ = log.append_child(&entry);
}
