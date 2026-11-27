//! Link1 on Windows: zbus peer to peer over a named pipe that only the user's own account may open or own. The pipe
//! carries bytes only, so the members that pass a descriptor elsewhere are not in the Windows client.

#![allow(unsafe_code, reason = "FFI: token, SID and security descriptor calls on handles this module owns")]

use std::ffi::c_void;
use std::fmt::Debug;
use std::io;
use std::os::windows::io::AsRawHandle as _;
use std::ptr::null_mut;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use zbus::connection::AuthMechanism;
use zbus::connection::socket::{ReadHalf, Socket, Split, WriteHalf};

/// How long a client retries while every pipe instance is busy; the daemon opens the next one as it accepts.
const BUSY_WAIT: Duration = Duration::from_secs(2);
const BUSY_STEP: Duration = Duration::from_millis(50);

/// The daemon's pipe for this account: `\\.\pipe\andromeda-link-<user SID>`.
pub fn default_name() -> io::Result<String> {
    Ok(format!(r"\\.\pipe\andromeda-link-{}", user_sid()?))
}

/// Opens a server instance of `name` that only this account owns and opens, refusing remote clients. `first` makes
/// the call fail when the name exists, so a pipe another account created first is never served.
pub fn listen(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let sid = user_sid()?;
    create(name, first, &format!("O:{sid}D:P(A;;GA;;;{sid})"))
}

fn create(name: &str, first: bool, sddl: &str) -> io::Result<NamedPipeServer> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `sddl` is NUL-terminated; on success `descriptor` is a LocalAlloc block freed below.
    let made = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            null_mut(),
        )
    };
    if made == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    // SAFETY: `attributes` is a valid SECURITY_ATTRIBUTES whose descriptor lives until after the call.
    let server = unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast::<c_void>()) };
    // SAFETY: `descriptor` came from LocalAlloc above and nothing uses it after this.
    unsafe { LocalFree(descriptor) };
    server
}

/// Opens `name` as a client and refuses it unless this account owns it.
pub async fn open(name: &str) -> io::Result<NamedPipeClient> {
    let mut waited = Duration::ZERO;
    let client = loop {
        match ClientOptions::new().open(name) {
            Ok(client) => break client,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.cast_signed()) && waited < BUSY_WAIT => {
                tokio::time::sleep(BUSY_STEP).await;
                waited += BUSY_STEP;
            }
            Err(error) => return Err(error),
        }
    };
    if owner_sid(client.as_raw_handle())? != user_sid()? {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the Link pipe belongs to another account"));
    }
    Ok(client)
}

/// Whether the client connected to `server` runs in this process's Windows session: the DACL admits the account's
/// processes in every session, such as a second Remote Desktop login.
pub fn same_session(server: &NamedPipeServer) -> io::Result<bool> {
    let mut pid = 0;
    // SAFETY: the handle is the connected pipe `server` owns.
    if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &raw mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session_of(pid)? == session_of(std::process::id())?)
}

fn session_of(pid: u32) -> io::Result<u32> {
    let mut session = 0;
    // SAFETY: `session` is a valid out pointer.
    if unsafe { ProcessIdToSessionId(pid, &raw mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

fn user_sid() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo handle of the current process needs no closing; `token` is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sid = token_user(token);
    // SAFETY: `token` is the handle opened above.
    unsafe { CloseHandle(token) };
    sid
}

fn token_user(token: HANDLE) -> io::Result<String> {
    let mut len = 0;
    // SAFETY: a null buffer of length 0 only asks for the needed length; this call fails by design.
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &raw mut len) };
    // u64 storage keeps TOKEN_USER's pointer field aligned.
    let mut buf = vec![0_u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    if unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &raw mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled `buf` with a TOKEN_USER whose SID points into `buf`, which outlives the use.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}

fn owner_sid(handle: HANDLE) -> io::Result<String> {
    let (mut owner, mut descriptor): (PSID, PSECURITY_DESCRIPTOR) = (null_mut(), null_mut());
    // SAFETY: `handle` is an open pipe handle; `owner` points into `descriptor`, freed after its last use.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status.cast_signed()));
    }
    let sid = sid_string(owner);
    // SAFETY: `descriptor` came from GetSecurityInfo's LocalAlloc.
    unsafe { LocalFree(descriptor) };
    sid
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    // SAFETY: `sid` is a valid SID; on success `text` is a NUL-terminated LocalAlloc string freed below.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `text` is NUL-terminated, so every index before the NUL is in bounds.
    let len = (0..usize::MAX).take_while(|&at| unsafe { *text.add(at) } != 0).count();
    // SAFETY: `len` u16s from `text` are initialized.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: `text` came from LocalAlloc and is not used after this.
    unsafe { LocalFree(text.cast()) };
    Ok(sid)
}

/// A pipe end as a zbus socket. Authentication is anonymous: the pipe's owner-only DACL and [`open`]'s owner check
/// already decide who talks.
#[derive(Debug)]
pub struct Pipe<S>(pub S);

impl<S: AsyncRead + AsyncWrite + Debug + Send + Sync + 'static> Socket for Pipe<S> {
    type ReadHalf = Reader<S>;
    type WriteHalf = Writer<S>;

    fn split(self) -> Split<Reader<S>, Writer<S>> {
        let (read, write) = tokio::io::split(self.0);
        Split::new(Reader(read), Writer(write))
    }
}

#[derive(Debug)]
pub struct Reader<S>(tokio::io::ReadHalf<S>);

#[async_trait::async_trait]
impl<S: AsyncRead + Debug + Send + Sync + 'static> ReadHalf for Reader<S> {
    async fn recvmsg(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer).await
    }

    fn auth_mechanism(&self) -> AuthMechanism {
        AuthMechanism::Anonymous
    }
}

#[derive(Debug)]
pub struct Writer<S>(tokio::io::WriteHalf<S>);

#[async_trait::async_trait]
impl<S: AsyncWrite + Debug + Send + Sync + 'static> WriteHalf for Writer<S> {
    // The cfg'd parameter keeps async_trait's lifetimes equal to the trait's, which declares it the same way.
    async fn sendmsg(&mut self, buffer: &[u8], #[cfg(unix)] _fds: &[std::os::fd::BorrowedFd<'_>]) -> io::Result<usize> {
        self.0.write(buffer).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.0.flush().await
    }

    async fn close(&mut self) -> io::Result<()> {
        self.0.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Result = std::result::Result<(), Box<dyn std::error::Error>>;

    struct Echo(&'static str);

    #[zbus::interface(name = "org.andromeda.Test")]
    impl Echo {
        fn echo(&self, text: &str) -> String {
            format!("{}{text}", self.0)
        }
    }

    fn test_name(what: &str) -> String {
        format!(r"\\.\pipe\andromeda-link-test-{what}-{}", std::process::id())
    }

    #[tokio::test]
    async fn a_client_calls_the_daemon_over_the_pipe() -> Result {
        let name = test_name("call");
        let server = listen(&name, true)?;
        let serving = tokio::spawn(async move {
            server.connect().await?;
            zbus::connection::Builder::socket(Pipe(server))
                .server(zbus::Guid::generate())
                .p2p()
                .auth_mechanism(AuthMechanism::Anonymous)
                .serve_at("/test", Echo("echo: "))
                .build()
                .await
        });
        let client = zbus::connection::Builder::socket(Pipe(open(&name).await?)).p2p().build().await?;
        let _server = serving.await??;
        let reply = client.call_method(None::<&str>, "/test", Some("org.andromeda.Test"), "Echo", &("hi",)).await?;
        assert_eq!(reply.body().deserialize::<String>()?, "echo: hi");
        Ok(())
    }

    #[tokio::test]
    async fn a_pipe_another_account_owns_is_refused() -> Result {
        let name = test_name("owner");
        let sid = user_sid()?;
        // Owned by Administrators, which the elevated CI account may assign; still open to this account.
        let _server = create(&name, true, &format!("O:BAD:P(A;;GA;;;{sid})"))?;
        let refused = open(&name).await.map(drop).map_err(|error| error.kind());
        assert_eq!(refused, Err(io::ErrorKind::PermissionDenied));
        Ok(())
    }

    #[tokio::test]
    async fn a_name_already_taken_is_not_served() -> Result {
        let name = test_name("first");
        let _squatter = listen(&name, true)?;
        assert!(listen(&name, true).is_err());
        Ok(())
    }
}
