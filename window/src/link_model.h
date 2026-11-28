#pragma once

#include <cstdint>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

// Link desktop app state with no UI or D-Bus in it, so tests drive it directly.
namespace linkapp {

  // The phone's lead media player as Link1 PhonePlayers reports it.
  struct PhonePlayer {
    std::string app;
    std::string title;
    std::string artist;
    std::string art;              // local path or empty
    bool playing = false;
    std::int64_t positionMs = 0;  // true at atMs
    std::int64_t lengthMs = 0;    // 0 unknown
    std::int64_t atMs = 0;        // ms since the Unix epoch
    std::vector<std::string> commands;
    [[nodiscard]] bool takes(std::string_view command) const;
    // Position at nowMs: advances only while playing, clamped to the length.
    [[nodiscard]] std::int64_t position(std::int64_t nowMs) const;
  };

  // One paired phone as Link1 Devices, DeviceInfo, DeviceStatus, DeviceControls and Paused report it.
  struct Phone {
    std::string id;
    std::string name;
    bool connected = false;
    std::string transport;     // wifi or bluetooth while connected
    int battery = -1;          // percent, -1 unknown
    bool charging = false;
    std::optional<int> signal;               // cellular bars 0..4, -1 without service; none until the phone reports
    std::optional<std::uint64_t> storageFree; // bytes of shared storage free, as last reported
    std::int64_t lastSeen = 0; // unix seconds of its last session
    int locked = -1;           // 1, 0, or -1 when the phone does not say
    int dnd = -1;              // same values as locked
    int mediaVolume = -1;      // percent, -1 when the phone does not say
    int wifi = -1;             // Wi-Fi, Bluetooth, torch: as locked; all -1 on a phone without their commands
    int bluetooth = -1;
    int torch = -1;
    std::optional<PhonePlayer> player;
    bool paused = false;
    std::string mic;           // ready, asking or on; empty while the phone cannot give its microphone
    std::string sound;         // ready, asking or on; empty while the phone cannot send its sound
    std::string camera;        // ready, asking or on; empty while the phone cannot be a webcam
  };

  struct Presence {
    enum class Kind : std::uint8_t { NoDaemon, NotPaired, Connected, Locked, Away };
    Kind kind = Kind::NoDaemon;
    Phone phone; // empty for NoDaemon and NotPaired
  };

  // The one place presence comes from: the selected phone while paired, else the first connected one, else the first.
  [[nodiscard]] Presence presenceFrom(bool daemon, const std::vector<Phone>& phones, const std::string& selected = {});

  // This app's pairing window: Idle -> Starting -> Showing -> Idle (finished, or closed by expiry or another client);
  // Starting | Showing -> Failed -> Starting (Try Again); Starting | Showing | Failed -> Idle on Cancel. pairStep rejects
  // every other move and leaves the state as is.
  struct Pair {
    enum class Kind : std::uint8_t { Idle, Starting, Showing, Failed };
    Kind kind = Kind::Idle;
    std::string code;  // Showing only
    std::string uri;   // Showing only
    std::string error; // Failed only
  };

  struct PairEvent {
    enum class Kind : std::uint8_t { Start, Started, Failed, Finished, Closed, Cancel };
    Kind kind = Kind::Start;
    std::string code;  // Started
    std::string uri;   // Started
    std::string error; // Failed
  };

  [[nodiscard]] bool pairStep(Pair& pair, const PairEvent& event);

  // A desktop key on the mirrored screen: printable text without Ctrl, Alt or Super goes as text (any layout, dead
  // keys composed); any other mapped key as an Android KeyEvent code with its meta state; None for an unmapped key.
  struct MirrorKey {
    enum class Kind : std::uint8_t { None, Text, Code };
    Kind kind = Kind::None;
    std::string text;
    std::int32_t code = 0;
    std::int32_t meta = 0;
  };
  struct KeyMods {
    bool shift = false;
    bool ctrl = false;
    bool alt = false;
    bool super = false;
  };
  [[nodiscard]] MirrorKey mirrorKeyFor(std::uint32_t keysym, std::uint32_t utf32, KeyMods mods);
  // The Android code of an XKB keysym regardless of modifiers, 0 when unmapped; a release looks its key up here.
  [[nodiscard]] std::int32_t androidKeyCode(std::uint32_t keysym);

  enum class Transfer : std::uint8_t { Offered, Running, Done, Failed };

  // Offered -> Running -> Done | Failed; Offered -> Done (a small file may finish before any progress) | Failed;
  // Failed -> Running (Try Again). Anything else is rejected.
  [[nodiscard]] bool transferMayMove(Transfer from, Transfer to);

  struct FeedAction {
    std::string id;
    std::string label;
    bool reply = false; // takes text
  };

  // One row of the Waiting page.
  struct FeedItem {
    enum class Kind : std::uint8_t { Text, Notification, MissedCall, Offer, Photos };
    Kind kind = Kind::Notification;
    std::string key; // unique: kind prefix + Link1 id
    std::string deviceId;
    std::string who;
    std::string body;
    std::int64_t atMs = 0;
    std::vector<FeedAction> actions;
    // Offer only.
    std::vector<std::pair<std::string, std::uint64_t>> files;
    Transfer transfer = Transfer::Offered;
    std::uint64_t bytes = 0;
    std::uint64_t total = 0;
    // Notification only: suggested replies, sent as the reply action's text.
    std::vector<std::string> replies;
    // Photos only: the newest new photos, paths for Thumbnail.
    std::vector<std::string> photos;
  };

  // The Waiting feed: newest first, one item per key, at most kMax items (the oldest leave first).
  class Feed {
  public:
    static constexpr std::size_t kMax = 200;
    // Items newer than this belong to "Now"; older ones to "Earlier today".
    static constexpr std::int64_t kNowMs = 60LL * 60 * 1000;

    void upsert(FeedItem item);
    bool remove(const std::string& key);
    // Moves an offer's transfer state; false (and no change) for an unknown key or a rejected move.
    bool moveTransfer(const std::string& key, Transfer to, std::uint64_t bytes = 0, std::uint64_t total = 0);
    void clear() { m_items.clear(); }

    [[nodiscard]] const std::vector<FeedItem>& items() const { return m_items; }
    [[nodiscard]] std::optional<std::size_t> find(const std::string& key) const;
    // Index of the first "Earlier today" item at time nowMs, or items().size() when every item is in "Now".
    [[nodiscard]] std::size_t earlierStart(std::int64_t nowMs) const;

  private:
    std::vector<FeedItem> m_items;
  };

  // Waiting as last shown, so a cold start draws it before Link1 answers. File offers stay out: a transfer lives only in
  // the daemon run that offered it. New photos stay out: their thumbnails need the daemon.
  struct Snapshot {
    std::vector<Phone> phones;
    std::vector<FeedItem> items;
    std::set<std::string> dismissed; // missed calls the user cleared
  };
  [[nodiscard]] std::string encodeSnapshot(const std::vector<Phone>& phones, const Feed& feed,
                                           const std::set<std::string>& dismissed);
  // nullopt for anything encodeSnapshot did not write.
  [[nodiscard]] std::optional<Snapshot> decodeSnapshot(std::string_view text);

  // The Settings page: one phone's switches (Link1 Grants and AutoAccept) and this desktop's.
  struct PhoneSettings {
    bool notifications = false; // grant "notifications"
    bool clipboard = false;     // grant "clipboard"
    bool screen = false;        // grant "screen"
    bool nearby = false;        // grant "nearby": join the phone's hotspot without asking
    bool commands = false;      // grant "commands": its buttons run Link1 Commands here
    bool files = false;
    bool media = false;
    bool input = false;
    bool system = false;
    bool ring = false;
    bool calls = false;
    bool autoAccept = false;
    bool notificationsOff = false; // Link1 NotificationsOff: the phone's notification access is off
    bool inputBlocked = false;     // grant "input" held, but SessionCapabilities lacks "input"
  };
  // Link1 Call: the phone's call while it rings or runs; none while idle.
  struct PhoneCall {
    enum class State : std::uint8_t { Ringing, Active };
    State state = State::Ringing;
    std::string number; // empty when the phone may not read it
    std::string name;
    std::string app;    // the app of a call without a number, such as Messenger
    bool muted = false; // CallAction mute was sent: the ringer stays silent until the call ends
  };
  // Link1 HotspotRequested: the phone asks to join its hotspot; the daemon declines after 30 s without an answer.
  struct HotspotAsk {
    std::string deviceId;
    std::string ssid;
  };
  // One Link1 Commands entry: the button's name on the phone and the shell line it runs here.
  struct Command {
    std::string name;
    std::string line;
    bool operator==(const Command&) const = default;
  };

  struct DesktopSettings {
    bool notificationsWhileLocked = false;
    bool nearbyVisible = false; // Quick Share Visible
    bool localSendVisible = false;
    std::string nearbyName;
    std::vector<Command> commands;
  };

  // One transfer on the Activity page, from the Transfer* signals (Link1 keeps no list).
  struct TransferEntry {
    std::string id;
    std::string deviceId;
    bool incoming = true;
    std::vector<std::pair<std::string, std::uint64_t>> files;
    Transfer state = Transfer::Offered;
    std::string status; // TransferFinished status: done, failed, no-space, too-large, busy, cancelled
    std::vector<std::string> paths; // where an incoming transfer saved its files
    std::vector<std::string> sources; // the local files an outgoing transfer sends: Try again sends them again
    std::uint64_t bytes = 0;
    std::uint64_t total = 0;
    std::int64_t atMs = 0;     // offered or first seen
    std::int64_t endMs = 0;    // finished
    std::int64_t rateFromMs = 0; // the first progress: the rate counts from here
    std::uint64_t rateFromBytes = 0;
  };

  // A phone's clipboard text (ClipboardOffered). Link1 pulls only the latest offer, so the text is kept here.
  struct ClipEntry {
    std::uint64_t id = 0;
    std::string deviceId;
    std::string text;
    std::int64_t atMs = 0;
  };

  // The Activity list, newest first: transfers and clips. A progress or finish for an unknown id is a transfer this
  // desktop sends (SendFiles from any app); a declined offer leaves the list. At most kClips clips stay, in memory only.
  class Activity {
  public:
    static constexpr std::size_t kClips = 20;
    using Row = std::variant<const TransferEntry*, const ClipEntry*>;

    void offered(const std::string& id, const std::string& deviceId,
                 std::vector<std::pair<std::string, std::uint64_t>> files, std::int64_t nowMs);
    // False when the move is rejected (a finished transfer) and nothing changed.
    bool progress(const std::string& id, std::uint64_t bytes, std::uint64_t total, std::int64_t nowMs);
    bool finished(const std::string& id, const std::string& status, std::vector<std::string> paths, std::int64_t nowMs);
    // The SendFiles reply: names the files of an outgoing transfer, before or after its first progress. Never moves
    // a state; an incoming transfer with this id stays as it is.
    void sent(const std::string& id, const std::string& deviceId, std::vector<std::pair<std::string, std::uint64_t>> files,
              std::vector<std::string> sources, std::int64_t nowMs);
    // Removes a failed transfer (sent again under a new id). False for any other state.
    bool forget(const std::string& id);
    [[nodiscard]] static bool retryable(const TransferEntry& e) {
      return !e.incoming && e.state == Transfer::Failed && e.status != "cancelled" && !e.sources.empty();
    }
    void clipped(ClipEntry clip);
    // Removes finished transfers and every clip.
    void clearFinished();

    [[nodiscard]] const std::vector<TransferEntry>& items() const { return m_items; }
    [[nodiscard]] std::size_t rowCount() const { return m_items.size() + m_clips.size(); }
    // Row index of the merged list, newest first; index < rowCount().
    [[nodiscard]] Row row(std::size_t index) const;
    [[nodiscard]] std::size_t finishedCount() const;
    [[nodiscard]] std::size_t offeredCount() const;

  private:
    std::vector<TransferEntry> m_items;
    std::vector<ClipEntry> m_clips; // newest first
  };

  // Bytes per second since the first progress; 0 until a second sample.
  [[nodiscard]] double transferRate(const TransferEntry& entry, std::int64_t nowMs);
  // "2.4 MB", "512 KB", "1.2 GB".
  [[nodiscard]] std::string byteSize(std::uint64_t bytes);

  // One SMS or MMS as Link1 Texts reports it; Delivered means the SMS delivery report came.
  struct Text {
    enum class Status : std::uint8_t { Received, Sent, Sending, Delivered, Failed };
    std::uint64_t id = 0;
    std::uint64_t thread = 0;
    std::string address;
    std::string body;
    std::int64_t atMs = 0;
    Status status = Status::Received;
    std::vector<std::uint64_t> images; // MMS image part ids, for Link1 TextImage
    [[nodiscard]] bool mine() const { return status != Status::Received; }
  };

  struct Contact {
    std::string name;
    std::vector<std::string> numbers;
  };

  struct Thread {
    std::uint64_t id = 0;
    std::string address; // the other side's number
    std::string name;    // the contact name, else the address
    std::string last;    // the newest body
    std::int64_t atMs = 0;
    int unread = 0;
  };

  // A text the user sent that Texts does not hold yet: Sending -> (in Texts: gone) | Failed; Failed -> Sending.
  struct Outgoing {
    enum class State : std::uint8_t { Sending, Failed };
    std::uint64_t serial = 0;
    std::string phone; // thread is this phone's thread id
    std::uint64_t thread = 0;
    std::string body;
    std::int64_t atMs = 0;
    State state = State::Sending;
  };

  // One row of a conversation, oldest first. An MMS gives one row per image, then one for its words.
  struct ConversationRow {
    enum class Kind : std::uint8_t { Day, In, Out, Sending, Failed };
    // Below an Out bubble from Texts: Delivered only on the newest Out row; Sending/NotSent as the phone reports.
    enum class Note : std::uint8_t { None, Delivered, Sending, NotSent };
    Kind kind = Kind::Day;
    std::string text; // the day label or the body
    std::int64_t atMs = 0;
    std::uint64_t key = 0;   // stable per row: text id, outgoing serial, or day
    bool runEnd = false; // the last bubble before the other side, a day line or the end: its time shows below
    std::uint64_t serial = 0; // Sending and Failed: the outgoing serial
    std::uint64_t image = 0;  // an MMS image part: the row is an image bubble
    Note note = Note::None;
  };

  struct CallEntry {
    enum class Kind : std::uint8_t { Incoming, Outgoing, Missed };
    std::string number;
    std::string name; // the phone's contact name, or empty
    std::int64_t atMs = 0;
    std::uint32_t seconds = 0;
    Kind kind = Kind::Incoming;
  };

  // Whether the phone gives its call log (Link1 CallsRefused): "" Ready, not-allowed Off, denied Denied, else Failed.
  enum class CallLog : std::uint8_t { Ready, Off, Denied, Failed };
  [[nodiscard]] CallLog callLogFrom(const std::string& refused);

  // "0:41", "12:48", "1:02:03".
  [[nodiscard]] std::string duration(std::uint32_t seconds);

  // Whole local days from atMs to nowMs, across DST; 0 is today.
  [[nodiscard]] long daysAgo(std::int64_t atMs, std::int64_t nowMs);
  // "Today", "Yesterday", a weekday within the last week ("Mon" when short), else "12 Mar" (and the year when it is not
  // this year).
  [[nodiscard]] std::string dayLabel(std::int64_t atMs, std::int64_t nowMs, bool shortWeekday = false);

  // "Last seen 12 min ago" from unix seconds; "Not connected" when the phone was never seen.
  [[nodiscard]] std::string lastSeenText(std::int64_t lastSeenS, std::int64_t nowS);

  // Match key of a phone number: its last 9 digits, so "+49 151 2345 6789" and "0151 23456789" meet; a name-like
  // sender (no digits) keys as itself.
  [[nodiscard]] std::string numberKey(const std::string& address);

  // The Messages page: threads from the newest texts Link1 keeps, unread from TextReceived since start (Link1 has no
  // read state), and texts the user sent until Texts holds them.
  class Messages {
  public:
    enum class Load : std::uint8_t { Loading, Ready };

    // The phone's texts, any order. Loading -> Ready; drops each outgoing text the new list holds.
    // A text id, a thread id and a pending send mean something only on the phone they came from.
    void setTexts(const std::string& phone, std::vector<Text> texts);
    void setContacts(const std::vector<Contact>& contacts);
    // A new text arrived: unread until its thread is on screen.
    void received(std::uint64_t textId);
    // The thread on screen (nullopt: none); its texts count as read now and while it stays on screen.
    void view(std::optional<std::uint64_t> thread);
    // Returns the outgoing serial; 0 (and no change) for an unknown thread or an empty body.
    std::uint64_t send(std::uint64_t thread, std::string body, std::int64_t atMs);
    // Sending -> Failed; false for an unknown serial or another state.
    bool sendFailed(std::uint64_t serial);
    // Failed -> Sending; false otherwise.
    bool retry(std::uint64_t serial);

    [[nodiscard]] Load load() const { return m_load; }
    // Newest first.
    [[nodiscard]] const std::vector<Thread>& threads() const { return m_threads; }
    [[nodiscard]] const Thread* thread(std::uint64_t id) const;
    [[nodiscard]] const Outgoing* outgoing(std::uint64_t serial) const;
    [[nodiscard]] int unread() const;
    // The contact name for a number, else the number.
    [[nodiscard]] std::string nameFor(const std::string& address) const;
    // What to dial for "Name or number": the digits, + * # of a number, else the first number of the first contact
    // whose name starts with the query (any case); empty when neither.
    [[nodiscard]] std::string numberFor(const std::string& query) const;
    // Contacts with a number whose name starts with the query (any case), in phone order, at most `limit`.
    [[nodiscard]] std::vector<Contact> matches(const std::string& query, std::size_t limit) const;
    // The thread with this number, if one exists.
    [[nodiscard]] std::optional<std::uint64_t> threadFor(const std::string& address) const;
    [[nodiscard]] std::vector<ConversationRow> conversation(std::uint64_t thread, std::int64_t nowMs) const;

  private:
    void rebuild();
    void readViewed();

    Load m_load = Load::Loading;
    std::vector<Text> m_texts;
    std::map<std::string, std::string> m_names; // numberKey -> contact name
    std::vector<Contact> m_contacts;
    std::set<std::uint64_t> m_unread;           // text ids
    std::optional<std::uint64_t> m_viewing;
    std::vector<Outgoing> m_outgoing;
    std::uint64_t m_serial = 0;
    std::string m_phone;
    std::vector<Thread> m_threads;
  };

  // The full file of the photo the viewer shows, read by ReadFile into the cache: Loading -> Ready | Failed.
  struct PhotoFile {
    enum class Kind : std::uint8_t { Loading, Ready, Failed };
    Kind kind = Kind::Loading;
    std::string phonePath;
    std::string local;      // the cache file once Ready
    std::uint64_t size = 0; // bytes once Ready
  };

  // One photo or video of a Link1 PhotosPage.
  struct Photo {
    std::string path; // for Thumbnail and SavePhoto
    std::string name;
    std::int64_t atS = 0;
    std::optional<std::uint32_t> videoS; // a video's length in seconds; nullopt for a photo
  };

  // The Photos page, filled page by page (Link1 PhotosPage) for one media filter.
  // Load: Idle | settled -> Loading (first page) -> Ready | Denied | Unsupported | Failed;
  // Ready with more on the phone -> More (next page, items stay) -> Ready. A new media filter drops the items and any
  // answer in flight (by serial) and goes back to Idle. Save: Idle | Done -> Saving -> Done once every photo of the
  // batch answered. The selection is a subset of items.
  class Photos {
  public:
    enum class Load : std::uint8_t { Idle, Loading, More, Ready, Denied, Unsupported, Failed };
    enum class Save : std::uint8_t { Idle, Saving, Done };
    enum class Media : std::uint8_t { All, Photos, Videos };
    // One PhotosPage call; its answer goes back with the serial.
    struct Ask {
      std::uint64_t serial = 0;
      std::uint32_t offset = 0;
      Media media = Media::All;
    };
    static constexpr std::uint16_t kCount = 64;

    [[nodiscard]] Load load() const { return m_load; }
    [[nodiscard]] Media media() const { return m_media; }
    [[nodiscard]] const std::vector<Photo>& items() const { return m_items; }
    // Items of the media filter on the phone, as its last answer gave.
    [[nodiscard]] std::uint32_t total() const { return m_total; }
    [[nodiscard]] bool hasMore() const { return m_next < m_total; }
    // Any state but Loading -> Loading from offset 0.
    // A new phone drops what the old one listed and its page in flight.
    [[nodiscard]] std::optional<Ask> startLoad(const std::string& device);
    [[nodiscard]] const std::string& device() const { return m_device; }
    // Ready with more on the phone -> More from the next offset.
    [[nodiscard]] std::optional<Ask> startMore();
    // Drops the items and the selection and goes Idle when media differs; false when it is the shown filter.
    bool setMedia(Media media);
    // Loading | More of the same serial -> Ready: a first page replaces the items, a next page appends the photos
    // not yet listed. The selection keeps the photos still listed.
    bool loaded(std::uint64_t serial, std::uint32_t total, std::vector<Photo> page);
    // Loading -> Denied | Unsupported | Failed; More -> Ready with the items kept, so the next scroll asks again.
    bool failed(std::uint64_t serial, Load why);

    void toggle(std::size_t index);
    void clearSelection() { m_selected.clear(); }
    [[nodiscard]] bool selected(std::size_t index) const;
    [[nodiscard]] std::size_t selectedCount() const { return m_selected.size(); }

    // Saving -> starts a batch of the selected paths and clears the selection; empty when rejected.
    [[nodiscard]] std::vector<std::string> startSave();
    void saveAnswered(bool ok);
    [[nodiscard]] Save save() const { return m_save; }
    [[nodiscard]] std::size_t saved() const { return m_saved; }
    [[nodiscard]] std::size_t saveFailed() const { return m_saveFailed; }

    // First index of each day ("Today", "Yesterday", ...) and its label.
    [[nodiscard]] std::vector<std::pair<std::size_t, std::string>> days(std::int64_t nowMs) const;

  private:
    // Drops the listing and any page in flight.
    void forget();

    Load m_load = Load::Idle;
    Media m_media = Media::All;
    std::string m_device;
    std::uint64_t m_serial = 0;
    std::uint32_t m_total = 0;
    std::uint32_t m_next = 0; // offset of the next page
    std::vector<Photo> m_items;
    std::set<std::string> m_listed;   // paths of m_items
    std::set<std::string> m_selected; // paths
    Save m_save = Save::Idle;
    std::size_t m_pending = 0;
    std::size_t m_saved = 0;
    std::size_t m_saveFailed = 0;
  };

  // n with its thousands set apart by a narrow space: "1 284".
  [[nodiscard]] std::string groupedCount(std::uint64_t n);

  inline constexpr std::size_t kStripPhotos = 6;
  // Waiting's "N new photos" from photos newest first (as Photos::items): those taken today (local time) after seenS;
  // nullopt when none.
  [[nodiscard]] std::optional<FeedItem> newPhotosItem(const std::string& deviceId, const std::vector<Photo>& photos,
                                                      std::int64_t seenS, std::int64_t nowMs);

  // One ListFiles entry.
  struct FileEntry {
    std::string name;
    bool folder = false;
    std::uint64_t size = 0;
    std::int64_t mtimeS = 0;
  };

  // A phone file this app saves into the downloads folder with ReadFile.
  struct FileSave {
    enum class State : std::uint8_t { Running, Done, Failed };
    State state = State::Running;
    std::uint64_t got = 0;
    std::uint64_t total = 0;
    std::string saved; // Done: the local path
  };

  // The Files page: one phone folder ("/" lists the roots). Every navigation starts a load with a new serial; a
  // reply for an older serial is dropped, so a slow folder never replaces the one on screen.
  // Load: Loading -> Ready | Failed{error}. Saves: none -> Running -> Done | Failed; Failed | Done -> Running.
  class Files {
  public:
    enum class Load : std::uint8_t { Idle, Loading, Ready, Failed };

    [[nodiscard]] const std::string& path() const { return m_path; }
    [[nodiscard]] Load load() const { return m_load; }
    [[nodiscard]] const std::string& error() const { return m_error; }
    // Folders first, then by name without case.
    [[nodiscard]] const std::vector<FileEntry>& entries() const { return m_entries; }
    // "Phone", then each folder of the path.
    [[nodiscard]] std::vector<std::string> crumbs() const;
    [[nodiscard]] std::string childPath(const std::string& name) const;

    // Starts a load of `path` (the same one reloads); returns its serial.
    std::uint64_t go(const std::string& path);
    // The parent of the path; "/" stays.
    [[nodiscard]] std::string parent() const;
    // The path of crumb `index` (0 = "/").
    [[nodiscard]] std::string crumbPath(std::size_t index) const;
    bool listed(std::uint64_t serial, std::vector<FileEntry> entries);
    bool failed(std::uint64_t serial, std::string error);

    [[nodiscard]] const FileSave* save(const std::string& path) const;
    bool saveStarted(const std::string& path, std::uint64_t total);
    bool saveProgress(const std::string& path, std::uint64_t got);
    bool saveDone(const std::string& path, std::string saved);
    bool saveFailed(const std::string& path);

  private:
    std::string m_path = "/";
    Load m_load = Load::Idle;
    std::string m_error;
    std::uint64_t m_serial = 0;
    std::vector<FileEntry> m_entries;
    std::map<std::string, FileSave> m_saves; // phone path -> its save
  };

  // Desktop files copied into the open phone folder with WriteFile, one after another. Idle -> Copying -> Idle (all
  // copied; the folder lists again) | Failed; Copying -> Idle on Cancel (the partial file is deleted). A failed
  // DeleteFile also shows as Failed; Dismiss: Failed -> Idle.
  struct FileOp {
    enum class Kind : std::uint8_t { Idle, Copying, Failed };
    Kind kind = Kind::Idle;
    std::string name;       // Copying: the file on its way
    std::size_t index = 0;  // Copying: its place, from 0
    std::size_t count = 0;  // Copying: files in this copy
    std::uint64_t sent = 0; // Copying: bytes of all files
    std::uint64_t total = 0;
    std::string error; // Failed
  };

  // The Mirror page. Start: Idle | Ended | Failed -> Asking (OpenMirror sent; the phone's user allows capture within
  // 60 s) -> Showing -> Ended (the phone stopped) | Failed; Stop: Asking | Showing -> Idle. Each Start opens a new
  // session; an event of an older session is rejected, so a late reply never revives a stopped mirror.
  struct Mirror {
    enum class Kind : std::uint8_t { Idle, Asking, Showing, Ended, Failed };
    Kind kind = Kind::Idle;
    std::uint64_t session = 0;
    std::uint32_t width = 0;  // Showing: the phone's video size
    std::uint32_t height = 0;
    std::string error;        // Failed only
  };

  struct MirrorEvent {
    enum class Kind : std::uint8_t { Start, Opened, Ended, Failed, Stop };
    Kind kind = Kind::Start;
    std::uint64_t session = 0; // Opened, Ended, Failed
    std::uint32_t width = 0;   // Opened
    std::uint32_t height = 0;
    std::string error;         // Failed
  };

  [[nodiscard]] bool mirrorStep(Mirror& mirror, const MirrorEvent& event);

  // Settings › Commands: the buttons the phone shows under "Run on <desktop>". The list is local truth while a send
  // is in flight or failed; otherwise Link1's Commands property replaces it. Mode: Idle | Editing (one row open, its
  // draft apart from the list; index == list size for a new command) | Removed (the last removed row offers Undo).
  // Only a commit (close, open another row, add, remove, undo) queues a SetCommands; a draft with an empty name or
  // line never reaches the list. Replies carry the send's serial; an older reply changes nothing.
  class CommandsEditor {
  public:
    static constexpr std::size_t kMax = 64;        // Link1 SetCommands rejects more
    static constexpr std::size_t kMaxName = 512;   // bytes; Link1 MAX_TITLE_LEN
    enum class Mode : std::uint8_t { Idle, Editing, Removed };
    enum class Sync : std::uint8_t { Synced, Sending, Failed };
    enum class Invalid : std::uint8_t { None, Name, Line, Full }; // Full: a new command while the list holds kMax

    bool listed(std::vector<Command> commands);
    bool open(std::size_t index);
    bool add();
    bool editName(std::string name);
    bool editLine(std::string line);
    bool close();
    bool remove();
    bool undo();
    // Drag reorder: the command at `from` lands at `to`; an open row commits first.
    bool move(std::size_t from, std::size_t to);
    // move() only when `from` still holds `expected` (a drag started on an older list).
    bool moveIf(std::size_t from, std::size_t to, const Command& expected);
    // The list to send with its serial, once per commit.
    [[nodiscard]] std::optional<std::pair<std::uint64_t, std::vector<Command>>> takeSend();
    bool sent(std::uint64_t serial, std::optional<std::string> error);
    bool retry();

    [[nodiscard]] const std::vector<Command>& list() const { return m_list; }
    [[nodiscard]] Mode mode() const { return m_mode; }
    [[nodiscard]] std::size_t index() const { return m_index; } // Editing: the open row; Removed: where it was
    [[nodiscard]] const Command& draft() const { return m_draft; }   // Editing: the fields; Removed: the removed one
    [[nodiscard]] Invalid invalid() const { return m_invalid; }      // set when a commit was refused
    [[nodiscard]] Sync sync() const { return m_sync; }
    [[nodiscard]] const std::string& error() const { return m_error; } // Failed only
    [[nodiscard]] bool canAdd() const { return m_list.size() < kMax; }

  private:
    bool commit();
    void queue();
    std::vector<Command> m_list;
    Mode m_mode = Mode::Idle;
    std::size_t m_index = 0;
    Command m_draft;
    Command m_opened; // Editing an existing row: its command when opened, to find it again after listed()
    Invalid m_invalid = Invalid::None;
    Sync m_sync = Sync::Synced;
    std::string m_error;
    std::uint64_t m_serial = 0;
    bool m_queued = false;
  };

  // Search, filter in place: per page, the indexes of loaded items whose name (a call's number, a thread's newest
  // text) holds the query, ASCII case-folded, in model order.
  struct SearchHits {
    std::vector<std::size_t> threads;
    std::vector<std::size_t> calls;
    std::vector<std::size_t> files;
    std::vector<std::size_t> photos;
  };
  SearchHits searchHits(std::string_view query, const std::vector<Thread>& threads, const std::vector<CallEntry>& calls,
                        const std::vector<FileEntry>& files, const std::vector<Photo>& photos);

} // namespace linkapp
