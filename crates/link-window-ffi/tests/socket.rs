//! The C ABI against a peer-to-peer Link1 server on `ANDROMEDA_LINK_SOCKET`, the macOS transport: presence while the
//! daemon is down, starts and stops; replies, errors, timeouts, signals and fds both ways.
#![cfg(unix)]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::too_many_arguments)]
#![allow(clippy::unused_self, clippy::needless_pass_by_value, reason = "zbus interface methods")]

use std::ffi::{CStr, CString, c_void};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use link_window_ffi::{LinkConn, LinkEvent, LinkKind, link_call, link_close, link_open};
use zbus::object_server::SignalEmitter;
use zbus::wire::{
    Endian,
    serialized::{Context, Data},
    to_bytes,
};

#[derive(Debug)]
struct Event {
    kind: LinkKind,
    token: u64,
    member: String,
    error_name: String,
    signature: String,
    body: Vec<u8>,
    fds: Vec<OwnedFd>,
}

extern "C" fn post(ctx: *mut c_void, event: *const LinkEvent) {
    let (tx, event) = unsafe { (&*ctx.cast::<mpsc::Sender<Event>>(), &*event) };
    let text = |s: *const std::ffi::c_char| {
        if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
    };
    let body =
        if event.len == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(event.body, event.len) }.to_vec() };
    let fds = (0..event.nfds).map(|i| unsafe { OwnedFd::from_raw_fd(*event.fds.add(i)) }).collect();
    let _ = tx.send(Event {
        kind: event.kind,
        token: event.token,
        member: text(event.member),
        error_name: text(event.error_name),
        signature: text(event.signature),
        body,
        fds,
    });
}

struct Daemon;

#[zbus::interface(name = "org.andromeda.Link1")]
impl Daemon {
    fn echo(&self, text: String) -> String {
        format!("echo {text}")
    }

    fn fail(&self) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::AccessDenied("nope".into()))
    }

    async fn hang(&self) {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }

    fn open_fd(&self) -> zbus::OwnedFd {
        let (read, mut write) = std::io::pipe().unwrap();
        write.write_all(b"from linkd").unwrap();
        OwnedFd::from(read).into()
    }

    fn read_fd(&self, fd: zbus::OwnedFd) -> String {
        let mut text = String::new();
        std::fs::File::from(OwnedFd::from(fd)).read_to_string(&mut text).unwrap();
        text
    }

    async fn emit(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>, text: String) -> zbus::fdo::Result<()> {
        Self::ping(&emitter, &text).await?;
        Ok(())
    }

    #[zbus(signal)]
    async fn ping(emitter: &SignalEmitter<'_>, text: &str) -> zbus::Result<()>;
}

/// The daemon: accepts on the socket and serves each peer; stop drops its runtime, as a daemon exit closes its sockets.
struct Server {
    runtime: Option<tokio::runtime::Runtime>,
    path: PathBuf,
}

impl Server {
    fn start(&mut self) {
        let listener = std::os::unix::net::UnixListener::bind(&self.path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.spawn(async move {
            let listener = tokio::net::UnixListener::from_std(listener).unwrap();
            let mut peers = Vec::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let conn = zbus::connection::Builder::unix_stream(stream.into_std().unwrap())
                    .server(zbus::Guid::generate())
                    .p2p()
                    .serve_at(link_api::PATH, Daemon)
                    .build()
                    .await
                    .unwrap();
                peers.push(conn);
            }
        });
        self.runtime = Some(runtime);
    }

    fn stop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

struct Client {
    conn: *mut LinkConn,
    events: std::rc::Rc<mpsc::Receiver<Event>>,
    _tx: Box<mpsc::Sender<Event>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe { link_close(self.conn) };
    }
}

impl Client {
    fn call(&self, token: u64, member: &str, signature: &str, body: &[u8], fds: &[i32], timeout_ms: u32, flags: u32) {
        let iface = CString::new("org.andromeda.Link1").unwrap();
        let member = CString::new(member).unwrap();
        let signature = CString::new(signature).unwrap();
        unsafe {
            link_call(
                self.conn,
                token,
                iface.as_ptr(),
                member.as_ptr(),
                signature.as_ptr(),
                body.as_ptr(),
                body.len(),
                fds.as_ptr(),
                fds.len(),
                timeout_ms,
                flags,
            );
        }
    }

    fn next(&self) -> Event {
        self.events.recv_timeout(Duration::from_secs(10)).expect("an event within 10 s")
    }

    fn reply(&self, token: u64) -> Event {
        let event = self.next();
        assert_eq!((event.kind, event.token), (LinkKind::Reply, token), "{event:?}");
        event
    }
}

fn string_body(text: &str) -> Vec<u8> {
    to_bytes(Context::new(Endian::Little, 0), &(text,)).unwrap().to_vec()
}

fn decode_string(event: &Event) -> String {
    assert_eq!(event.signature, "s");
    let data = Data::new(&event.body[..], Context::new(Endian::Little, 0));
    data.deserialize::<String>().unwrap().0
}

#[test]
fn link1_over_the_socket() {
    let dir = std::env::temp_dir().join(format!("link-window-ffi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("link.sock");
    // SAFETY: this is the only test in this binary, and it sets the variable before any thread reads it.
    unsafe { std::env::set_var(link_api::SOCKET_ENV, &path) };
    let mut server = Server { runtime: None, path };

    let (tx, events) = mpsc::channel();
    let tx = Box::new(tx);
    let conn = unsafe { link_open(post, std::ptr::from_ref(&*tx).cast_mut().cast()) };
    let events = std::rc::Rc::new(events);
    let client = Client { conn, events: events.clone(), _tx: tx };

    assert_eq!(client.next().kind, LinkKind::Down, "no daemon yet");
    client.call(1, "Echo", "s", &string_body("early"), &[], 0, 0);
    let early = client.next();
    assert_eq!(
        (early.kind, early.token, early.error_name.as_str()),
        (LinkKind::Error, 1, "org.freedesktop.DBus.Error.Disconnected")
    );

    server.start();
    assert_eq!(client.next().kind, LinkKind::Up, "the client reconnects by itself");

    client.call(2, "Echo", "s", &string_body("hi"), &[], 0, 0);
    assert_eq!(decode_string(&client.reply(2)), "echo hi");

    client.call(3, "Fail", "", &[], &[], 0, 0);
    let failed = client.next();
    assert_eq!(
        (failed.kind, failed.token, failed.error_name.as_str()),
        (LinkKind::Error, 3, "org.freedesktop.DBus.Error.AccessDenied")
    );

    client.call(4, "Hang", "", &[], &[], 200, 0);
    let late = client.next();
    assert_eq!(
        (late.kind, late.token, late.error_name.as_str()),
        (LinkKind::Error, 4, "org.freedesktop.DBus.Error.NoReply")
    );

    // An error-only call posts nothing on success: the next event is the reply to the call after it.
    client.call(5, "Echo", "s", &string_body("quiet"), &[], 0, 1);
    client.call(6, "Echo", "s", &string_body("loud"), &[], 0, 0);
    assert_eq!(decode_string(&client.reply(6)), "echo loud");

    client.call(7, "OpenFd", "", &[], &[], 0, 0);
    let opened = client.reply(7);
    assert_eq!((opened.signature.as_str(), opened.fds.len()), ("h", 1));
    let mut text = String::new();
    std::fs::File::from(opened.fds.into_iter().next().unwrap()).read_to_string(&mut text).unwrap();
    assert_eq!(text, "from linkd");

    let (read, mut write) = std::io::pipe().unwrap();
    write.write_all(b"from the window").unwrap();
    drop(write);
    let fd_body = to_bytes(Context::new(Endian::Little, 0), &(zbus::Fd::from(&read),)).unwrap().to_vec();
    client.call(8, "ReadFd", "h", &fd_body, &[std::os::fd::AsRawFd::as_raw_fd(&read)], 0, 0);
    drop(read);
    assert_eq!(decode_string(&client.reply(8)), "from the window");

    client.call(9, "Emit", "s", &string_body("pong"), &[], 0, 0);
    let mut seen = [client.next(), client.next()];
    seen.sort_by_key(|event| event.kind as u8);
    assert_eq!((seen[0].kind, seen[0].token), (LinkKind::Reply, 9));
    assert_eq!((seen[1].kind, seen[1].member.as_str()), (LinkKind::Signal, "Ping"));
    assert_eq!(decode_string(&seen[1]), "pong");

    client.call(10, "Hang", "", &[], &[], 0, 0);
    server.stop();
    let mut stopped = [client.next(), client.next()];
    stopped.sort_by_key(|event| event.kind as u8);
    assert_eq!(stopped[0].kind, LinkKind::Down);
    assert_eq!(
        (stopped[1].kind, stopped[1].token, stopped[1].error_name.as_str()),
        (LinkKind::Error, 10, "org.freedesktop.DBus.Error.Disconnected")
    );

    server.start();
    assert_eq!(client.next().kind, LinkKind::Up, "a restarted daemon comes back");
    client.call(11, "Echo", "s", &string_body("again"), &[], 0, 0);
    assert_eq!(decode_string(&client.reply(11)), "echo again");

    drop(client);
    assert!(events.try_recv().is_err(), "no post after link_close");
    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
