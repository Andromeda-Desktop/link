use super::*;

pub const MAX_NOTIFICATION_ID_LEN: usize = 256;
pub const MAX_APP_LEN: usize = 128;
pub const MAX_TITLE_LEN: usize = 512;
/// Also the longest reply.
pub const MAX_TEXT_LEN: usize = 4096;
pub const MAX_ICON_LEN: usize = 16 * 1024;
pub const MAX_ACTIONS: usize = 3;
pub const MAX_ACTION_LEN: usize = 64;
/// Suggested replies of one notification, each at most [`MAX_ACTION_LEN`] bytes.
pub const MAX_REPLIES: usize = 3;
/// Notifications in a snapshot or a pull: 200 of the longest ids fit a frame.
pub const MAX_SNAPSHOT: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationButton {
    pub id: String,
    pub label: String,
    /// Whether the action takes reply text (Android `RemoteInput`).
    pub reply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPosted {
    pub id: String,
    pub app: String,
    pub title: String,
    pub text: String,
    /// PNG.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    pub icon: Option<Vec<u8>>,
    pub actions: Vec<NotificationButton>,
    /// Android's `FLAG_ONLY_ALERT_ONCE`: an update replaces the shown one without a sound or a popup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alert_once: bool,
    /// Suggested replies (Android's `RemoteInput` choices, then the system's smart replies); a pick goes back as the
    /// reply action's `reply_text`. Only to desktops with [`NOTIFICATION_REPLIES`], and only with a reply action.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRemoved {
    pub id: String,
}

/// Every notification on show on the phone, sent on connect; the desktop removes the ones missing from it and pulls
/// the new and changed ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationSnapshot {
    pub notifications: Vec<NotificationDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDigest {
    pub id: String,
    /// [`NotificationPosted::digest`].
    pub hash: u64,
}

/// The desktop asks for these notifications in full, as `notification-posted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPull {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAction {
    pub id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationDismiss {
    pub id: String,
}

impl NotificationPosted {
    /// FNV-1a 64 over what the desktop shows, so that both sides compute it alike; `alert_once` is not content.
    pub fn digest(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for chunk in [&(bytes.len() as u64).to_be_bytes()[..], bytes] {
                for byte in chunk {
                    hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
                }
            }
        };
        for field in [&self.id, &self.app, &self.title, &self.text] {
            feed(field.as_bytes());
        }
        feed(self.icon.as_deref().unwrap_or_default());
        for action in &self.actions {
            feed(action.id.as_bytes());
            feed(action.label.as_bytes());
            feed(&[u8::from(action.reply)]);
        }
        for reply in &self.replies {
            feed(reply.as_bytes());
        }
        hash
    }

    pub(super) fn valid(&self) -> bool {
        notification_id(&self.id)
            && sized(&self.app, 1, MAX_APP_LEN)
            && sized(&self.title, 0, MAX_TITLE_LEN)
            && sized(&self.text, 0, MAX_TEXT_LEN)
            && self.icon.as_ref().is_none_or(|icon| (1..=MAX_ICON_LEN).contains(&icon.len()))
            && self.actions.len() <= MAX_ACTIONS
            && self
                .actions
                .iter()
                .all(|action| sized(&action.id, 1, MAX_ACTION_LEN) && sized(&action.label, 1, MAX_ACTION_LEN))
            && self.replies.len() <= MAX_REPLIES
            && self.replies.iter().all(|reply| sized(reply, 1, MAX_ACTION_LEN))
            && (self.replies.is_empty() || self.actions.iter().any(|action| action.reply))
    }
}

impl NotificationAction {
    pub(super) fn valid(&self) -> bool {
        notification_id(&self.id)
            && sized(&self.action, 1, MAX_ACTION_LEN)
            && self.reply_text.as_ref().is_none_or(|reply| sized(reply, 1, MAX_TEXT_LEN))
    }
}
