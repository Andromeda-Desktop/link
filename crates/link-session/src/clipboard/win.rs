//! The Windows clipboard on its own thread. A message-only window hears each change (`AddClipboardFormatListener`)
//! and owns a phone's offer, whose types it renders only when an app pastes one (delayed rendering).

#![allow(unsafe_code, reason = "FFI: user32 clipboard and kernel32 global memory calls")]

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::mpsc as sync;
use std::time::Duration;

use anyhow::Context as _;
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HANDLE, HWND, LPARAM, LRESULT, SetLastError, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW, RegisterClassExW,
    WM_APP, WM_CLIPBOARDUPDATE, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSEXW,
};
use windows_sys::core::w;

use super::win_format::{
    cf_html_of_html, dib_of_image, html_of_cf_html, png_len, png_of_dib, text_of_utf16, utf16_of_text,
};
use super::{Event, HTML_MIME, MAX_BYTES, TEXT_MIMES, plan};

/// Posted to the window when `Win::offer_remote` queued a phone's types.
const OFFER: u32 = WM_APP;
/// Another app may hold the clipboard open for a moment.
const OPEN_TRIES: u32 = 10;
const OPEN_RETRY: Duration = Duration::from_millis(20);
const PNG: &str = "image/png";
const JPEG: &str = "image/jpeg";

const UNICODE_TEXT: u32 = CF_UNICODETEXT as u32;
const DIB: u32 = CF_DIB as u32;

pub struct Win {
    events: mpsc::UnboundedReceiver<Event>,
    offers: sync::Sender<Offer>,
    /// The window's address; it lives until the process ends.
    window: usize,
}

impl Win {
    pub fn connect() -> anyhow::Result<Self> {
        let (sender, events) = mpsc::unbounded_channel();
        let (offers, offered) = sync::channel();
        let (made, window) = sync::sync_channel(1);
        std::thread::Builder::new().name("clipboard".into()).spawn(move || {
            let window = match create() {
                Ok(window) => window,
                Err(error) => {
                    let _ = made.send(Err(error));
                    return;
                }
            };
            STATE.with(|cell| {
                *cell.borrow_mut() =
                    Some(State { window, events: sender, offers: offered, formats: Formats::register(), offer: None });
            });
            let _ = made.send(Ok(window.expose_provenance()));
            let mut message = MSG::default();
            // SAFETY: `message` is written by GetMessageW and read by DispatchMessageW on this thread.
            while unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) } > 0 {
                unsafe { DispatchMessageW(&raw const message) };
            }
        })?;
        let window = window.recv().context("the clipboard thread stopped")??;
        Ok(Self { events, offers, window })
    }

    /// Makes `device`'s offer `id` of `mimes` the clipboard; its types are rendered when an app pastes them.
    pub fn offer_remote(&mut self, device: &str, id: u64, mimes: &[String], size: u64) -> anyhow::Result<()> {
        let offer = Offer { device: device.to_owned(), id, mimes: mimes.to_vec(), size };
        self.offers.send(offer).context("the clipboard thread stopped")?;
        let window: HWND = std::ptr::with_exposed_provenance_mut(self.window);
        // SAFETY: the window lives until the process ends.
        if unsafe { PostMessageW(window, OFFER, 0, 0) } == 0 {
            return Err(std::io::Error::last_os_error()).context("waking the clipboard thread");
        }
        Ok(())
    }

    pub async fn next(&mut self) -> anyhow::Result<Event> {
        self.events.recv().await.context("the clipboard thread stopped")
    }
}

thread_local! {
    /// The clipboard thread's state, for its window procedure.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn create() -> anyhow::Result<HWND> {
    let class = w!("AndromedaLinkClipboard");
    // SAFETY: null asks for this program's module.
    let instance = unsafe { GetModuleHandleW(null()) };
    let window_class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        lpszClassName: class,
        ..WNDCLASSEXW::default()
    };
    // SAFETY: `window_class` is filled in and its class name is a static string.
    if unsafe { RegisterClassExW(&raw const window_class) } == 0 {
        return Err(std::io::Error::last_os_error()).context("registering the clipboard window class");
    }
    // SAFETY: the class is registered above; a message-only window, no menu or creation data.
    let window = unsafe { CreateWindowExW(0, class, class, 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), instance, null()) };
    if window.is_null() {
        return Err(std::io::Error::last_os_error()).context("creating the clipboard window");
    }
    // SAFETY: `window` lives until the process ends, so the listener is never removed.
    if unsafe { AddClipboardFormatListener(window) } == 0 {
        return Err(std::io::Error::last_os_error()).context("listening to the clipboard");
    }
    Ok(window)
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = match message {
        WM_CLIPBOARDUPDATE => with_state(State::changed),
        OFFER => with_state(State::offer),
        WM_RENDERFORMAT => with_state(|state| state.render(u32::try_from(wparam).unwrap_or(0))),
        // The window goes with the process, while the phone may not answer; unrendered formats leave the clipboard
        // when this returns ("WM_RENDERALLFORMATS").
        WM_RENDERALLFORMATS => with_state(|state| state.offer = None),
        _ => false,
    };
    if handled {
        return 0;
    }
    // SAFETY: the arguments are the ones Windows passed in.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Runs `handle` on the thread's state; false before the state is set or while a handler runs.
fn with_state(handle: impl FnOnce(&mut State)) -> bool {
    STATE.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else {
            log::warn!("a clipboard message arrived inside another one; dropped");
            return false;
        };
        state.as_mut().map(handle).is_some()
    })
}

/// The registered formats this backend reads or writes.
struct Formats {
    html: u32,
    png: u32,
    /// Microsoft's markers for content that must stay out of history and off other devices ("Clipboard Formats").
    exclude: u32,
    history: u32,
    cloud: u32,
    /// Set with a phone's offer, so a clipboard manager that copies it back is not sent to phones again.
    remote: u32,
}

impl Formats {
    fn register() -> Self {
        // SAFETY: each name is a static wide string; 0 (failure) never matches an available format.
        unsafe {
            Self {
                html: RegisterClipboardFormatW(w!("HTML Format")),
                png: RegisterClipboardFormatW(w!("PNG")),
                exclude: RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
                history: RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
                cloud: RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
                // `super::REMOTE_MIME`, which `w!` cannot take.
                remote: RegisterClipboardFormatW(w!("application/x-andromeda-link-remote")),
            }
        }
    }
}

struct State {
    window: HWND,
    events: mpsc::UnboundedSender<Event>,
    offers: sync::Receiver<Offer>,
    formats: Formats,
    /// The phone offer this window's clipboard holds; set only once it took the clipboard.
    offer: Option<Offer>,
}

/// A phone's clip: its device, id and types.
struct Offer {
    device: String,
    id: u64,
    mimes: Vec<String>,
    size: u64,
}

/// A format's bytes as read, converted to the phone's type once the clipboard is closed.
enum Raw {
    Text(Vec<u8>),
    Html(Vec<u8>),
    Png(Vec<u8>),
    Dib(Vec<u8>),
}

impl Raw {
    fn convert(self) -> Option<Vec<u8>> {
        match self {
            Self::Text(data) => Some(text_of_utf16(&data).into_bytes()),
            Self::Html(data) => html_of_cf_html(&data).map(String::into_bytes),
            Self::Png(mut data) => {
                data.truncate(png_len(&data)?);
                Some(data)
            }
            Self::Dib(data) => png_of_dib(&data),
        }
    }
}

impl State {
    /// A new clipboard from another app: its types for phones, unless it must not leave this PC.
    fn changed(&mut self) {
        // SAFETY: no arguments.
        if unsafe { GetClipboardOwner() } == self.window {
            return;
        }
        let Some(open) = Open::new(self.window) else {
            log::warn!("reading the clipboard: {}", std::io::Error::last_os_error());
            return;
        };
        if self.private(&open) || open.has(self.formats.remote) {
            return;
        }
        let available: Vec<String> = [
            (open.has(UNICODE_TEXT), &TEXT_MIMES[..2]),
            (open.has(self.formats.html), &[HTML_MIME][..]),
            (open.has(self.formats.png) || open.has(DIB), &[PNG][..]),
        ]
        .into_iter()
        .filter(|(has, _)| *has)
        .flat_map(|(_, mimes)| mimes.iter().map(|&mime| mime.to_owned()))
        .collect();
        // Only copies while the clipboard is open: every other app's OpenClipboard waits for it.
        let raw: Vec<(Vec<String>, Raw)> = plan(&available)
            .into_iter()
            .filter_map(|names| {
                let raw = match names.first()?.as_str() {
                    HTML_MIME => Raw::Html(open.data(self.formats.html)?),
                    PNG => match open.data(self.formats.png) {
                        Some(png) => Raw::Png(png),
                        None => Raw::Dib(open.data(DIB)?),
                    },
                    _ => Raw::Text(open.data(UNICODE_TEXT)?),
                };
                Some((names, raw))
            })
            .collect();
        drop(open);
        let types = fit(raw
            .into_iter()
            .filter_map(|(names, raw)| Some((names, raw.convert()?)))
            .filter(|(_, data)| !data.is_empty()));
        if !types.is_empty() && self.events.send(Event::Local { types }).is_err() {
            log::warn!("the session stopped reading the clipboard; a copy was not sent to phones");
        }
    }

    /// Whether the clipboard carries a marker that keeps it out of history or off other devices.
    fn private(&self, open: &Open) -> bool {
        let denied = |format: u32| open.data(format).is_some_and(|data| data.get(..4) == Some(&[0; 4]));
        open.has(self.formats.exclude) || denied(self.formats.history) || denied(self.formats.cloud)
    }

    /// Takes the clipboard for the newest queued phone offer, every type to be rendered on paste.
    fn offer(&mut self) {
        let Some(offer) = self.offers.try_iter().last() else { return };
        let has = |wanted: &[&str]| offer.mimes.iter().any(|mime| wanted.contains(&mime.as_str()));
        let formats: Vec<u32> = [
            (has(&TEXT_MIMES), UNICODE_TEXT),
            (has(&[HTML_MIME]), self.formats.html),
            (has(&[PNG]), self.formats.png),
            (has(&[PNG, JPEG]), DIB),
        ]
        .into_iter()
        .filter_map(|(offered, format)| offered.then_some(format))
        .collect();
        if formats.is_empty() {
            log::info!("no Windows clipboard type for {:?}", offer.mimes);
            return;
        }
        let Some(_open) = Open::new(self.window) else {
            log::warn!("taking the clipboard: {}", std::io::Error::last_os_error());
            return;
        };
        // SAFETY: the clipboard is open for this window.
        if unsafe { EmptyClipboard() } == 0 {
            log::warn!("taking the clipboard: {}", std::io::Error::last_os_error());
            return;
        }
        for format in formats {
            // SAFETY: as above; a null handle asks for WM_RENDERFORMAT on paste, so only the last error tells a
            // failure.
            let failed = unsafe {
                SetLastError(0);
                SetClipboardData(format, null_mut()).is_null() && GetLastError() != 0
            };
            if failed {
                log::warn!("offering clipboard format {format}: {}", std::io::Error::last_os_error());
            }
        }
        set(self.formats.remote, &[1]);
        // A phone's clip stays out of Win+V history and the cloud clipboard ("Clipboard Formats"): it lives on the
        // phone, and a history read would pull every type at once.
        set(self.formats.history, &0u32.to_le_bytes());
        set(self.formats.cloud, &0u32.to_le_bytes());
        let taken =
            Event::Taken { device: offer.device.clone(), id: offer.id, mimes: offer.mimes.clone(), size: offer.size };
        self.offer = Some(offer);
        if self.events.send(taken).is_err() {
            log::warn!("the session stopped reading the clipboard; a phone clip is not prefetched");
        }
    }

    /// Puts the phone's bytes for `format` on the clipboard; an app waits in its paste meanwhile.
    fn render(&mut self, format: u32) {
        let Some(offer) = &self.offer else { return };
        let first = |wanted: &[&'static str]| {
            wanted.iter().copied().find(|mime| offer.mimes.iter().any(|offered| offered == mime))
        };
        let source = match format {
            UNICODE_TEXT => first(&TEXT_MIMES),
            // The phone's own first image type, which the session prefetches; a PNG made from a JPEG comes after it.
            DIB => offer.mimes.iter().map(String::as_str).find(|mime| [PNG, JPEG].contains(mime)),
            _ if format == self.formats.html => first(&[HTML_MIME]),
            // A JPEG pastes as PNG too; the session makes the PNG (prefetch.rs).
            _ if format == self.formats.png => first(&[PNG]),
            _ => None,
        };
        let Some(mime) = source else { return };
        let (reply, answer) = sync::sync_channel(1);
        let paste = Event::Paste { device: offer.device.clone(), id: offer.id, mime: mime.to_owned(), reply };
        if self.events.send(paste).is_err() {
            log::warn!("the session stopped reading the clipboard; a paste of {mime} gets nothing");
            return;
        }
        // No deadline here: the daemon ends a pull that stops moving, and the session then answers or drops `reply`.
        let bytes = match answer.recv() {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return,
            Err(_) => {
                log::warn!("the session dropped a paste of {mime}");
                return;
            }
        };
        let data = match format {
            UNICODE_TEXT => Some(utf16_of_text(&String::from_utf8_lossy(&bytes))),
            DIB => dib_of_image(&bytes),
            _ if format == self.formats.html => Some(cf_html_of_html(&String::from_utf8_lossy(&bytes))),
            _ => Some(bytes),
        };
        match data {
            Some(data) => set(format, &data),
            None => log::warn!("the phone's {mime} did not convert to clipboard format {format}"),
        }
    }
}

/// The clipboard opened by this thread's window, closed on drop.
struct Open(());

impl Open {
    fn new(window: HWND) -> Option<Self> {
        for _ in 0..OPEN_TRIES {
            // SAFETY: `window` belongs to this thread.
            if unsafe { OpenClipboard(window) } != 0 {
                return Some(Self(()));
            }
            std::thread::sleep(OPEN_RETRY);
        }
        None
    }

    #[expect(clippy::unused_self, reason = "an `Open` proves the clipboard is open")]
    fn has(&self, format: u32) -> bool {
        // SAFETY: the clipboard is open.
        format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
    }

    /// The bytes of `format`, if the clipboard has it and it fits the protocol's limit.
    fn data(&self, format: u32) -> Option<Vec<u8>> {
        if !self.has(format) {
            return None;
        }
        // SAFETY: the clipboard is open; the handle stays the clipboard's, read under GlobalLock and unlocked here.
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            if size == 0 || size as u64 > MAX_BYTES {
                return None;
            }
            let data = GlobalLock(handle);
            if data.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard.
        unsafe { CloseClipboard() };
    }
}

/// Hands `data` to the clipboard as `format`: while answering `WM_RENDERFORMAT`, which needs no `OpenClipboard`, or
/// with the clipboard open.
fn set(format: u32, data: &[u8]) {
    // SAFETY: a moveable block of `data.len()` bytes is filled under GlobalLock; the clipboard owns it once
    // SetClipboardData succeeds, else it is freed here.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if memory.is_null() {
            log::warn!("clipboard memory: {}", std::io::Error::last_os_error());
            return;
        }
        let target = GlobalLock(memory);
        if target.is_null() {
            GlobalFree(memory);
            return;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), target.cast::<u8>(), data.len());
        GlobalUnlock(memory);
        if SetClipboardData(format, memory as HANDLE).is_null() {
            log::warn!("putting format {format} on the clipboard: {}", std::io::Error::last_os_error());
            GlobalFree(memory);
        }
    }
}

/// The types in order while their total fits the protocol's limit.
fn fit(types: impl Iterator<Item = (Vec<String>, Vec<u8>)>) -> Vec<(Vec<String>, Vec<u8>)> {
    let mut total = 0;
    types
        .take_while(|(_, data)| {
            total += data.len() as u64;
            total <= MAX_BYTES
        })
        .collect()
}
