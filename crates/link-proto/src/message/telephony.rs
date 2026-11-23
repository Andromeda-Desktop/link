use super::*;

pub const MAX_NUMBER_LEN: usize = 64;
pub const MAX_CALLER_LEN: usize = 128;
/// Contacts per `contacts` page, so a page of the longest fits a control frame.
pub const MAX_CONTACTS_PAGE: usize = 64;
pub const MAX_CONTACT_NUMBERS: usize = 8;

/// Why the phone refused a contacts or text message request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessagesRefusal {
    /// The phone's messages switch for this desktop is off.
    NotAllowed,
    /// Android does not let the app read or send them.
    Denied,
    Failed,
}

impl MessagesRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAllowed => "not-allowed",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }
}

/// Asks for a page of the phone's contacts; answered by `contacts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactsList {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub name: String,
    pub numbers: Vec<String>,
}

/// One page of contacts; `next` is the cursor for the rest. With `refused`, no contacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contacts {
    pub req: u64,
    pub contacts: Vec<Contact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<MessagesRefusal>,
}

/// Texts per `texts` page, so a page of the longest fits a control frame.
pub const MAX_TEXTS_PAGE: usize = 16;
/// Bytes of a text's body; the phone cuts longer ones when listing.
pub const MAX_TEXT_BODY: usize = 2048;

/// Asks for a page of the phone's texts, newest first; answered by `texts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextsList {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u32>,
}

/// Image parts per text, so a page of the longest still fits a control frame.
pub const MAX_TEXT_PARTS: usize = 4;
/// An MMS text's `id` is its MMS row id plus this; an SMS keeps its row id, so the two never collide.
pub const MMS_ID: u64 = 1 << 62;

/// One text message (SMS or MMS); `date` in Unix milliseconds, `sent` for one this phone sent. `state` and `parts`
/// need [`TEXTS_MMS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub id: u64,
    pub thread: u64,
    pub address: String,
    pub body: String,
    pub date: u64,
    pub sent: bool,
    /// Only on a sent text; none means sent and no delivery report yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TextState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<TextPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextState {
    Sending,
    Delivered,
    Failed,
}

/// An MMS image part, fetched with `text-part`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPart {
    pub id: u64,
    pub mime: String,
}

/// Asks for a PNG of an MMS image part at most `px` on its longer side; answered like `fs-thumb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPartThumb {
    pub req: u64,
    pub part: u64,
    pub px: u16,
}

/// One page of texts; `next` is the cursor for the rest. With `refused`, no texts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Texts {
    pub req: u64,
    pub texts: Vec<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<MessagesRefusal>,
}

/// Asks the phone to send a text; answered by `text-sent` once Android reports it sent or failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSend {
    pub req: u64,
    pub to: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSent {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<MessagesRefusal>,
}

/// Calls per `calls` page, so a page of the longest fits a control frame.
pub const MAX_CALLS_PAGE: usize = 64;

/// Asks for a page of the phone's call log, newest first; answered by `calls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallsList {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallKind {
    Incoming,
    Outgoing,
    Missed,
}

impl CallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
            Self::Missed => "missed",
        }
    }
}

/// One call of the phone's log; `date` in Unix milliseconds, `duration` in seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggedCall {
    pub number: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub date: u64,
    pub duration: u32,
    pub kind: CallKind,
}

/// One page of the call log; `next` is the cursor for the rest. With `refused`, no calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calls {
    pub req: u64,
    pub calls: Vec<LoggedCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<MessagesRefusal>,
}

/// Asks the phone to place a call; the audio stays on the phone. Answered by `dialed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dial {
    pub req: u64,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dialed {
    pub req: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<MessagesRefusal>,
}

/// Digits, `+`, `*` and `#`: what a phone may dial.
pub fn dialable(number: &str) -> bool {
    sized(number, 1, MAX_NUMBER_LEN) && number.bytes().all(|b| b.is_ascii_digit() || b"+*#".contains(&b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    Ringing,
    Active,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub state: CallState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// The contact's name, when the phone may read contacts; for an app call, the caller the app shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The app of a call without a number, such as Messenger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallActionKind {
    /// Silences the ringer until the call ends.
    Mute,
    /// Ends the ringing call.
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallAction {
    pub action: CallActionKind,
}

impl CallState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ringing => "ringing",
            Self::Active => "active",
            Self::Idle => "idle",
        }
    }
}

impl CallActionKind {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "mute" => Some(Self::Mute),
            "decline" => Some(Self::Decline),
            _ => None,
        }
    }
}
