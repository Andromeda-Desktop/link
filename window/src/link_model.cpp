#include "link_model.h"
#include "i18n/i18n.h"

#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <ctime>
#include <format>
#include <functional>
#include <nlohmann/json.hpp>
#include <tuple>
#include <xkbcommon/xkbcommon-keysyms.h>

namespace linkapp {

  bool PhonePlayer::takes(std::string_view command) const { return std::ranges::find(commands, command) != commands.end(); }

  std::int64_t PhonePlayer::position(std::int64_t nowMs) const {
    const std::int64_t at = playing ? positionMs + std::max<std::int64_t>(0, nowMs - atMs) : positionMs;
    return lengthMs > 0 ? std::min(at, lengthMs) : at;
  }

  Presence presenceFrom(bool daemon, const std::vector<Phone>& phones, const std::string& selected) {
    using K = Presence::Kind;
    if (!daemon) return {K::NoDaemon, {}};
    if (phones.empty()) return {K::NotPaired, {}};
    auto live = std::ranges::find(phones, selected, &Phone::id);
    if (live == phones.end()) live = std::ranges::find_if(phones, [](const Phone& p) { return p.connected; });
    if (live == phones.end()) return {K::Away, phones.front()};
    if (!live->connected) return {K::Away, *live};
    return {live->locked == 1 ? K::Locked : K::Connected, *live};
  }

  namespace {
    constexpr int kSnapshotVersion = 1;
  } // namespace

  std::string encodeSnapshot(const std::vector<Phone>& phones, const Feed& feed, const std::set<std::string>& dismissed) {
    using nlohmann::json;
    json out{{"version", kSnapshotVersion}, {"phones", json::array()}, {"items", json::array()}, {"dismissed", dismissed}};
    for (const Phone& p : phones) {
      out["phones"].push_back({{"id", p.id}, {"name", p.name}, {"connected", p.connected}, {"transport", p.transport},
                               {"battery", p.battery}, {"charging", p.charging}, {"lastSeen", p.lastSeen},
                               {"locked", p.locked}, {"dnd", p.dnd}, {"paused", p.paused}});
    }
    for (const FeedItem& item : feed.items()) {
      if (item.kind == FeedItem::Kind::Offer || item.kind == FeedItem::Kind::Photos) continue;
      json actions = json::array();
      for (const FeedAction& a : item.actions) actions.push_back({{"id", a.id}, {"label", a.label}, {"reply", a.reply}});
      out["items"].push_back({{"kind", static_cast<int>(item.kind)}, {"key", item.key}, {"deviceId", item.deviceId},
                              {"app", item.app}, {"who", item.who}, {"body", item.body}, {"atMs", item.atMs}, {"actions", std::move(actions)}});
    }
    return out.dump();
  }

  std::optional<Snapshot> decodeSnapshot(std::string_view text) {
    const nlohmann::json in = nlohmann::json::parse(text, nullptr, false);
    try {
      if (in.is_discarded() || in.at("version").get<int>() != kSnapshotVersion) return std::nullopt;
      Snapshot snapshot;
      for (const auto& p : in.at("phones")) {
        snapshot.phones.push_back({.id = p.at("id"), .name = p.at("name"), .connected = p.at("connected"),
                                   .transport = p.at("transport"), .battery = p.at("battery"), .charging = p.at("charging"),
                                   .lastSeen = p.at("lastSeen"), .locked = p.at("locked"), .dnd = p.at("dnd"),
                                   .paused = p.at("paused")});
      }
      for (const auto& i : in.at("items")) {
        const int kind = i.at("kind");
        if (kind < 0 || kind >= static_cast<int>(FeedItem::Kind::Offer)) return std::nullopt;
        FeedItem item{.kind = static_cast<FeedItem::Kind>(kind), .key = i.at("key"), .deviceId = i.at("deviceId"),
                      .app = i.value("app", ""), .who = i.at("who"), .body = i.at("body"), .atMs = i.at("atMs")};
        for (const auto& a : i.at("actions")) item.actions.push_back({a.at("id"), a.at("label"), a.at("reply")});
        snapshot.items.push_back(std::move(item));
      }
      snapshot.dismissed = in.at("dismissed").get<std::set<std::string>>();
      return snapshot;
    } catch (const nlohmann::json::exception&) {
      return std::nullopt;
    }
  }

  bool pairStep(Pair& pair, const PairEvent& event) {
    using K = Pair::Kind;
    using E = PairEvent::Kind;
    switch (event.kind) {
    case E::Start:
      if (pair.kind != K::Idle && pair.kind != K::Failed) return false;
      pair = {.kind = K::Starting};
      return true;
    case E::Started:
      if (pair.kind != K::Starting || event.code.empty()) return false;
      pair = {.kind = K::Showing, .code = event.code, .uri = event.uri};
      return true;
    case E::Failed:
      if (pair.kind != K::Starting && pair.kind != K::Showing) return false;
      pair = {.kind = K::Failed, .error = event.error};
      return true;
    case E::Finished:
    case E::Closed:
      if (pair.kind != K::Showing) return false;
      pair = {};
      return true;
    case E::Cancel:
      if (pair.kind == K::Idle) return false;
      pair = {};
      return true;
    }
    return false;
  }

  std::int32_t androidKeyCode(std::uint32_t keysym) {
    // Android KeyEvent codes.
    if (keysym >= XKB_KEY_a && keysym <= XKB_KEY_z) return static_cast<std::int32_t>(keysym - XKB_KEY_a) + 29;
    if (keysym >= XKB_KEY_A && keysym <= XKB_KEY_Z) return static_cast<std::int32_t>(keysym - XKB_KEY_A) + 29;
    if (keysym >= XKB_KEY_0 && keysym <= XKB_KEY_9) return static_cast<std::int32_t>(keysym - XKB_KEY_0) + 7;
    if (keysym >= XKB_KEY_F1 && keysym <= XKB_KEY_F12) return static_cast<std::int32_t>(keysym - XKB_KEY_F1) + 131;
    switch (keysym) {
    case XKB_KEY_Escape: return 4; // Back: Android apps leave a screen on Back, not Escape
    case XKB_KEY_Up: return 19;
    case XKB_KEY_Down: return 20;
    case XKB_KEY_Left: return 21;
    case XKB_KEY_Right: return 22;
    case XKB_KEY_comma: return 55;
    case XKB_KEY_period: return 56;
    case XKB_KEY_Tab:
    case XKB_KEY_ISO_Left_Tab: return 61;
    case XKB_KEY_space: return 62;
    case XKB_KEY_Return:
    case XKB_KEY_KP_Enter: return 66;
    case XKB_KEY_BackSpace: return 67;
    case XKB_KEY_grave: return 68;
    case XKB_KEY_minus: return 69;
    case XKB_KEY_equal: return 70;
    case XKB_KEY_bracketleft: return 71;
    case XKB_KEY_bracketright: return 72;
    case XKB_KEY_backslash: return 73;
    case XKB_KEY_semicolon: return 74;
    case XKB_KEY_apostrophe: return 75;
    case XKB_KEY_slash: return 76;
    case XKB_KEY_Menu: return 82;
    case XKB_KEY_Prior: return 92;
    case XKB_KEY_Next: return 93;
    case XKB_KEY_Delete: return 112;
    case XKB_KEY_Home: return 122;
    case XKB_KEY_End: return 123;
    case XKB_KEY_Insert: return 124;
    default: return 0;
    }
  }

  MirrorKey mirrorKeyFor(std::uint32_t keysym, std::uint32_t utf32, KeyMods mods) {
    const bool chord = mods.ctrl || mods.alt || mods.super;
    if (!chord && utf32 >= 0x20 && utf32 != 0x7F && utf32 <= 0x10FFFF && (utf32 < 0xD800 || utf32 > 0xDFFF)) {
      std::string text;
      if (utf32 < 0x80) {
        text += static_cast<char>(utf32);
      } else if (utf32 < 0x800) {
        text += static_cast<char>(0xC0 | (utf32 >> 6));
        text += static_cast<char>(0x80 | (utf32 & 0x3F));
      } else if (utf32 < 0x10000) {
        text += static_cast<char>(0xE0 | (utf32 >> 12));
        text += static_cast<char>(0x80 | ((utf32 >> 6) & 0x3F));
        text += static_cast<char>(0x80 | (utf32 & 0x3F));
      } else {
        text += static_cast<char>(0xF0 | (utf32 >> 18));
        text += static_cast<char>(0x80 | ((utf32 >> 12) & 0x3F));
        text += static_cast<char>(0x80 | ((utf32 >> 6) & 0x3F));
        text += static_cast<char>(0x80 | (utf32 & 0x3F));
      }
      return {.kind = MirrorKey::Kind::Text, .text = std::move(text)};
    }
    const std::int32_t code = androidKeyCode(keysym);
    if (code == 0) return {};
    // Android meta: SHIFT_ON|SHIFT_LEFT_ON, ALT_ON|ALT_LEFT_ON, CTRL_ON|CTRL_LEFT_ON, META_ON|META_LEFT_ON.
    const std::int32_t meta = (mods.shift ? 0x41 : 0) | (mods.alt ? 0x12 : 0) | (mods.ctrl ? 0x3000 : 0) |
                              (mods.super ? 0x30000 : 0);
    return {.kind = MirrorKey::Kind::Code, .code = code, .meta = meta};
  }

  bool transferMayMove(Transfer from, Transfer to) {
    switch (from) {
    case Transfer::Offered: return to == Transfer::Running || to == Transfer::Done || to == Transfer::Failed;
    case Transfer::Running: return to == Transfer::Running || to == Transfer::Done || to == Transfer::Failed;
    case Transfer::Failed: return to == Transfer::Running;
    case Transfer::Done: return false;
    }
    return false;
  }

  std::optional<std::size_t> Feed::find(const std::string& key) const {
    const auto it = std::ranges::find(m_items, key, &FeedItem::key);
    if (it == m_items.end()) return std::nullopt;
    return static_cast<std::size_t>(it - m_items.begin());
  }

  void Feed::upsert(FeedItem item) {
    if (const auto at = find(item.key)) m_items.erase(m_items.begin() + static_cast<std::ptrdiff_t>(*at));
    const auto pos = std::ranges::find_if(m_items, [&](const FeedItem& it) { return it.atMs <= item.atMs; });
    m_items.insert(pos, std::move(item));
    if (m_items.size() > kMax) m_items.resize(kMax);
  }

  bool Feed::remove(const std::string& key) {
    const auto at = find(key);
    if (!at) return false;
    m_items.erase(m_items.begin() + static_cast<std::ptrdiff_t>(*at));
    return true;
  }

  bool Feed::moveTransfer(const std::string& key, Transfer to, std::uint64_t bytes, std::uint64_t total) {
    const auto at = find(key);
    if (!at) return false;
    FeedItem& item = m_items[*at];
    if (item.kind != FeedItem::Kind::Offer || !transferMayMove(item.transfer, to)) return false;
    item.transfer = to;
    if (total > 0) {
      item.bytes = bytes;
      item.total = total;
    }
    return true;
  }

  std::size_t Feed::earlierStart(std::int64_t nowMs) const {
    const auto it = std::ranges::find_if(m_items, [&](const FeedItem& item) { return nowMs - item.atMs >= kNowMs; });
    return static_cast<std::size_t>(it - m_items.begin());
  }

  std::string lastSeenText(std::int64_t lastSeenS, std::int64_t nowS) {
    if (lastSeenS <= 0) return i18n::tr("link.last-seen.never");
    const std::int64_t ago = std::max<std::int64_t>(0, nowS - lastSeenS);
    if (ago < 60) return i18n::tr("link.last-seen.just-now");
    if (ago < 3600) return i18n::tr("link.last-seen.minutes", "count", ago / 60);
    const long days = daysAgo(lastSeenS * 1000, nowS * 1000);
    if (days == 0) return i18n::tr("link.last-seen.hours", "count", ago / 3600);
    if (days == 1) return i18n::tr("link.last-seen.yesterday");
    return i18n::tr("link.last-seen.day", "day", dayLabel(lastSeenS * 1000, nowS * 1000));
  }

  namespace {
    std::tm localTm(std::int64_t ms) {
      const auto t = static_cast<std::time_t>(ms / 1000);
      std::tm tm{};
      localtime_r(&t, &tm);
      return tm;
    }
  } // namespace

  long daysAgo(std::int64_t atMs, std::int64_t nowMs) {
    // Noon of each local date; mktime normalizes the midnights across DST.
    std::tm a = localTm(atMs);
    std::tm now = localTm(nowMs);
    a.tm_hour = now.tm_hour = 12;
    a.tm_min = now.tm_min = a.tm_sec = now.tm_sec = 0;
    a.tm_isdst = now.tm_isdst = -1;
    return static_cast<long>(std::lround(std::difftime(std::mktime(&now), std::mktime(&a)) / 86400.0));
  }

  std::string dayLabel(std::int64_t atMs, std::int64_t nowMs, bool shortWeekday) {
    const std::tm at = localTm(atMs);
    const std::tm now = localTm(nowMs);
    const long days = daysAgo(atMs, nowMs);
    static constexpr std::array kDays{"link.day.sunday", "link.day.monday", "link.day.tuesday", "link.day.wednesday",
                                      "link.day.thursday", "link.day.friday", "link.day.saturday"};
    static constexpr std::array kShortDays{"link.day.short-sunday", "link.day.short-monday", "link.day.short-tuesday",
                                           "link.day.short-wednesday", "link.day.short-thursday", "link.day.short-friday",
                                           "link.day.short-saturday"};
    static constexpr std::array kMonths{"link.month.jan", "link.month.feb", "link.month.mar", "link.month.apr",
                                        "link.month.may", "link.month.jun", "link.month.jul", "link.month.aug",
                                        "link.month.sep", "link.month.oct", "link.month.nov", "link.month.dec"};
    if (days == 0) return i18n::tr("link.day.today");
    if (days == 1) return i18n::tr("link.day.yesterday");
    if (days > 1 && days < 7) return i18n::tr((shortWeekday ? kShortDays : kDays)[static_cast<std::size_t>(at.tm_wday)]);
    const std::string month = i18n::tr(kMonths[static_cast<std::size_t>(at.tm_mon)]);
    if (at.tm_year == now.tm_year) return std::format("{} {}", at.tm_mday, month);
    return std::format("{} {} {}", at.tm_mday, month, at.tm_year + 1900);
  }

  void Activity::offered(const std::string& id, const std::string& deviceId,
                         std::vector<std::pair<std::string, std::uint64_t>> files, std::int64_t nowMs) {
    if (std::ranges::contains(m_items, id, &TransferEntry::id)) return;
    TransferEntry entry{.id = id, .deviceId = deviceId, .files = std::move(files), .atMs = nowMs};
    for (const auto& file : entry.files) entry.total += file.second;
    m_items.insert(m_items.begin(), std::move(entry));
  }

  bool Activity::progress(const std::string& id, std::uint64_t bytes, std::uint64_t total, std::int64_t nowMs) {
    auto it = std::ranges::find(m_items, id, &TransferEntry::id);
    if (it == m_items.end()) {
      m_items.insert(m_items.begin(), {.id = id, .incoming = false, .state = Transfer::Running, .atMs = nowMs});
      it = m_items.begin();
    } else if (!transferMayMove(it->state, Transfer::Running)) {
      return false;
    }
    if (it->state != Transfer::Running || it->rateFromMs == 0) {
      it->rateFromMs = nowMs;
      it->rateFromBytes = bytes;
    }
    it->state = Transfer::Running;
    it->bytes = bytes;
    it->total = total;
    return true;
  }

  bool Activity::finished(const std::string& id, const std::string& status, std::vector<std::string> paths, std::int64_t nowMs) {
    auto it = std::ranges::find(m_items, id, &TransferEntry::id);
    if (status == "declined") {
      if (it == m_items.end()) return false;
      m_items.erase(it);
      return true;
    }
    const Transfer to = status == "done" ? Transfer::Done : Transfer::Failed;
    if (it == m_items.end()) {
      m_items.insert(m_items.begin(), {.id = id, .incoming = false, .state = Transfer::Running, .atMs = nowMs});
      it = m_items.begin();
    }
    if (!transferMayMove(it->state, to)) return false;
    it->state = to;
    it->status = status;
    it->paths = std::move(paths);
    it->endMs = nowMs;
    if (to == Transfer::Done) it->bytes = it->total;
    return true;
  }

  void Activity::sent(const std::string& id, const std::string& deviceId,
                      std::vector<std::pair<std::string, std::uint64_t>> files, std::vector<std::string> sources,
                      std::int64_t nowMs) {
    auto it = std::ranges::find(m_items, id, &TransferEntry::id);
    if (it == m_items.end()) {
      m_items.insert(m_items.begin(), {.id = id, .incoming = false, .state = Transfer::Running, .atMs = nowMs});
      it = m_items.begin();
    } else if (it->incoming) {
      return;
    }
    it->deviceId = deviceId;
    it->files = std::move(files);
    it->sources = std::move(sources);
    if (it->total == 0) {
      for (const auto& file : it->files) it->total += file.second;
      if (it->state == Transfer::Done) it->bytes = it->total;
    }
  }

  bool Activity::forget(const std::string& id) {
    const auto it = std::ranges::find(m_items, id, &TransferEntry::id);
    if (it == m_items.end() || it->state != Transfer::Failed) return false;
    m_items.erase(it);
    return true;
  }

  void Activity::clipped(ClipEntry clip) {
    m_clips.insert(m_clips.begin(), std::move(clip));
    if (m_clips.size() > kClips) m_clips.pop_back();
  }

  Activity::Row Activity::row(std::size_t index) const {
    std::size_t t = 0;
    std::size_t c = 0;
    for (;;) {
      const bool clip = t == m_items.size() || (c < m_clips.size() && m_clips[c].atMs > m_items[t].atMs);
      if (index-- == 0) return clip ? Row(&m_clips[c]) : Row(&m_items[t]);
      ++(clip ? c : t);
    }
  }

  void Activity::clearFinished() {
    std::erase_if(m_items, [](const TransferEntry& e) { return e.state == Transfer::Done || e.state == Transfer::Failed; });
    m_clips.clear();
  }

  std::size_t Activity::finishedCount() const {
    return m_clips.size() + static_cast<std::size_t>(std::ranges::count_if(
        m_items, [](const TransferEntry& e) { return e.state == Transfer::Done || e.state == Transfer::Failed; }));
  }

  std::size_t Activity::offeredCount() const {
    return static_cast<std::size_t>(std::ranges::count(m_items, Transfer::Offered, &TransferEntry::state));
  }

  double transferRate(const TransferEntry& entry, std::int64_t nowMs) {
    const std::int64_t ms = nowMs - entry.rateFromMs;
    if (entry.rateFromMs == 0 || ms <= 0 || entry.bytes <= entry.rateFromBytes) return 0.0;
    return static_cast<double>(entry.bytes - entry.rateFromBytes) * 1000.0 / static_cast<double>(ms);
  }

  std::string byteSize(std::uint64_t bytes) {
    if (bytes >= 1'000'000'000) return std::format("{:.1f} GB", static_cast<double>(bytes) / 1e9);
    if (bytes >= 1'000'000) return std::format("{:.1f} MB", static_cast<double>(bytes) / 1e6);
    return std::format("{} KB", (bytes + 999) / 1000);
  }

  CallLog callLogFrom(const std::string& refused) {
    if (refused.empty()) return CallLog::Ready;
    if (refused == "not-allowed") return CallLog::Off;
    if (refused == "denied") return CallLog::Denied;
    return CallLog::Failed;
  }

  std::string duration(std::uint32_t seconds) {
    if (seconds < 60) return std::format("{} s", seconds);
    const std::uint32_t minutes = (seconds + 30) / 60;
    if (minutes < 60) return std::format("{} min", minutes);
    return minutes % 60 == 0 ? std::format("{} h", minutes / 60) : std::format("{} h {} min", minutes / 60, minutes % 60);
  }

  std::string numberKey(const std::string& address) {
    std::string digits;
    for (const char c : address) {
      if (c >= '0' && c <= '9') digits += c;
    }
    if (digits.empty()) return address;
    return digits.size() > 9 ? digits.substr(digits.size() - 9) : digits;
  }

  namespace {
    // The phone stamps a sent text with its own clock; an older text with the same body is not this one.
    constexpr std::int64_t kClockSkewMs = 5LL * 60 * 1000;
  } // namespace

  void Messages::setTexts(const std::string& phone, std::vector<Text> texts) {
    // TextReceived before the first Texts is this phone's: only a switch to another phone drops the unread ids.
    if (!m_phone.empty() && phone != m_phone) m_unread.clear();
    m_phone = phone;
    m_texts = std::move(texts);
    std::ranges::sort(m_texts, [](const Text& a, const Text& b) { return std::tie(a.atMs, a.id) < std::tie(b.atMs, b.id); });
    m_load = Load::Ready;
    std::erase_if(m_outgoing, [&](const Outgoing& out) {
      return std::ranges::any_of(m_texts, [&](const Text& t) {
        return out.phone == m_phone && t.mine() && t.thread == out.thread && t.body == out.body &&
               t.atMs >= out.atMs - kClockSkewMs;
      });
    });
    readViewed();
    rebuild();
  }

  void Messages::setContacts(const std::vector<Contact>& contacts) {
    m_contacts = contacts;
    m_names.clear();
    for (const Contact& contact : contacts) {
      for (const std::string& number : contact.numbers) m_names.try_emplace(numberKey(number), contact.name);
    }
    rebuild();
  }

  void Messages::received(std::uint64_t textId) {
    m_unread.insert(textId);
    readViewed();
    rebuild();
  }

  void Messages::view(std::optional<std::uint64_t> thread) {
    m_viewing = thread;
    readViewed();
    rebuild();
  }

  std::uint64_t Messages::send(std::uint64_t thread, std::string body, std::int64_t atMs) {
    if (body.empty() || this->thread(thread) == nullptr) return 0;
    m_outgoing.push_back({.serial = ++m_serial, .phone = m_phone, .thread = thread, .body = std::move(body), .atMs = atMs});
    return m_serial;
  }

  bool Messages::sendFailed(std::uint64_t serial) {
    const auto it = std::ranges::find(m_outgoing, serial, &Outgoing::serial);
    if (it == m_outgoing.end() || it->state != Outgoing::State::Sending) return false;
    it->state = Outgoing::State::Failed;
    return true;
  }

  bool Messages::retry(std::uint64_t serial) {
    const auto it = std::ranges::find(m_outgoing, serial, &Outgoing::serial);
    if (it == m_outgoing.end() || it->phone != m_phone || it->state != Outgoing::State::Failed) return false;
    it->state = Outgoing::State::Sending;
    return true;
  }

  const Thread* Messages::thread(std::uint64_t id) const {
    const auto it = std::ranges::find(m_threads, id, &Thread::id);
    return it == m_threads.end() ? nullptr : &*it;
  }

  const Outgoing* Messages::outgoing(std::uint64_t serial) const {
    const auto it = std::ranges::find(m_outgoing, serial, &Outgoing::serial);
    return it == m_outgoing.end() || it->phone != m_phone ? nullptr : &*it;
  }

  std::string Messages::nameFor(const std::string& address) const {
    const auto it = m_names.find(numberKey(address));
    return it == m_names.end() ? address : it->second;
  }

  std::string Messages::numberFor(const std::string& query) const {
    std::string dial;
    bool number = true;
    for (const char c : query) {
      if ((c >= '0' && c <= '9') || c == '+' || c == '*' || c == '#') {
        dial += c;
      } else if (c != ' ' && c != '-' && c != '(' && c != ')' && c != '/' && c != '.') {
        number = false;
      }
    }
    if (number) return dial;
    const auto lower = [](std::string text) {
      for (char& c : text) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
      return text;
    };
    std::string want = lower(query);
    while (!want.empty() && want.back() == ' ') want.pop_back();
    for (const Contact& contact : m_contacts) {
      if (!contact.numbers.empty() && lower(contact.name).starts_with(want)) return numberFor(contact.numbers.front());
    }
    return {};
  }

  std::vector<Contact> Messages::matches(const std::string& query, std::size_t limit) const {
    const auto lower = [](std::string text) {
      for (char& c : text) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
      return text;
    };
    const std::string want = lower(query);
    std::vector<Contact> found;
    if (want.empty()) return found;
    for (const Contact& contact : m_contacts) {
      if (found.size() == limit) break;
      if (!contact.numbers.empty() && lower(contact.name).starts_with(want)) found.push_back(contact);
    }
    return found;
  }

  std::optional<std::uint64_t> Messages::threadFor(const std::string& address) const {
    const std::string key = numberKey(address);
    const auto it = std::ranges::find_if(m_threads, [&](const Thread& t) { return numberKey(t.address) == key; });
    return it == m_threads.end() ? std::nullopt : std::optional(it->id);
  }

  int Messages::unread() const {
    int n = 0;
    for (const Thread& t : m_threads) n += t.unread;
    return n;
  }

  // An unread id stays until its text shows up in a viewed thread: TextReceived can come before Texts holds it.
  void Messages::readViewed() {
    if (!m_viewing) return;
    for (const Text& t : m_texts) {
      if (t.thread == *m_viewing) m_unread.erase(t.id);
    }
  }

  void Messages::rebuild() {
    std::map<std::uint64_t, Thread> byId;
    for (const Text& t : m_texts) { // oldest first: the last write per thread is its newest text
      Thread& thread = byId[t.thread];
      thread.id = t.thread;
      thread.address = t.address;
      thread.last = t.body.empty() && !t.images.empty() ? "Photo" : t.body;
      thread.atMs = t.atMs;
      if (m_unread.contains(t.id) && !t.mine()) ++thread.unread;
    }
    m_threads.clear();
    m_threads.reserve(byId.size());
    for (auto& [id, thread] : byId) {
      thread.name = nameFor(thread.address);
      m_threads.push_back(std::move(thread));
    }
    std::ranges::sort(m_threads, [](const Thread& a, const Thread& b) { return std::tie(b.atMs, b.id) < std::tie(a.atMs, a.id); });
  }

  std::vector<ConversationRow> Messages::conversation(std::uint64_t thread, std::int64_t nowMs) const {
    using K = ConversationRow::Kind;
    std::vector<ConversationRow> rows;
    std::string day;
    const auto add = [&](ConversationRow row) {
      const std::string label = dayLabel(row.atMs, nowMs);
      if (label != day) {
        if (!rows.empty()) rows.back().runEnd = rows.back().kind != K::Day;
        day = label;
        rows.push_back({.kind = K::Day, .text = label, .atMs = row.atMs, .key = std::hash<std::string>{}(label)});
      } else if (!rows.empty() && (rows.back().kind == K::In) != (row.kind == K::In)) {
        rows.back().runEnd = true;
      }
      rows.push_back(std::move(row));
    };
    using S = Text::Status;
    using N = ConversationRow::Note;
    for (const Text& t : m_texts) {
      if (t.thread != thread) continue;
      const K kind = t.mine() ? K::Out : K::In;
      const N note = t.status == S::Sending ? N::Sending : t.status == S::Failed ? N::NotSent : N::None;
      for (const std::uint64_t part : t.images) {
        add({.kind = kind, .atMs = t.atMs, .key = (1ULL << 63) | part, .image = part, .note = note});
      }
      if (!t.body.empty() || t.images.empty()) {
        add({.kind = kind, .text = t.body.empty() ? "Attachment" : t.body, .atMs = t.atMs, .key = t.id, .note = note});
      }
    }
    for (const Outgoing& out : m_outgoing) {
      if (out.phone != m_phone || out.thread != thread) continue;
      add({.kind = out.state == Outgoing::State::Failed ? K::Failed : K::Sending, .text = out.body, .atMs = out.atMs,
           .key = ~out.serial, .serial = out.serial});
    }
    if (!rows.empty()) rows.back().runEnd = true;
    const auto newestOut = std::ranges::find_if(rows.rbegin(), rows.rend(), [](const ConversationRow& row) {
      return row.kind == K::Out || row.kind == K::Sending || row.kind == K::Failed;
    });
    if (newestOut != rows.rend() && newestOut->kind == K::Out) {
      const auto text = std::ranges::find(m_texts, newestOut->key, &Text::id);
      if (text != m_texts.end() && text->status == S::Delivered) newestOut->note = N::Delivered;
    }
    return rows;
  }

  std::optional<Photos::Ask> Photos::startLoad(const std::string& device) {
    if (device != m_device) {
      m_device = device;
      forget();
    } else if (m_load == Load::Loading) {
      return std::nullopt;
    }
    m_load = Load::Loading;
    return Ask{.serial = ++m_serial, .offset = 0, .media = m_media};
  }

  std::optional<Photos::Ask> Photos::startMore() {
    if (m_load != Load::Ready || !hasMore()) return std::nullopt;
    m_load = Load::More;
    return Ask{.serial = ++m_serial, .offset = m_next, .media = m_media};
  }

  bool Photos::setMedia(Media media) {
    if (media == m_media) return false;
    m_media = media;
    forget();
    return true;
  }

  void Photos::forget() {
    ++m_serial;
    m_load = Load::Idle;
    m_items.clear();
    m_listed.clear();
    m_selected.clear();
    m_total = m_next = 0;
  }

  bool Photos::loaded(std::uint64_t serial, std::uint32_t total, std::vector<Photo> page) {
    if (serial != m_serial || (m_load != Load::Loading && m_load != Load::More)) return false;
    if (m_load == Load::Loading) {
      m_items.clear();
      m_listed.clear();
      m_next = 0;
    }
    std::ranges::stable_sort(page, std::greater{}, &Photo::atS);
    for (Photo& photo : page) {
      if (m_listed.insert(photo.path).second) m_items.push_back(std::move(photo));
    }
    std::erase_if(m_selected, [&](const std::string& path) { return !m_listed.contains(path); });
    m_next += kCount;
    m_total = total;
    m_load = Load::Ready;
    return true;
  }

  bool Photos::failed(std::uint64_t serial, Load why) {
    if (serial != m_serial || why == Load::Idle || why == Load::Loading || why == Load::More || why == Load::Ready) {
      return false;
    }
    if (m_load == Load::More) {
      m_load = Load::Ready;
      return true;
    }
    if (m_load != Load::Loading) return false;
    m_load = why;
    m_items.clear();
    m_listed.clear();
    m_selected.clear();
    m_total = m_next = 0;
    return true;
  }

  void Photos::toggle(std::size_t index) {
    if (index >= m_items.size()) return;
    const std::string& path = m_items[index].path;
    if (!m_selected.erase(path)) m_selected.insert(path);
  }

  bool Photos::selected(std::size_t index) const {
    return index < m_items.size() && m_selected.contains(m_items[index].path);
  }

  std::vector<std::string> Photos::startSave() {
    if (m_save == Save::Saving || m_selected.empty()) return {};
    std::vector<std::string> paths;
    for (const Photo& photo : m_items) {
      if (m_selected.contains(photo.path)) paths.push_back(photo.path);
    }
    m_selected.clear();
    m_save = Save::Saving;
    m_pending = paths.size();
    m_saved = m_saveFailed = 0;
    return paths;
  }

  void Photos::saveAnswered(bool ok) {
    if (m_save != Save::Saving) return;
    ++(ok ? m_saved : m_saveFailed);
    if (--m_pending == 0) m_save = Save::Done;
  }

  std::vector<std::pair<std::size_t, std::string>> Photos::days(std::int64_t nowMs) const {
    std::vector<std::pair<std::size_t, std::string>> out;
    // Items come newest first: one label per local day, so the cost grows with days, not items.
    std::int64_t dayStartS = 0;
    for (std::size_t i = 0; i < m_items.size(); ++i) {
      const std::int64_t at = m_items[i].atS;
      if (!out.empty() && at >= dayStartS) continue;
      const auto t = static_cast<std::time_t>(at);
      std::tm tm{};
      localtime_r(&t, &tm);
      tm.tm_hour = tm.tm_min = tm.tm_sec = 0;
      tm.tm_isdst = -1;
      dayStartS = std::mktime(&tm);
      std::string label = dayLabel(at * 1000, nowMs);
      if (out.empty() || out.back().second != label) out.emplace_back(i, std::move(label));
    }
    return out;
  }

  std::string groupedCount(std::uint64_t n) {
    std::string digits = std::to_string(n);
    std::string out;
    for (std::size_t i = 0; i < digits.size(); ++i) {
      if (i > 0 && (digits.size() - i) % 3 == 0) out += "\u202f";
      out += digits[i];
    }
    return out;
  }

  std::optional<FeedItem> newPhotosItem(const std::string& deviceId, const std::vector<Photo>& photos, std::int64_t seenS,
                                        std::int64_t nowMs) {
    FeedItem item{.kind = FeedItem::Kind::Photos, .key = "p:new", .deviceId = deviceId};
    std::size_t n = 0;
    for (const Photo& photo : photos) {
      if (photo.atS <= seenS || daysAgo(photo.atS * 1000, nowMs) != 0) continue;
      if (n++ == 0) item.atMs = photo.atS * 1000;
      if (item.photos.size() < kStripPhotos) item.photos.push_back(photo.path);
    }
    if (n == 0) return std::nullopt;
    item.who = i18n::trp("link.feed.new-photos", static_cast<long>(n));
    return item;
  }

  std::vector<std::string> Files::crumbs() const {
    std::vector<std::string> out{i18n::tr("link.files.phone-root")};
    for (std::size_t at = 1; at < m_path.size();) {
      const std::size_t end = std::min(m_path.find('/', at), m_path.size());
      out.push_back(m_path.substr(at, end - at));
      at = end + 1;
    }
    return out;
  }

  std::string Files::childPath(const std::string& name) const {
    return m_path == "/" ? "/" + name : m_path + "/" + name;
  }

  std::uint64_t Files::go(const std::string& path) {
    if (path != m_path) m_entries.clear();
    m_path = path;
    m_load = Load::Loading;
    m_error.clear();
    return ++m_serial;
  }

  std::string Files::parent() const {
    const std::size_t slash = m_path.rfind('/');
    return slash == 0 || slash == std::string::npos ? std::string("/") : m_path.substr(0, slash);
  }

  std::string Files::crumbPath(std::size_t index) const {
    std::size_t at = 0;
    for (std::size_t i = 0; i < index; ++i) {
      at = m_path.find('/', at + 1);
      if (at == std::string::npos) return m_path;
    }
    return index == 0 ? std::string("/") : m_path.substr(0, at);
  }

  bool Files::listed(std::uint64_t serial, std::vector<FileEntry> entries) {
    if (serial != m_serial || m_load != Load::Loading) return false;
    const auto lower = [](const std::string& text) {
      std::string out = text;
      std::ranges::transform(out, out.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
      return out;
    };
    std::ranges::stable_sort(entries, [&](const FileEntry& a, const FileEntry& b) {
      return std::tuple(!a.folder, lower(a.name)) < std::tuple(!b.folder, lower(b.name));
    });
    m_entries = std::move(entries);
    m_load = Load::Ready;
    return true;
  }

  bool Files::failed(std::uint64_t serial, std::string error) {
    if (serial != m_serial || m_load != Load::Loading) return false;
    m_entries.clear();
    m_error = std::move(error);
    m_load = Load::Failed;
    return true;
  }

  const FileSave* Files::save(const std::string& path) const {
    const auto it = m_saves.find(path);
    return it == m_saves.end() ? nullptr : &it->second;
  }

  bool Files::saveStarted(const std::string& path, std::uint64_t total) {
    const auto it = m_saves.find(path);
    if (it != m_saves.end() && it->second.state == FileSave::State::Running) return false;
    m_saves[path] = {.state = FileSave::State::Running, .total = total};
    return true;
  }

  bool Files::saveProgress(const std::string& path, std::uint64_t got) {
    const auto it = m_saves.find(path);
    if (it == m_saves.end() || it->second.state != FileSave::State::Running || got < it->second.got) return false;
    it->second.got = got;
    return true;
  }

  bool Files::saveDone(const std::string& path, std::string saved) {
    const auto it = m_saves.find(path);
    if (it == m_saves.end() || it->second.state != FileSave::State::Running) return false;
    it->second = {.state = FileSave::State::Done, .got = it->second.got, .total = it->second.total, .saved = std::move(saved)};
    return true;
  }

  bool Files::saveFailed(const std::string& path) {
    const auto it = m_saves.find(path);
    if (it == m_saves.end() || it->second.state != FileSave::State::Running) return false;
    it->second.state = FileSave::State::Failed;
    return true;
  }

  bool mirrorStep(Mirror& mirror, const MirrorEvent& event) {
    using K = Mirror::Kind;
    using E = MirrorEvent::Kind;
    const bool current = event.session == mirror.session;
    switch (event.kind) {
    case E::Start:
      if (mirror.kind == K::Asking || mirror.kind == K::Showing) return false;
      mirror = {.kind = K::Asking, .session = mirror.session + 1};
      return true;
    case E::Opened:
      if (!current || mirror.kind != K::Asking) return false;
      if (event.width == 0 || event.height == 0) {
        mirror = {.kind = K::Failed, .session = mirror.session, .error = i18n::tr("link.client.no-screen-size")};
        return true;
      }
      mirror.kind = K::Showing;
      mirror.width = event.width;
      mirror.height = event.height;
      return true;
    case E::Ended:
      if (!current || mirror.kind != K::Showing) return false;
      mirror = {.kind = K::Ended, .session = mirror.session};
      return true;
    case E::Failed:
      if (!current || (mirror.kind != K::Asking && mirror.kind != K::Showing)) return false;
      mirror = {.kind = K::Failed, .session = mirror.session, .error = event.error};
      return true;
    case E::Stop:
      if (mirror.kind != K::Asking && mirror.kind != K::Showing) return false;
      mirror = {.kind = K::Idle, .session = mirror.session};
      return true;
    }
    return false;
  }

  namespace {
    bool blank(const std::string& text) {
      return text.find_first_not_of(" \t\r\n") == std::string::npos;
    }
  } // namespace

  void CommandsEditor::queue() {
    ++m_serial;
    m_queued = true;
    m_sync = Sync::Sending;
    m_error.clear();
  }

  // Ends Editing. False when the draft is invalid: the row stays open and invalid() says which field.
  bool CommandsEditor::commit() {
    if (m_mode != Mode::Editing) return true;
    const bool fresh = m_index == m_list.size();
    if (fresh && blank(m_draft.name) && blank(m_draft.line)) {
      m_mode = Mode::Idle;
      m_invalid = Invalid::None;
      return true;
    }
    if (blank(m_draft.name) || m_draft.name.size() > kMaxName || blank(m_draft.line)) {
      m_invalid = blank(m_draft.line) && !blank(m_draft.name) && m_draft.name.size() <= kMaxName ? Invalid::Line : Invalid::Name;
      return false;
    }
    if (fresh && !canAdd()) {
      m_invalid = Invalid::Full;
      return false;
    }
    if (fresh) {
      m_list.push_back(m_draft);
      queue();
    } else if (m_list[m_index] != m_draft) {
      m_list[m_index] = m_draft;
      queue();
    }
    m_mode = Mode::Idle;
    m_invalid = Invalid::None;
    return true;
  }

  bool CommandsEditor::listed(std::vector<Command> commands) {
    if (m_sync != Sync::Synced || commands == m_list) return false;
    const bool fresh = m_mode == Mode::Editing && m_index == m_list.size();
    m_list = std::move(commands);
    if (m_mode == Mode::Editing && !fresh) {
      const auto it = std::ranges::find(m_list, m_opened);
      m_index = it == m_list.end() ? m_list.size() : static_cast<std::size_t>(it - m_list.begin());
    } else if (fresh || m_index > m_list.size()) {
      m_index = m_list.size();
    }
    return true;
  }

  bool CommandsEditor::open(std::size_t index) {
    if (index >= m_list.size()) return false;
    if (m_mode == Mode::Editing) {
      if (m_index == index) return false;
      if (!commit()) return true;
    }
    m_mode = Mode::Editing;
    m_index = index;
    m_draft = m_opened = m_list[index];
    m_invalid = Invalid::None;
    return true;
  }

  bool CommandsEditor::add() {
    bool changed = false;
    if (m_mode == Mode::Editing) {
      if (m_index == m_list.size()) return false;
      if (!commit()) return true;
      changed = true;
    }
    if (!canAdd()) return changed;
    m_mode = Mode::Editing;
    m_index = m_list.size();
    m_draft = {};
    m_invalid = Invalid::None;
    return true;
  }

  bool CommandsEditor::editName(std::string name) {
    if (m_mode != Mode::Editing || m_draft.name == name) return false;
    m_draft.name = std::move(name);
    if (m_invalid == Invalid::Name && !blank(m_draft.name)) m_invalid = Invalid::None;
    return true;
  }

  bool CommandsEditor::editLine(std::string line) {
    if (m_mode != Mode::Editing || m_draft.line == line) return false;
    m_draft.line = std::move(line);
    if (m_invalid == Invalid::Line && !blank(m_draft.line)) m_invalid = Invalid::None;
    return true;
  }

  bool CommandsEditor::close() {
    if (m_mode != Mode::Editing) return false;
    commit();
    return true;
  }

  bool CommandsEditor::remove() {
    if (m_mode != Mode::Editing) return false;
    m_invalid = Invalid::None;
    if (m_index == m_list.size()) {
      m_mode = Mode::Idle;
      return true;
    }
    m_draft = m_list[m_index];
    m_list.erase(m_list.begin() + static_cast<std::ptrdiff_t>(m_index));
    m_mode = Mode::Removed;
    queue();
    return true;
  }

  bool CommandsEditor::undo() {
    if (m_mode != Mode::Removed || !canAdd()) return false;
    m_list.insert(m_list.begin() + static_cast<std::ptrdiff_t>(std::min(m_index, m_list.size())), m_draft);
    m_mode = Mode::Idle;
    queue();
    return true;
  }

  bool CommandsEditor::moveIf(std::size_t from, std::size_t to, const Command& expected) {
    return from < m_list.size() && m_list[from] == expected && move(from, to);
  }

  bool CommandsEditor::move(std::size_t from, std::size_t to) {
    if (from >= m_list.size() || to >= m_list.size() || from == to) return false;
    if (m_mode == Mode::Editing && !commit()) return true;
    if (from >= m_list.size() || to >= m_list.size()) return true;
    m_mode = Mode::Idle;
    const auto first = m_list.begin();
    if (from < to) std::rotate(first + static_cast<std::ptrdiff_t>(from), first + static_cast<std::ptrdiff_t>(from) + 1,
                               first + static_cast<std::ptrdiff_t>(to) + 1);
    else std::rotate(first + static_cast<std::ptrdiff_t>(to), first + static_cast<std::ptrdiff_t>(from),
                     first + static_cast<std::ptrdiff_t>(from) + 1);
    queue();
    return true;
  }

  std::optional<std::pair<std::uint64_t, std::vector<Command>>> CommandsEditor::takeSend() {
    if (!m_queued) return std::nullopt;
    m_queued = false;
    return std::pair{m_serial, m_list};
  }

  bool CommandsEditor::sent(std::uint64_t serial, std::optional<std::string> error) {
    if (serial != m_serial || m_queued || m_sync != Sync::Sending) return false;
    m_sync = error ? Sync::Failed : Sync::Synced;
    m_error = error ? std::move(*error) : std::string();
    return true;
  }

  bool CommandsEditor::retry() {
    if (m_sync != Sync::Failed) return false;
    queue();
    return true;
  }

  SearchHits searchHits(std::string_view query, const std::vector<Thread>& threads, const std::vector<CallEntry>& calls,
                        const std::vector<FileEntry>& files, const std::vector<Photo>& photos) {
    const auto fold = [](unsigned char c) { return static_cast<char>(c < 0x80 ? std::tolower(c) : c); };
    std::string q;
    for (const char c : query) q += fold(static_cast<unsigned char>(c));
    const auto has = [&](std::string_view text) {
      return !std::ranges::search(text, q, {}, [&](char c) { return fold(static_cast<unsigned char>(c)); }).empty();
    };
    SearchHits hits;
    for (std::size_t i = 0; i < threads.size(); ++i) {
      if (has(threads[i].name) || has(threads[i].last)) hits.threads.push_back(i);
    }
    for (std::size_t i = 0; i < calls.size(); ++i) {
      if (has(calls[i].name) || has(calls[i].number)) hits.calls.push_back(i);
    }
    for (std::size_t i = 0; i < files.size(); ++i) {
      if (has(files[i].name)) hits.files.push_back(i);
    }
    for (std::size_t i = 0; i < photos.size(); ++i) {
      if (has(photos[i].name)) hits.photos.push_back(i);
    }
    return hits;
  }

} // namespace linkapp
