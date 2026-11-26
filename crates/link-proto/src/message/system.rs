use super::*;

/// Both hellos carry this among their `addresses` when the peer takes `desktop-status` and `system-command`; older
/// peers close a session on those types.
pub const SYSTEM: &str = "feature:system";
/// Both hellos carry this, with [`SYSTEM`], when the peer takes the status fields and commands after volume and mute,
/// and `system-result`.
pub const SYSTEM_CONTROL: &str = "feature:system-control";

/// Desktop to phone, on connect and after each change: the desktop's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopStatus {
    /// The default output's volume, percent. Volume and mute are absent only without a volume control, and then only
    /// to a phone with [`SYSTEM_CONTROL`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// [`SYSTEM_CONTROL`] from here on. Percent; absent without a battery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charging: Option<bool>,
    /// Percent; absent without a backlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dnd: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<PowerProfile>,
    /// Every output off; absent when the desktop cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_off: Option<bool>,
}

impl DesktopStatus {
    pub(super) fn valid(&self) -> bool {
        [self.volume, self.battery, self.brightness].into_iter().flatten().all(|percent| percent <= 100)
    }

    /// The status for a phone without [`SYSTEM_CONTROL`], which needs volume and mute; None without them.
    #[must_use]
    pub fn without_control(self) -> Option<Self> {
        Some(Self { volume: Some(self.volume?), muted: Some(self.muted?), ..Self::default() })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerProfile {
    PowerSaver,
    Balanced,
    Performance,
}

impl PowerProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemCommandKind {
    /// `value` is the percent, 0 to 100.
    SetVolume,
    /// `value` is 1 to mute, 0 to unmute.
    SetMute,
    /// [`SYSTEM_CONTROL`] from here on. To the desktop; `value` is the percent.
    SetBrightness,
    Lock,
    Suspend,
    ScreenOff,
    /// To a desktop that lists `screen-on`.
    ScreenOn,
    /// Either way; `value` is 1 for on.
    SetDnd,
    /// To the desktop; `value` is 0 power-saver, 1 balanced, 2 performance.
    SetProfile,
    /// To the phone; `value` is the percent.
    SetMediaVolume,
    /// To the phone; `value` is 0 silent, 1 vibrate, 2 normal.
    SetRinger,
    /// [`PHONE_TOGGLES`] from here on. To the phone; `value` is 1 for on.
    SetWifi,
    SetBluetooth,
    SetTorch,
}

impl SystemCommandKind {
    /// False for the commands only a desktop sends.
    pub fn to_desktop(self) -> bool {
        !self.phone_only()
    }

    fn phone_only(self) -> bool {
        matches!(self, Self::SetMediaVolume | Self::SetRinger | Self::SetWifi | Self::SetBluetooth | Self::SetTorch)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SetVolume => "set-volume",
            Self::SetMute => "set-mute",
            Self::SetBrightness => "set-brightness",
            Self::Lock => "lock",
            Self::Suspend => "suspend",
            Self::ScreenOff => "screen-off",
            Self::ScreenOn => "screen-on",
            Self::SetDnd => "set-dnd",
            Self::SetProfile => "set-profile",
            Self::SetMediaVolume => "set-media-volume",
            Self::SetRinger => "set-ringer",
            Self::SetWifi => "set-wifi",
            Self::SetBluetooth => "set-bluetooth",
            Self::SetTorch => "set-torch",
        }
    }

    /// True for the commands a desktop may send.
    pub fn to_phone(self) -> bool {
        self.phone_only() || self == Self::SetDnd
    }

    /// True for the commands only a phone with [`PHONE_TOGGLES`] takes.
    pub fn toggle(self) -> bool {
        matches!(self, Self::SetWifi | Self::SetBluetooth | Self::SetTorch)
    }

    /// The largest `value`, or `None` for a command without one.
    pub fn max_value(self) -> Option<u8> {
        match self {
            Self::SetVolume | Self::SetBrightness | Self::SetMediaVolume => Some(100),
            Self::SetMute | Self::SetDnd | Self::SetWifi | Self::SetBluetooth | Self::SetTorch => Some(1),
            Self::SetProfile | Self::SetRinger => Some(2),
            Self::Lock | Self::Suspend | Self::ScreenOff | Self::ScreenOn => None,
        }
    }
}

/// Phone to desktop, while the desktop's system switch for the phone is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCommand {
    pub command: SystemCommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<u8>,
}

impl SystemCommand {
    pub(super) fn valid(&self) -> bool {
        match (self.command.max_value(), self.value) {
            (Some(max), Some(value)) => value <= max,
            (None, None) => true,
            _ => false,
        }
    }
}

/// [`SYSTEM_CONTROL`]: the answer to the `system-command` whose message id is `command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemResult {
    pub command: u64,
    /// For the user, when the command was not done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub const MAX_SYSTEM_ERROR_LEN: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputAction {
    /// The pointer by (dx, dy) logical pixels.
    Move,
    /// The pointer to (dx, dy) in 1/[`MIRROR_UNIT`] of the remote-control picture's width and height.
    Point,
    /// A scroll by (dx, dy) logical pixels; positive is down and right.
    Scroll,
    Press,
    Release,
    Click,
    Key,
    /// `text` typed into the focused window.
    Text,
    /// A shortcut in `text`: modifiers and one key joined by `+`, as `super+t` or `ctrl+shift+escape`. Only to a
    /// desktop whose capabilities list `shortcuts`.
    Shortcut,
    /// Every finger on the phone's touchpad now, in `contacts`; none when the last one lifts. Only to a desktop whose
    /// capabilities list `touchpad`.
    Touch,
}

/// One finger on the phone's touchpad: a stable `id` while it stays down, and its position from the pad's top left in
/// 1/100 mm, so the desktop's touchpad settings see real distances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TouchContact {
    pub id: u8,
    pub x: u32,
    pub y: u32,
}

/// The most fingers in one [`InputAction::Touch`].
pub const MAX_TOUCH_CONTACTS: usize = 5;
/// The largest [`TouchContact`] id.
pub const MAX_TOUCH_ID: u8 = 9;
/// The largest touch position: 300 mm in 1/100 mm.
pub const MAX_TOUCH_POSITION: u32 = 30_000;

/// The longest [`InputAction::Shortcut`], in bytes.
pub const MAX_SHORTCUT_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputKey {
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F5,
}

/// Phone to desktop: the phone as the desktop's touchpad and keyboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteInput {
    pub action: InputAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dx: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dy: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<InputButton>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<InputKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<TouchContact>>,
}

pub const MAX_INPUT_DELTA: i32 = 10_000;

impl RemoteInput {
    pub(super) fn valid(&self) -> bool {
        let delta =
            |value: Option<i32>| value.is_some_and(|value| (-MAX_INPUT_DELTA..=MAX_INPUT_DELTA).contains(&value));
        match self.action {
            InputAction::Move | InputAction::Scroll => delta(self.dx) && delta(self.dy),
            InputAction::Point => {
                let fraction = |value: Option<i32>| value.is_some_and(|value| (0..=MIRROR_UNIT).contains(&value));
                fraction(self.dx) && fraction(self.dy)
            }
            InputAction::Press | InputAction::Release | InputAction::Click => self.button.is_some(),
            InputAction::Key => self.key.is_some(),
            InputAction::Text => self.text.as_ref().is_some_and(|text| sized(text, 1, MAX_MIRROR_TEXT_LEN)),
            InputAction::Shortcut => self.text.as_ref().is_some_and(|text| sized(text, 1, MAX_SHORTCUT_LEN)),
            InputAction::Touch => self.contacts.as_ref().is_some_and(|contacts| {
                let ids: std::collections::HashSet<u8> = contacts.iter().map(|contact| contact.id).collect();
                contacts.len() <= MAX_TOUCH_CONTACTS
                    && ids.len() == contacts.len()
                    && contacts.iter().all(|contact| {
                        contact.id <= MAX_TOUCH_ID && contact.x <= MAX_TOUCH_POSITION && contact.y <= MAX_TOUCH_POSITION
                    })
            }),
        }
    }
}
