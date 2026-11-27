//! The Link window's Link1 transport behind the C ABI of `include/link_ffi.h`: one thread runs a `link_api::connect`
//! connection and hands every reply and Link1 signal to the window as a raw D-Bus body.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::num::NonZeroU32;
use std::thread::JoinHandle;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::message::Type;
use zbus::{Connection, Message, MessageStream};

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd};
#[cfg(not(unix))]
type RawFd = i32;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(25);
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
const ERROR_ONLY: u32 = 1;
const DISCONNECTED: &str = "org.freedesktop.DBus.Error.Disconnected";
const NO_REPLY: &str = "org.freedesktop.DBus.Error.NoReply";
const FAILED: &str = "org.freedesktop.DBus.Error.Failed";

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Up = 0,
    Down = 1,
    Reply = 2,
    Error = 3,
    Signal = 4,
}

#[repr(C)]
pub struct LinkEvent {
    pub kind: LinkKind,
    pub token: u64,
    pub iface: *const c_char,
    pub member: *const c_char,
    pub error_name: *const c_char,
    pub error_message: *const c_char,
    pub signature: *const c_char,
    pub body: *const u8,
    pub len: usize,
    pub fds: *const RawFd,
    pub nfds: usize,
}

pub type LinkPost = extern "C" fn(ctx: *mut c_void, event: *const LinkEvent);

pub struct LinkConn {
    commands: mpsc::UnboundedSender<Call>,
    thread: JoinHandle<()>,
}

struct Call {
    token: u64,
    iface: String,
    member: String,
    signature: String,
    body: Vec<u8>,
    #[cfg(unix)]
    fds: Vec<OwnedFd>,
    timeout: Duration,
    error_only: bool,
    /// An fd the call names that could not be dup'ed; the call fails with it.
    fd_error: Option<String>,
}

#[derive(Clone, Copy)]
struct Pending {
    token: u64,
    error_only: bool,
}

/// The window's callback; the C contract makes it callable from the connection thread.
struct Poster {
    post: LinkPost,
    ctx: *mut c_void,
}

// SAFETY: link_open's caller promises that `post` with `ctx` may run on another thread (link_ffi.h).
unsafe impl Send for Poster {}

/// Strings for one event; empty fields stay null.
#[derive(Default)]
struct Event<'a> {
    token: u64,
    iface: Option<&'a str>,
    member: Option<&'a str>,
    error_name: Option<&'a str>,
    error_message: Option<&'a str>,
    signature: Option<&'a str>,
    body: &'a [u8],
    fds: Vec<RawFd>,
}

fn c_string(text: Option<&str>) -> Option<CString> {
    text.map(|text| CString::new(text.replace('\0', "")).unwrap_or_default())
}

impl Poster {
    fn post(&self, kind: LinkKind, event: &Event<'_>) {
        let strings = [event.iface, event.member, event.error_name, event.error_message, event.signature].map(c_string);
        let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let raw = LinkEvent {
            kind,
            token: event.token,
            iface: ptr(&strings[0]),
            member: ptr(&strings[1]),
            error_name: ptr(&strings[2]),
            error_message: ptr(&strings[3]),
            signature: ptr(&strings[4]),
            body: event.body.as_ptr(),
            len: event.body.len(),
            fds: event.fds.as_ptr(),
            nfds: event.fds.len(),
        };
        (self.post)(self.ctx, std::ptr::from_ref(&raw));
    }

    fn presence(&self, up: bool) {
        self.post(if up { LinkKind::Up } else { LinkKind::Down }, &Event::default());
    }

    fn error(&self, token: u64, name: &str, message: &str) {
        self.post(
            LinkKind::Error,
            &Event { token, error_name: Some(name), error_message: Some(message), ..Event::default() },
        );
    }
}

/// Owned copies of a message's fds for the receiver.
#[cfg(unix)]
fn owned_fds(msg: &Message) -> Vec<RawFd> {
    msg.body()
        .data()
        .fds()
        .iter()
        .filter_map(|fd| fd.as_fd().try_clone_to_owned().ok().map(IntoRawFd::into_raw_fd))
        .collect()
}

#[cfg(not(unix))]
fn owned_fds(_: &Message) -> Vec<RawFd> {
    Vec::new()
}

fn send_reply(poster: &Poster, pending: Pending, msg: &Message) {
    let body = msg.body();
    match msg.message_type() {
        Type::MethodReturn if pending.error_only => {}
        Type::MethodReturn => {
            let signature = body.signature().to_string_no_parens();
            poster.post(
                LinkKind::Reply,
                &Event {
                    token: pending.token,
                    signature: Some(&signature),
                    body: body.data(),
                    fds: owned_fds(msg),
                    ..Event::default()
                },
            );
        }
        _ => {
            let header = msg.header();
            let name = header.error_name().map_or(FAILED, |name| name.as_str());
            let message = body.deserialize::<&str>().unwrap_or_default();
            poster.error(pending.token, name, message);
        }
    }
}

fn build(call: Call, bus: bool) -> zbus::Result<Message> {
    if let Some(error) = call.fd_error {
        return Err(zbus::Error::Failure(error));
    }
    let mut builder = Message::method_call(link_api::PATH, call.member.as_str()).interface(call.iface.as_str());
    if bus {
        builder = builder.destination(link_api::SERVICE);
    }
    // SAFETY: a malformed body makes linkd answer with an error; it cannot corrupt this process.
    unsafe {
        builder.build_raw_body(
            &call.body,
            call.signature.as_str(),
            #[cfg(unix)]
            call.fds.into_iter().map(Into::into).collect(),
        )
    }
}

/// Serves one connection until it closes (true) or the window closes the `LinkConn` (false).
async fn serve(conn: &Connection, commands: &mut mpsc::UnboundedReceiver<Call>, poster: &Poster) -> bool {
    let bus = conn.is_bus();
    let mut stream = MessageStream::from(conn);
    if bus {
        match watch_owner(conn).await {
            Ok(up) => poster.presence(up),
            Err(error) => {
                log::warn!("link: cannot watch {}: {error}", link_api::SERVICE);
                return true;
            }
        }
    } else {
        poster.presence(true);
    }
    let mut pending: HashMap<NonZeroU32, Pending> = HashMap::new();
    let (expired_tx, mut expired) = mpsc::unbounded_channel::<NonZeroU32>();
    let lost = loop {
        tokio::select! {
            call = commands.recv() => {
                let Some(call) = call else { break false };
                let (token, timeout, error_only) = (call.token, call.timeout, call.error_only);
                let msg = match build(call, bus) {
                    Ok(msg) => msg,
                    Err(error) => {
                        poster.error(token, FAILED, &error.to_string());
                        continue;
                    }
                };
                let serial = msg.primary_header().serial_num();
                pending.insert(serial, Pending { token, error_only });
                if let Err(error) = conn.send(&msg).await {
                    pending.remove(&serial);
                    poster.error(token, DISCONNECTED, &error.to_string());
                    continue;
                }
                let expired_tx = expired_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(timeout).await;
                    let _ = expired_tx.send(serial);
                });
            }
            Some(serial) = expired.recv() => {
                if let Some(call) = pending.remove(&serial) {
                    poster.error(call.token, NO_REPLY, "linkd did not reply in time");
                }
            }
            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { break true };
                dispatch(&msg, &mut pending, poster);
            }
        }
    };
    for call in pending.into_values() {
        poster.error(call.token, DISCONNECTED, "the connection to linkd closed");
    }
    if lost {
        poster.presence(false);
    }
    lost
}

fn dispatch(msg: &Message, pending: &mut HashMap<NonZeroU32, Pending>, poster: &Poster) {
    let header = msg.header();
    match msg.message_type() {
        Type::MethodReturn | Type::Error => {
            if let Some(call) = header.reply_serial().and_then(|serial| pending.remove(&serial)) {
                send_reply(poster, call, msg);
            }
        }
        Type::Signal => {
            let (Some(iface), Some(member)) = (header.interface(), header.member()) else { return };
            if iface.as_str() == "org.freedesktop.DBus" && member.as_str() == "NameOwnerChanged" {
                if let Ok((name, _, owner)) = msg.body().deserialize::<(&str, &str, &str)>()
                    && name == link_api::SERVICE
                {
                    poster.presence(!owner.is_empty());
                }
                return;
            }
            if header.path().is_none_or(|path| path.as_str() != link_api::PATH) {
                return;
            }
            let body = msg.body();
            let signature = body.signature().to_string_no_parens();
            poster.post(
                LinkKind::Signal,
                &Event {
                    iface: Some(iface.as_str()),
                    member: Some(member.as_str()),
                    signature: Some(&signature),
                    body: body.data(),
                    fds: owned_fds(msg),
                    ..Event::default()
                },
            );
        }
        Type::MethodCall => {}
    }
}

/// Subscribes to linkd's signals and its name owner on the session bus; true when linkd runs now.
async fn watch_owner(conn: &Connection) -> zbus::Result<bool> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let signals =
        zbus::MatchRule::builder().msg_type(Type::Signal).sender(link_api::SERVICE).path(link_api::PATH).build()?;
    dbus.add_match_rule(signals).await?;
    let owner = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .add_arg(link_api::SERVICE)
        .build()?;
    dbus.add_match_rule(owner).await?;
    Ok(dbus.name_has_owner(link_api::SERVICE.try_into()?).await?)
}

/// Fails calls until the retry delay ends; false when the window closed the `LinkConn`.
async fn wait_down(delay: Duration, commands: &mut mpsc::UnboundedReceiver<Call>, poster: &Poster) -> bool {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return true,
            call = commands.recv() => match call {
                Some(call) => poster.error(call.token, DISCONNECTED, "linkd is not running"),
                None => return false,
            },
        }
    }
}

async fn run(mut commands: mpsc::UnboundedReceiver<Call>, poster: Poster) {
    let mut delay = RETRY_FIRST;
    let mut was_up = None;
    loop {
        match link_api::connect().await {
            Ok(conn) => {
                delay = RETRY_FIRST;
                was_up = Some(true);
                if !serve(&conn, &mut commands, &poster).await {
                    return;
                }
            }
            Err(error) => {
                log::debug!("link: no connection to linkd: {error}");
                if was_up != Some(false) {
                    poster.presence(false);
                    was_up = Some(false);
                }
            }
        }
        if !wait_down(delay, &mut commands, &poster).await {
            return;
        }
        delay = (delay * 2).min(RETRY_MAX);
    }
}

/// # Safety
/// `post` must be callable with `ctx` from another thread until `link_close` returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn link_open(post: LinkPost, ctx: *mut c_void) -> *mut LinkConn {
    let (commands, receiver) = mpsc::unbounded_channel();
    let poster = Poster { post, ctx };
    let thread = std::thread::Builder::new().name("link-conn".into()).spawn(move || {
        match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(run(receiver, poster)),
            Err(error) => log::error!("link: no runtime: {error}"),
        }
    });
    match thread {
        Ok(thread) => Box::into_raw(Box::new(LinkConn { commands, thread })),
        Err(error) => {
            log::error!("link: no connection thread: {error}");
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `conn` comes from `link_open` and is not used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn link_close(conn: *mut LinkConn) {
    if conn.is_null() {
        return;
    }
    // SAFETY: the caller hands back the box from link_open.
    let LinkConn { commands, thread } = *unsafe { Box::from_raw(conn) };
    drop(commands);
    let _ = thread.join();
}

/// # Safety
/// `conn` comes from `link_open`; the strings are NUL-terminated; `body` holds `len` bytes and `fds` holds `nfds`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn link_call(
    conn: *mut LinkConn,
    token: u64,
    iface: *const c_char,
    member: *const c_char,
    signature: *const c_char,
    body: *const u8,
    len: usize,
    fds: *const RawFd,
    nfds: usize,
    timeout_ms: u32,
    flags: u32,
) {
    // SAFETY: the caller's contract above.
    let (conn, iface, member, signature, body, fds) = unsafe {
        let text = |s: *const c_char| {
            if s.is_null() { String::new() } else { CStr::from_ptr(s).to_string_lossy().into_owned() }
        };
        let body = if len == 0 { &[][..] } else { std::slice::from_raw_parts(body, len) };
        let fds = if nfds == 0 { &[][..] } else { std::slice::from_raw_parts(fds, nfds) };
        (&*conn, text(iface), text(member), text(signature), body.to_vec(), fds)
    };
    let mut fd_error = None;
    #[cfg(unix)]
    let fds = {
        let mut owned = Vec::with_capacity(fds.len());
        for &fd in fds {
            // SAFETY: the caller keeps fd open during this call.
            match unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned() {
                Ok(fd) => owned.push(fd),
                Err(error) => fd_error = Some(format!("cannot dup fd {fd}: {error}")),
            }
        }
        owned
    };
    #[cfg(not(unix))]
    if !fds.is_empty() {
        fd_error = Some("this platform passes no fds".into());
    }
    let timeout = if timeout_ms == 0 { DEFAULT_TIMEOUT } else { Duration::from_millis(u64::from(timeout_ms)) };
    let call = Call {
        token,
        iface,
        member,
        signature,
        body,
        #[cfg(unix)]
        fds,
        timeout,
        error_only: flags & ERROR_ONLY != 0,
        fd_error,
    };
    // The thread ends only in link_close, after which the caller makes no call.
    let _ = conn.commands.send(call);
}
