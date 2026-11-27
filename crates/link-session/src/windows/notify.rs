//! Phone notifications as Windows toasts, with their actions and inline reply. An unpackaged program shows toasts
//! under an `AppUserModelID` registered in `HKCU\Software\Classes\AppUserModelId`; while this process runs, each toast's
//! Activated and Dismissed events carry the user's answer. The uninstaller removes the registration.

#![allow(unsafe_code, reason = "FFI: advapi32 registry calls with NUL-terminated wide strings that outlive each call")]

use std::collections::HashMap;

use tokio::sync::mpsc;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs, ToastNotification,
    ToastNotificationHistory, ToastNotificationManager, ToastNotifier,
};
use windows_core::{HSTRING, IInspectable, Interface as _};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};

use crate::link::Link;

const AUMID: &str = "Andromeda.Link";
const REGISTRATION: &str = r"Software\Classes\AppUserModelId\Andromeda.Link";
const DISPLAY_NAME: &str = "Andromeda Link";
const REPLY_INPUT: &str = "reply";
/// Before an action id in a button's arguments; a click on the toast itself has none.
const ACTION_PREFIX: &str = "a:";
/// Windows shows at most five buttons on a toast (app notification content schema).
const MAX_ACTIONS: usize = 5;

/// A user's answer to a phone notification; `action` is None for a dismissal.
struct Answer {
    device: String,
    id: String,
    action: Option<String>,
    reply: String,
}

pub struct Notifications {
    notifier: ToastNotifier,
    history: ToastNotificationHistory,
    answers: mpsc::UnboundedSender<Answer>,
    /// The toasts on screen or in the notification center, by (device, key); each one's events need it alive.
    shown: HashMap<(String, String), ToastNotification>,
}

impl Notifications {
    pub fn connect(link: Link) -> anyhow::Result<Self> {
        register()?;
        let aumid = HSTRING::from(AUMID);
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&aumid)?;
        let history = ToastNotificationManager::History()?;
        // A previous run's toasts have no handlers left, so their buttons would not reach the phone.
        if let Err(error) = history.ClearWithId(&aumid) {
            log::warn!("clearing an earlier run's phone notifications: {error}");
        }
        let (answers, mut answered) = mpsc::unbounded_channel::<Answer>();
        tokio::spawn(async move {
            while let Some(answer) = answered.recv().await {
                match answer.action {
                    Some(action) => link.notification_action(&answer.device, &answer.id, &action, &answer.reply).await,
                    None => link.notification_dismiss(&answer.device, &answer.id).await,
                }
            }
        });
        Ok(Self { notifier, history, answers, shown: HashMap::new() })
    }

    /// Shows a phone notification, or replaces the one with its key; an update without `alert` makes no sound.
    pub fn posted(&mut self, posted: &link_api::NotificationPostedArgs<'_>) {
        let key = (posted.device_id.to_owned(), posted.id.to_owned());
        match self.toast(posted) {
            Ok(toast) => {
                self.shown.insert(key, toast);
            }
            Err(error) => log::warn!("showing a phone notification: {error}"),
        }
    }

    pub fn removed(&mut self, device: &str, id: &str) {
        if let Some(toast) = self.shown.remove(&(device.to_owned(), id.to_owned()))
            && let Err(error) = self.notifier.Hide(&toast)
        {
            log::debug!("hiding a phone notification: {error}");
        }
        let (tag, group) = (HSTRING::from(tag(device, id)), HSTRING::from(tag(device, "")));
        if let Err(error) = self.history.RemoveGroupedTagWithId(&tag, &group, &HSTRING::from(AUMID)) {
            log::debug!("removing a phone notification from the notification center: {error}");
        }
    }

    fn toast(&self, posted: &link_api::NotificationPostedArgs<'_>) -> windows_core::Result<ToastNotification> {
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(toast_xml(
            posted.app,
            posted.title,
            posted.text,
            &posted.actions,
            posted.alert,
        )))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(tag(posted.device_id, posted.id)))?;
        toast.SetGroup(&HSTRING::from(tag(posted.device_id, "")))?;
        let (device, id) = (posted.device_id.to_owned(), posted.id.to_owned());
        let answers = self.answers.clone();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            let Some(args) = args.as_ref().and_then(|args| args.cast::<ToastActivatedEventArgs>().ok()) else {
                return Ok(());
            };
            let arguments = args.Arguments()?.to_string();
            // A click on the toast itself: the phone has no answer for it.
            let Some(action) = arguments.strip_prefix(ACTION_PREFIX) else { return Ok(()) };
            let reply = reply(&args).unwrap_or_default();
            let answer = Answer { device: device.clone(), id: id.clone(), action: Some(action.to_owned()), reply };
            drop(answers.send(answer));
            Ok(())
        }))?;
        let (device, id) = (posted.device_id.to_owned(), posted.id.to_owned());
        let answers = self.answers.clone();
        toast.Dismissed(&TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(move |_, args| {
            // A timed-out toast moves to the notification center; one hidden here was removed by the phone.
            if args.as_ref().map(ToastDismissedEventArgs::Reason).transpose()?
                == Some(ToastDismissalReason::UserCanceled)
            {
                drop(answers.send(Answer {
                    device: device.clone(),
                    id: id.clone(),
                    action: None,
                    reply: String::new(),
                }));
            }
            Ok(())
        }))?;
        self.notifier.Show(&toast)?;
        Ok(toast)
    }
}

/// Names this program's toasts: Windows shows no toast for an `AppUserModelID` it does not know.
fn register() -> anyhow::Result<()> {
    let (key, name, value) = (wide(REGISTRATION), wide("DisplayName"), wide(DISPLAY_NAME));
    let bytes = u32::try_from(value.len() * 2)?;
    // SAFETY: key, name and value are NUL-terminated and live across the call; bytes counts value with its NUL.
    let status = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), bytes)
    };
    if status != ERROR_SUCCESS {
        anyhow::bail!("registering {AUMID}: {}", std::io::Error::from_raw_os_error(status.cast_signed()));
    }
    Ok(())
}

/// The text typed into the reply field.
fn reply(args: &ToastActivatedEventArgs) -> Option<String> {
    let value = args.UserInput().ok()?.Lookup(&HSTRING::from(REPLY_INPUT)).ok()?;
    Some(value.cast::<IPropertyValue>().ok()?.GetString().ok()?.to_string())
}

/// A toast tag for a phone notification (`id` empty: its device's group). Windows takes at most 64 characters, so a
/// hash (FNV-1a) that a replaced toast and its removal agree on.
fn tag(device: &str, id: &str) -> String {
    let hash = device
        .bytes()
        .chain([0])
        .chain(id.bytes())
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// The toast for a phone notification: title (else the app), text and the app's name; its actions as buttons, a
/// reply action beside a text field.
fn toast_xml(app: &str, title: &str, text: &str, actions: &[(&str, &str, bool)], alert: bool) -> String {
    let (title, attribution) = if title.is_empty() { (app, "") } else { (title, app) };
    let attribution = if attribution.is_empty() {
        String::new()
    } else {
        format!(r#"<text placement="attribution">{}</text>"#, escape(attribution))
    };
    let actions = &actions[..actions.len().min(MAX_ACTIONS)];
    // One field serves the first reply action; Windows places that button beside it.
    let replied = actions.iter().position(|&(_, _, reply)| reply);
    let field = if replied.is_some() { format!(r#"<input id="{REPLY_INPUT}" type="text"/>"#) } else { String::new() };
    let buttons: Vec<String> = actions
        .iter()
        .enumerate()
        .map(|(index, &(id, label, _))| {
            let beside =
                if replied == Some(index) { format!(r#" hint-inputId="{REPLY_INPUT}""#) } else { String::new() };
            format!(r#"<action content="{}" arguments="{ACTION_PREFIX}{}"{beside}/>"#, escape(label), escape(id))
        })
        .collect();
    let buttons = buttons.concat();
    let actions = if actions.is_empty() { String::new() } else { format!("<actions>{field}{buttons}</actions>") };
    let audio = if alert { "" } else { r#"<audio silent="true"/>"# };
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text>{attribution}</binding></visual>{actions}{audio}</toast>"#,
        escape(title),
        escape(text)
    )
}

/// `text` for XML text or a quoted attribute, without the characters XML 1.0 does not allow.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for char in text.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\t' | '\n' | '\r' => escaped.push(char),
            char if char < ' ' || matches!(char, '\u{fffe}' | '\u{ffff}') => {}
            char => escaped.push(char),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' own parser takes the toast: text that needs escaping, a reply action beside the field, at most five
    /// buttons.
    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn a_toast_with_markup_in_its_text_parses_and_keeps_five_buttons_with_the_reply_beside_its_field() {
        let actions: Vec<(&str, &str, bool)> = vec![
            ("open", "Open \"it\"", false),
            ("reply", "Reply", true),
            ("read", "Mark <read>", false),
            ("mute", "Mute & hide", false),
            ("later", "Later", false),
            ("sixth", "Sixth", false),
        ];
        let xml = toast_xml("Chat", "Ann & <Bob>", "a\u{1}b\n\"c\"", &actions, false);
        let document = XmlDocument::new().expect("a document");
        document.LoadXml(&HSTRING::from(xml)).expect("well-formed toast XML");
        let buttons = document.GetElementsByTagName(&HSTRING::from("action")).expect("actions");
        assert_eq!(buttons.Length().expect("a count"), 5);
        let reply = buttons
            .Item(1)
            .expect("the reply button")
            .cast::<windows::Data::Xml::Dom::XmlElement>()
            .expect("an element");
        assert_eq!(reply.GetAttribute(&HSTRING::from("hint-inputId")).expect("hint").to_string(), REPLY_INPUT);
        assert_eq!(reply.GetAttribute(&HSTRING::from("arguments")).expect("arguments").to_string(), "a:reply");
        let texts = document.GetElementsByTagName(&HSTRING::from("text")).expect("texts");
        assert_eq!(texts.Item(0).expect("the title").InnerText().expect("text").to_string(), "Ann & <Bob>");
        assert_eq!(texts.Item(1).expect("the body").InnerText().expect("text").to_string(), "ab\n\"c\"");
        assert_eq!(
            document.GetElementsByTagName(&HSTRING::from("audio")).expect("audio").Length().expect("a count"),
            1
        );
    }

    #[test]
    fn tags_fit_windows_and_differ_by_device_and_key() {
        assert_eq!(tag("phone-a", "1").len(), 16);
        assert_ne!(tag("phone-a", "1"), tag("phone-a", "2"));
        assert_ne!(tag("phone-a", "1"), tag("phone-b", "1"));
        assert_ne!(tag("phone-a1", ""), tag("phone-a", "1"), "the device and key do not run together");
    }
}
