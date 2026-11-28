#include "link_client.h"

#include "core/log.h"
#include "i18n/i18n.h"
#include "link_bus.h"
#include "protocols/async_file_writer.h"

#include <cctype>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <algorithm>
#include <array>
#include <fcntl.h>
#include <sys/stat.h>
#include <filesystem>
#include <glib.h>
#include <chrono>
#include <format>
#include <fstream>
#include <map>
#include <optional>
#include <unistd.h>

namespace linkapp {

  namespace {

    constexpr Logger kLog("link");
    constexpr auto kInterface = "org.andromeda.Link1";
    constexpr auto kQuickShare = "org.andromeda.Link1.QuickShare";

    std::int64_t nowMs() {
      return std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::system_clock::now().time_since_epoch()).count();
    }

    template <typename T>
    T value(const std::map<std::string, bus::Variant>& map, const std::string& key, T fallback) {
      const auto it = map.find(key);
      if (it == map.end() || !it->second.containsValueOfType<T>()) return fallback;
      return it->second.get<T>();
    }

    template <typename T>
    T property(const std::map<std::string, bus::Variant>& props, const std::string& key) {
      const auto it = props.find(key);
      if (it == props.end() || !it->second.containsValueOfType<T>()) return T{};
      return it->second.get<T>();
    }

  } // namespace

  namespace {
    std::filesystem::path selectedFile() {
      return std::filesystem::path(g_get_user_state_dir()) / "andromeda" / "link-app" / "phone";
    }

    std::filesystem::path photosSeenFile(const std::string& deviceId) {
      return selectedFile().parent_path() / ("photos-seen-" + deviceId);
    }

    std::filesystem::path snapshotFile() {
      return std::filesystem::path(g_get_user_cache_dir()) / "andromeda" / "link-app" / "waiting.json";
    }

    // Items a phone may have removed while this app was closed: older ones stay out of a cold start.
    constexpr std::int64_t kSnapshotMaxAgeMs = 24LL * 3600 * 1000;
  } // namespace

  LinkClient::LinkClient(std::function<void(const std::vector<std::string>&)> showInFolder)
      : m_showInFolder(std::move(showInFolder)) {
    std::getline(std::ifstream(selectedFile()), m_selected);
    std::ifstream cached(snapshotFile(), std::ios::binary);
    if (auto snapshot = decodeSnapshot(std::string(std::istreambuf_iterator<char>(cached), {}))) {
      m_daemon = true;
      m_phones = std::move(snapshot->phones);
      m_dismissed = std::move(snapshot->dismissed);
      const std::int64_t oldest = nowMs() - kSnapshotMaxAgeMs;
      for (FeedItem& item : snapshot->items) {
        if (item.atMs >= oldest) m_feed.upsert(std::move(item));
      }
    }
    // Link1 starting or stopping after this app: ask again, so presence leaves NoDaemon or enters it.
    m_proxy = std::make_unique<bus::Link>([this](bool) {
      refresh();
      refreshNearby();
    });
    m_proxy->uponSignal("PropertiesChanged")
        .onInterface("org.freedesktop.DBus.Properties")
        .call([this](const std::string& interfaceName, const std::map<std::string, bus::Variant>&,
                     const std::vector<std::string>&) {
          if (interfaceName == kInterface) refresh();
          if (interfaceName == kQuickShare) refreshNearby();
        });
    m_proxy->uponSignal("PairingFinished").onInterface(kInterface).call([this](const std::string& id, const std::string&) {
      if (!pairStep(m_pair, {.kind = PairEvent::Kind::Finished})) return;
      if (m_paired) m_paired(id);
      changed();
    });
    m_proxy->uponSignal("PairingFailed").onInterface(kInterface).call([this](const std::string& reason) {
      if (pairStep(m_pair, {.kind = PairEvent::Kind::Failed, .error = reason})) changed();
    });
    m_proxy->uponSignal("NotificationPosted")
        .onInterface(kInterface)
        .call([this](const std::string& device, const std::string& id, const std::string& app, const std::string& title,
                     const std::string& text, const std::vector<std::uint8_t>&,
                     const std::vector<bus::Struct<std::string, std::string, bool>>& actions, bool,
                     const std::vector<std::string>& replies) {
          FeedItem item{.kind = FeedItem::Kind::Notification, .key = "n:" + id, .deviceId = device,
                        .who = app.empty() ? title : app + " · " + title, .body = text, .atMs = nowMs(),
                        .replies = replies};
          for (const auto& a : actions) item.actions.push_back({a.get<0>(), a.get<1>(), a.get<2>()});
          m_feed.upsert(std::move(item));
          changed();
        });
    m_proxy->uponSignal("Call")
        .onInterface(kInterface)
        .call([this](const std::string& device, const std::string& state, const std::string& number, const std::string& name,
                     const std::string& app) {
          if (state == "idle") {
            if (m_phoneCalls.erase(device) > 0) changed();
            return;
          }
          if (state != "ringing" && state != "active") return;
          const auto phone = std::ranges::find(m_phones, device, &Phone::id);
          if (phone == m_phones.end() || !phone->connected) return;
          PhoneCall& call = m_phoneCalls[device];
          call.state = state == "ringing" ? PhoneCall::State::Ringing : PhoneCall::State::Active;
          call.number = number;
          call.name = name;
          call.app = app;
          changed();
        });
    m_proxy->uponSignal("HotspotRequested")
        .onInterface(kInterface)
        .call([this](const std::string& device, const std::string& ssid) {
          if (!ssid.empty()) {
            m_hotspotAsk = HotspotAsk{.deviceId = device, .ssid = ssid};
          } else if (m_hotspotAsk && m_hotspotAsk->deviceId == device) {
            m_hotspotAsk.reset();
          } else {
            return;
          }
          changed();
        });
    m_proxy->uponSignal("ScreenShape")
        .onInterface(kInterface)
        .call([this](const std::string& device, const std::vector<std::uint16_t>& corners,
                     const std::vector<bus::Struct<std::uint16_t, std::uint16_t, std::uint16_t, std::uint16_t>>&) {
          if (device != m_mirrorDevice) return;
          const std::uint16_t radius = corners.empty() ? 0 : std::ranges::max(corners);
          if (radius != std::exchange(m_mirrorCorner, radius)) changed();
        });
    m_proxy->uponSignal("NotificationRemoved")
        .onInterface(kInterface)
        .call([this](const std::string&, const std::string& id) {
          if (m_feed.remove("n:" + id)) changed();
        });
    m_proxy->uponSignal("TextReceived")
        .onInterface(kInterface)
        .call([this](const std::string& device, std::uint64_t id, const std::string& address, const std::string& body) {
          m_feed.upsert({.kind = FeedItem::Kind::Text, .key = "t:" + std::to_string(id), .deviceId = device,
                         .who = address, .body = body, .atMs = nowMs()});
          m_messages.received(id);
          changed();
        });
    m_proxy->uponSignal("TransferOffered")
        .onInterface(kInterface)
        .call([this](const std::string& transfer, const std::string& device,
                     const std::vector<bus::Struct<std::string, std::uint64_t>>& files) {
          FeedItem item{.kind = FeedItem::Kind::Offer, .key = "o:" + transfer, .deviceId = device, .atMs = nowMs()};
          for (const auto& f : files) {
            item.files.emplace_back(f.get<0>(), f.get<1>());
            item.total += f.get<1>();
          }
          m_activity.offered(transfer, device, item.files, item.atMs);
          m_feed.upsert(std::move(item));
          changed();
        });
    m_proxy->uponSignal("TransferProgress")
        .onInterface(kInterface)
        .call([this](const std::string& transfer, std::uint64_t bytes, std::uint64_t total) {
          const bool feed = m_feed.moveTransfer("o:" + transfer, Transfer::Running, bytes, total);
          if (m_activity.progress(transfer, bytes, total, nowMs()) || feed) changed();
        });
    m_proxy->uponSignal("TransferFinished")
        .onInterface(kInterface)
        .call([this](const std::string& transfer, const std::string& status, const std::vector<std::string>& paths) {
          const bool listed = m_activity.finished(transfer, status, paths, nowMs());
          const std::string key = "o:" + transfer;
          const bool moved = status == "done"       ? m_feed.moveTransfer(key, Transfer::Done)
                             : status == "declined" ? m_feed.remove(key)
                                                    : m_feed.moveTransfer(key, Transfer::Failed);
          if (moved || listed) changed();
        });
    // Text clips only: Link1 pulls only the latest offer, so an older image could not be copied again.
    m_proxy->uponSignal("ClipboardOffered")
        .onInterface(kInterface)
        .call([this](const std::string& device, std::uint64_t id, const std::vector<std::string>& mimes, std::uint64_t size) {
          constexpr std::uint64_t kMaxClip = 60 * 1024; // below a pipe's 64 KiB buffer: the daemon's write never blocks
          if (mimes.empty() || size == 0 || size > kMaxClip || std::ranges::contains(mimes, "x-kde-passwordManagerHint") ||
              (mimes.front() != "text/plain;charset=utf-8" && mimes.front() != "text/plain")) {
            return;
          }
          std::array<int, 2> fds{};
          if (::pipe(fds.data()) != 0) return;
          for (const int fd : fds) {
            ::fcntl(fd, F_SETFD, FD_CLOEXEC);
            ::fcntl(fd, F_SETFL, O_NONBLOCK);
          }
          auto source = std::make_shared<bus::UnixFd>(fds[0], bus::adopt_fd);
          m_proxy->callMethodAsync("PullClipboard").onInterface(kInterface)
              .withArguments(device, id, mimes.front(), bus::UnixFd{fds[1], bus::adopt_fd})
              .uponReplyInvoke([this, source, device, id](std::optional<bus::Error> error, std::uint64_t bytes) {
                if (error) {
                  kLog.warn("PullClipboard: {}", error->getMessage());
                  return;
                }
                std::string text(bytes, '\0');
                std::size_t got = 0;
                while (got < text.size()) {
                  const ssize_t n = ::read(source->get(), text.data() + got, text.size() - got);
                  if (n <= 0 && !(n < 0 && errno == EINTR)) break;
                  if (n > 0) got += static_cast<std::size_t>(n);
                }
                if (got != bytes || text.empty()) return;
                m_activity.clipped({.id = id, .deviceId = device, .text = std::move(text), .atMs = nowMs()});
                changed();
              });
        });
    refresh();
    refreshNearby();
  }

  LinkClient::~LinkClient() {
    andromeda::writeFileAsync(snapshotFile(), encodeSnapshot(m_phones, m_feed, m_dismissed));
  }

  void LinkClient::changed() {
    if (m_changed) m_changed();
  }

  void LinkClient::refresh() {
    const std::uint64_t serial = ++m_sent;
    m_proxy->callMethodAsync("GetAll")
        .onInterface("org.freedesktop.DBus.Properties")
        .withArguments(std::string{kInterface})
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](
                             std::optional<bus::Error> error, std::map<std::string, bus::Variant> props) {
          if (alive.expired() || serial <= m_applied) return;
          m_applied = serial;
          m_daemon = !error.has_value();
          m_phones.clear();
          if (error.has_value()) {
            m_phoneCalls.clear();
            kLog.debug("link unavailable: {}", error->getMessage());
            changed();
            return;
          }
          // The daemon closes a window on expiry or CancelPairing by clearing Pairing, with no signal.
          const bool pairing = property<bool>(props, "Pairing");
          if (m_pairing && !pairing) (void)pairStep(m_pair, {.kind = PairEvent::Kind::Closed});
          m_pairing = pairing;
          const auto info = property<std::map<std::string, std::map<std::string, bus::Variant>>>(props, "DeviceInfo");
          const auto status =
              property<std::map<std::string, bus::Struct<std::uint32_t, bool, std::string, std::int32_t>>>(props, "DeviceStatus");
          const auto controls =
              property<std::map<std::string, bus::Struct<std::int32_t, std::string, std::int32_t, std::int32_t>>>(props, "DeviceControls");
          const auto toggles =
              property<std::map<std::string, bus::Struct<std::int32_t, std::int32_t, std::int32_t>>>(props, "DeviceToggles");
          const auto paused = property<std::vector<std::string>>(props, "Paused");
          using Player = bus::Struct<std::string, std::string, std::string, std::string, std::string, bool, std::uint64_t,
                                       std::uint64_t, std::int32_t, std::int64_t, std::vector<std::string>>;
          const auto players = property<std::map<std::string, Player>>(props, "PhonePlayers");
          for (const auto& d : property<std::vector<bus::Struct<std::string, std::string, bool>>>(props, "Devices")) {
            Phone phone{.id = d.get<0>(), .name = d.get<1>(), .connected = d.get<2>()};
            if (const auto it = info.find(phone.id); it != info.end()) {
              phone.transport = value<std::string>(it->second, "transport", "");
              phone.lastSeen = value<std::int64_t>(it->second, "last_seen", 0);
              phone.battery = value<std::int32_t>(it->second, "battery", -1);
              phone.mic = value<std::string>(it->second, "mic", "");
              phone.sound = value<std::string>(it->second, "sound", "");
              phone.camera = value<std::string>(it->second, "camera", "");
              if (it->second.contains("storage_free")) phone.storageFree = value<std::uint64_t>(it->second, "storage_free", 0);
            }
            if (const auto it = status.find(phone.id); it != status.end()) {
              phone.battery = static_cast<int>(it->second.get<0>());
              phone.charging = it->second.get<1>();
              phone.signal = it->second.get<3>();
            }
            if (const auto it = controls.find(phone.id); it != controls.end()) {
              phone.mediaVolume = it->second.get<0>();
              phone.dnd = it->second.get<2>();
              phone.ringer = it->second.get<1>();
              phone.locked = it->second.get<3>();
            }
            if (const auto it = toggles.find(phone.id); it != toggles.end()) {
              phone.wifi = it->second.get<0>();
              phone.bluetooth = it->second.get<1>();
              phone.torch = it->second.get<2>();
            }
            if (const auto it = players.find(phone.id); it != players.end()) {
              const Player& p = it->second;
              const std::string& art = p.get<4>();
              phone.player = PhonePlayer{.app = p.get<1>(), .title = p.get<2>(), .artist = p.get<3>(),
                                         .art = art.starts_with("file://") ? art.substr(7) : std::string(),
                                         .playing = p.get<5>(), .positionMs = static_cast<std::int64_t>(p.get<6>()),
                                         .lengthMs = static_cast<std::int64_t>(p.get<7>()), .atMs = p.get<9>(),
                                         .commands = p.get<10>()};
            }
            phone.paused = std::ranges::find(paused, phone.id) != paused.end();
            m_phones.push_back(std::move(phone));
          }
          // A phone that left takes its call with it: no idle Call comes for it.
          std::erase_if(m_phoneCalls, [this](const auto& entry) {
            const auto phone = std::ranges::find(m_phones, entry.first, &Phone::id);
            return phone == m_phones.end() || !phone->connected;
          });
          // Missed calls of the last day; the Calls page holds the rest.
          const std::int64_t since = nowMs() - 24LL * 3600 * 1000;
          using Call = bus::Struct<std::string, std::string, std::uint64_t, std::uint32_t, std::string>;
          for (const auto& [device, calls] : property<std::map<std::string, std::vector<Call>>>(props, "Calls")) {
            for (const Call& call : calls) {
              const auto at = static_cast<std::int64_t>(call.get<2>());
              if (call.get<4>() != "missed" || at < since) continue;
              const std::string key = "c:" + device + ":" + std::to_string(at);
              if (m_feed.find(key) || m_dismissed.contains(key)) continue;
              m_feed.upsert({.kind = FeedItem::Kind::MissedCall, .key = key, .deviceId = device,
                             .who = call.get<1>().empty() ? call.get<0>() : call.get<1>(), .body = i18n::tr("link.client.missed-call"),
                             .atMs = at, .actions = {{call.get<0>(), i18n::tr("link.client.call-back"), false}}});
            }
          }
          // Texts and Contacts of the phone presence() shows; Link1 keeps its newest 256 texts.
          const std::string phone = presence().phone.id;
          using Part = bus::Struct<std::uint64_t, std::string>;
          using Sms = bus::Struct<std::uint64_t, std::uint64_t, std::string, std::string, std::uint64_t, std::uint8_t,
                                    std::vector<Part>>;
          const auto texts = property<std::map<std::string, std::vector<Sms>>>(props, "Texts");
          std::vector<Text> list;
          if (const auto it = texts.find(phone); it != texts.end()) {
            list.reserve(it->second.size());
            for (const Sms& t : it->second) {
              const auto status = t.get<5>() <= static_cast<std::uint8_t>(Text::Status::Failed)
                                      ? static_cast<Text::Status>(t.get<5>())
                                      : Text::Status::Sent;
              Text text{.id = t.get<0>(), .thread = t.get<1>(), .address = t.get<2>(), .body = t.get<3>(),
                        .atMs = static_cast<std::int64_t>(t.get<4>()), .status = status};
              for (const Part& part : t.get<6>()) {
                if (part.get<1>().starts_with("image/")) text.images.push_back(part.get<0>());
              }
              list.push_back(std::move(text));
            }
          }
          using Person = bus::Struct<std::string, std::vector<std::string>>;
          const auto contacts = property<std::map<std::string, std::vector<Person>>>(props, "Contacts");
          std::vector<Contact> people;
          if (const auto it = contacts.find(phone); it != contacts.end()) {
            people.reserve(it->second.size());
            for (const Person& c : it->second) people.push_back({.name = c.get<0>(), .numbers = c.get<1>()});
          }
          m_calls.clear();
          const auto log = property<std::map<std::string, std::vector<Call>>>(props, "Calls");
          if (const auto it = log.find(phone); it != log.end()) {
            m_calls.reserve(it->second.size());
            for (const Call& c : it->second) {
              const std::string& kind = c.get<4>();
              m_calls.push_back({.number = c.get<0>(), .name = c.get<1>(), .atMs = static_cast<std::int64_t>(c.get<2>()),
                                 .seconds = c.get<3>(),
                                 .kind = kind == "missed"     ? CallEntry::Kind::Missed
                                         : kind == "outgoing" ? CallEntry::Kind::Outgoing
                                                              : CallEntry::Kind::Incoming});
            }
          }
          const auto refused = property<std::map<std::string, std::string>>(props, "CallsRefused");
          const auto why = refused.find(phone);
          m_callLog = callLogFrom(why == refused.end() ? std::string() : why->second);
          const auto grants = property<std::map<std::string, std::vector<std::string>>>(props, "Grants");
          const auto granted = [&](const char* feature) {
            const auto it = grants.find(phone);
            return it != grants.end() && std::ranges::contains(it->second, std::string(feature));
          };
          m_phoneSettings = {.notifications = granted("notifications"), .clipboard = granted("clipboard"),
                             .screen = granted("screen"),
                             .nearby = granted("nearby"), .commands = granted("commands"),
                             .files = granted("files"), .media = granted("media"), .input = granted("input"),
                             .system = granted("system"), .ring = granted("ring"), .calls = granted("calls"),
                             .autoAccept = std::ranges::contains(property<std::vector<std::string>>(props, "AutoAccept"), phone),
                             .notificationsOff = std::ranges::contains(property<std::vector<std::string>>(props, "NotificationsOff"), phone),
                             .inputBlocked = granted("input") && !std::ranges::contains(
                                 property<std::vector<std::string>>(props, "SessionCapabilities"), std::string("input"))};
          m_desktopSettings.notificationsWhileLocked = property<bool>(props, "NotificationsWhileLocked");
          m_desktopSettings.localSendVisible = property<bool>(props, "LocalSendVisible");
          m_desktopSettings.commands.clear();
          for (const auto& [name, line] : property<std::vector<bus::Struct<std::string, std::string>>>(props, "Commands"))
            m_desktopSettings.commands.push_back({.name = name, .line = line});
          m_messages.setContacts(people);
          m_messages.setTexts(phone, std::move(list));
          const Presence shown = presence();
          if (shown.kind == Presence::Kind::Connected &&
              (m_photos.load() == Photos::Load::Idle || m_photos.device() != shown.phone.id)) {
            loadPhotos();
          }
          changed();
        });
  }

  void LinkClient::refreshNearby() {
    const std::uint64_t serial = ++m_nearbySent;
    m_proxy->callMethodAsync("GetAll")
        .onInterface("org.freedesktop.DBus.Properties")
        .withArguments(std::string{kQuickShare})
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](
                             std::optional<bus::Error> error, std::map<std::string, bus::Variant> props) {
          if (alive.expired() || serial <= m_nearbyApplied || error) return;
          m_nearbyApplied = serial;
          m_desktopSettings.nearbyVisible = property<bool>(props, "Visible");
          m_desktopSettings.nearbyName = property<std::string>(props, "Name");
          changed();
        });
  }

  void LinkClient::answerHotspot(bool allow, bool always) {
    if (!m_hotspotAsk) return;
    const std::string device = std::exchange(m_hotspotAsk, std::nullopt)->deviceId;
    if (always) setGrant(device, "nearby", true);
    m_proxy->callMethodAsync("AnswerHotspot").onInterface(kInterface).withArguments(device, allow)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("AnswerHotspot: {}", error->getMessage());
        });
    changed();
  }

  void LinkClient::setCommands(const std::vector<Command>& commands, std::function<void(std::optional<std::string>)> done) {
    std::vector<bus::Struct<std::string, std::string>> list;
    for (const Command& c : commands) list.emplace_back(c.name, c.line);
    m_proxy->callMethodAsync("SetCommands").onInterface(kInterface).withArguments(list)
        .uponReplyInvoke([done = std::move(done)](std::optional<bus::Error> error) {
          if (error) kLog.warn("SetCommands: {}", error->getMessage());
          done(error ? std::optional<std::string>(error->getMessage()) : std::nullopt);
        });
  }

  void LinkClient::setGrant(const std::string& deviceId, const std::string& feature, bool granted) {
    m_proxy->callMethodAsync("SetGrant").onInterface(kInterface).withArguments(deviceId, feature, granted)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SetGrant: {}", error->getMessage());
        });
  }

  void LinkClient::setAutoAccept(const std::string& deviceId, bool on) {
    m_proxy->callMethodAsync("SetAutoAccept").onInterface(kInterface).withArguments(deviceId, on)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SetAutoAccept: {}", error->getMessage());
        });
  }

  void LinkClient::setNotificationsWhileLocked(bool on) {
    m_proxy->callMethodAsync("SetNotificationsWhileLocked").onInterface(kInterface).withArguments(on)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SetNotificationsWhileLocked: {}", error->getMessage());
        });
  }

  void LinkClient::setLocalSendVisible(bool on) {
    m_proxy->callMethodAsync("SetLocalSendVisible").onInterface(kInterface).withArguments(on)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SetLocalSendVisible: {}", error->getMessage());
        });
  }

  void LinkClient::setNearbyVisible(bool on) {
    m_proxy->callMethodAsync("Set").onInterface("org.freedesktop.DBus.Properties")
        .withArguments(std::string{kQuickShare}, std::string("Visible"), bus::Variant(on))
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("QuickShare Visible: {}", error->getMessage());
        });
  }

  void LinkClient::unpair(const std::string& deviceId) {
    m_proxy->callMethodAsync("Unpair").onInterface(kInterface).withArguments(deviceId)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Unpair: {}", error->getMessage());
        });
  }

  void LinkClient::viewThread(std::optional<std::uint64_t> thread) { m_messages.view(thread); }

  void LinkClient::sendText(std::uint64_t thread, const std::string& body) {
    const std::uint64_t serial = m_messages.send(thread, body, nowMs());
    if (serial == 0) return;
    send(serial);
    changed();
  }

  void LinkClient::retryText(std::uint64_t serial) {
    if (!m_messages.retry(serial)) return;
    send(serial);
    changed();
  }

  void LinkClient::send(std::uint64_t serial) {
    const Outgoing* out = m_messages.outgoing(serial);
    const Thread* thread = out ? m_messages.thread(out->thread) : nullptr;
    if (thread == nullptr) return;
    m_proxy->callMethodAsync("SendText").onInterface(kInterface).withArguments(out->phone, thread->address, out->body)
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error) {
          if (alive.expired() || !error) return;
          kLog.warn("SendText: {}", error->getMessage());
          if (m_messages.sendFailed(serial)) changed();
        });
  }

  void LinkClient::reply(const FeedItem& item, const std::string& text) {
    if (item.kind == FeedItem::Kind::Text) {
      m_proxy->callMethodAsync("SendText").onInterface(kInterface).withArguments(item.deviceId, item.who, text)
          .uponReplyInvoke([](std::optional<bus::Error> error) {
            if (error) kLog.warn("SendText: {}", error->getMessage());
          });
    } else {
      const auto reply = std::ranges::find_if(item.actions, &FeedAction::reply);
      if (reply == item.actions.end()) return;
      m_proxy->callMethodAsync("NotificationAction").onInterface(kInterface)
          .withArguments(item.deviceId, item.key.substr(2), reply->id, text)
          .uponReplyInvoke([](std::optional<bus::Error> error) {
            if (error) kLog.warn("NotificationAction: {}", error->getMessage());
          });
    }
    if (m_feed.remove(item.key)) changed();
  }

  void LinkClient::action(const FeedItem& item, const std::string& actionId) {
    if (item.kind == FeedItem::Kind::MissedCall) {
      dial(item.deviceId, actionId);
      return;
    }
    m_proxy->callMethodAsync("NotificationAction").onInterface(kInterface)
        .withArguments(item.deviceId, item.key.substr(2), actionId, std::string{})
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("NotificationAction: {}", error->getMessage());
        });
  }

  void LinkClient::dismiss(const FeedItem& item) {
    if (item.kind == FeedItem::Kind::Notification) {
      m_proxy->callMethodAsync("NotificationDismiss").onInterface(kInterface).withArguments(item.deviceId, item.key.substr(2))
          .uponReplyInvoke([](std::optional<bus::Error> error) {
            if (error) kLog.warn("NotificationDismiss: {}", error->getMessage());
          });
    }
    if (item.kind == FeedItem::Kind::MissedCall) m_dismissed.insert(item.key);
    if (item.kind == FeedItem::Kind::Photos) markPhotosSeen();
    if (m_feed.remove(item.key) || item.kind == FeedItem::Kind::Photos) changed();
  }

  void LinkClient::accept(const FeedItem& offer) { acceptTransfer(offer.key.substr(2)); }

  void LinkClient::decline(const FeedItem& offer) { declineTransfer(offer.key.substr(2)); }

  void LinkClient::acceptTransfer(const std::string& id) {
    m_proxy->callMethodAsync("AcceptTransfer").onInterface(kInterface).withArguments(id)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("AcceptTransfer: {}", error->getMessage());
        });
  }

  // TransferFinished "declined" removes the Activity entry; the Waiting item leaves now.
  void LinkClient::declineTransfer(const std::string& id) {
    m_proxy->callMethodAsync("DeclineTransfer").onInterface(kInterface).withArguments(id)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("DeclineTransfer: {}", error->getMessage());
        });
    if (m_feed.remove("o:" + id)) changed();
  }

  void LinkClient::cancelTransfer(const std::string& id) {
    m_proxy->callMethodAsync("CancelTransfer").onInterface(kInterface).withArguments(id)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("CancelTransfer: {}", error->getMessage());
        });
  }

  void LinkClient::clearFinished() {
    m_activity.clearFinished();
    changed();
  }

  void LinkClient::showInFolder(const std::vector<std::string>& paths) {
    if (m_showInFolder) m_showInFolder(paths);
  }

  void LinkClient::loadPhotos() {
    const std::string device = presence().phone.id;
    if (device.empty()) return;
    if (device != m_photos.device()) {
      clearThumbs();
      (void)m_feed.remove("p:new");
      m_photosSeenS = 0;
      std::ifstream(photosSeenFile(device)) >> m_photosSeenS;
    }
    if (const auto ask = m_photos.startLoad(device)) askPhotos(*ask);
  }

  void LinkClient::loadMorePhotos() {
    if (m_photos.device().empty() || m_photos.device() != presence().phone.id) return;
    if (const auto ask = m_photos.startMore()) askPhotos(*ask);
  }

  void LinkClient::setPhotoMedia(Photos::Media media) {
    if (m_photos.setMedia(media)) loadPhotos();
    changed();
  }

  void LinkClient::askPhotos(const Photos::Ask& ask) {
    static constexpr std::array<const char*, 3> kMedia{"all", "photos", "videos"};
    using Row = bus::Struct<std::string, std::string, std::uint64_t, bool, std::uint32_t>;
    m_proxy->callMethodAsync("PhotosPage")
        .onInterface(kInterface)
        .withArguments(m_photos.device(), ask.offset, Photos::kCount, std::string(kMedia[static_cast<std::size_t>(ask.media)]))
        .uponReplyInvoke([this, ask, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error,
                                                                            std::uint32_t total, std::vector<Row> list) {
          if (alive.expired()) return;
          if (error) {
            kLog.warn("PhotosPage: {}", error->getMessage());
            const bool refused = error->getName().ends_with(".Refused");
            if (m_photos.failed(ask.serial, !refused                             ? Photos::Load::Failed
                                            : error->getMessage() == "denied" ? Photos::Load::Denied
                                                                              : Photos::Load::Unsupported)) {
              changed();
            }
            return;
          }
          std::vector<Photo> page;
          page.reserve(list.size());
          for (const auto& p : list) {
            page.push_back({.path = p.get<0>(),
                            .name = p.get<1>(),
                            .atS = static_cast<std::int64_t>(p.get<2>()),
                            .videoS = p.get<3>() ? std::optional(p.get<4>()) : std::nullopt});
          }
          if (!m_photos.loaded(ask.serial, total, std::move(page))) return;
          // The Waiting strip counts the newest items of a first page that holds photos.
          if (ask.offset == 0 && ask.media != Photos::Media::Videos) {
            if (auto item = newPhotosItem(m_photos.device(), m_photos.items(), m_photosSeenS, nowMs())) {
              m_feed.upsert(std::move(*item));
            } else {
              (void)m_feed.remove("p:new");
            }
          }
          // A page whose names were all dropped draws no tile that could ask for the next one.
          if (m_photos.items().size() < Photos::kCount) loadMorePhotos();
          changed();
        });
  }

  void LinkClient::clearThumbs() {
    m_thumbs.clear();
    m_thumbUse.clear();
    m_thumbBytes = 0;
    m_thumbTicket = m_thumbEpoch.begin();
  }

  void LinkClient::markPhotosSeen() {
    const std::vector<Photo>& photos = m_photos.items();
    if (!photos.empty() && photos.front().atS > m_photosSeenS) {
      m_photosSeenS = photos.front().atS;
      andromeda::writeFileAsync(photosSeenFile(m_photos.device()), std::format("{}\n", m_photosSeenS));
    }
    (void)m_feed.remove("p:new");
  }

  void LinkClient::togglePhoto(std::size_t index) {
    m_photos.toggle(index);
    changed();
  }

  void LinkClient::clearPhotoSelection() {
    m_photos.clearSelection();
    changed();
  }

  void LinkClient::savePhotos() {
    const std::vector<std::string> paths = m_photos.startSave();
    for (const std::string& path : paths) {
      m_proxy->callMethodAsync("SavePhoto")
          .onInterface(kInterface)
          .withArguments(m_photos.device(), path)
          .uponReplyInvoke([this, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error, std::string) {
            if (alive.expired()) return;
            if (error) kLog.warn("SavePhoto: {}", error->getMessage());
            m_photos.saveAnswered(!error);
            changed();
          });
    }
    if (!paths.empty()) changed();
  }

  void LinkClient::savePhotoTo(const std::string& path, const std::string& folder,
                               std::function<void(const std::string&)> done) {
    m_proxy->callMethodAsync("SavePhotoTo")
        .onInterface(kInterface)
        .withArguments(m_photos.device(), path, folder)
        .uponReplyInvoke([done = std::move(done)](std::optional<bus::Error> error, std::string saved) {
          if (error) kLog.warn("SavePhotoTo: {}", error->getMessage());
          done(error ? std::string() : saved);
        });
  }

  const std::vector<std::uint8_t>* LinkClient::thumbnail(const std::string& path) {
    // The key holds the phone: after a phone switch, a late reply for the same path must not fill the new phone's tile.
    return cachedImage(std::format("photo:{}:{}", m_photos.device(), path), [&] {
      return m_proxy->callMethodAsync("Thumbnail").onInterface(kInterface).withArguments(m_photos.device(), path, std::uint16_t{256});
    });
  }

  const std::vector<std::uint8_t>* LinkClient::textImage(std::uint64_t part) {
    const std::string device = presence().phone.id;
    if (device.empty()) return nullptr;
    // Twice the bubble's 240 x 160, for 2x scales.
    return cachedImage(std::format("mms:{}:{}", device, part), [&] {
      return m_proxy->callMethodAsync("TextImage").onInterface(kInterface).withArguments(device, part, std::uint16_t{480});
    });
  }

  template <typename Ask> const std::vector<std::uint8_t>* LinkClient::cachedImage(const std::string& key, Ask ask) {
    if (const auto it = m_thumbs.find(key); it != m_thumbs.end()) {
      m_thumbUse.splice(m_thumbUse.begin(), m_thumbUse, it->second.use);
      return it->second.png.empty() ? nullptr : &it->second.png;
    }
    m_thumbUse.push_front(key);
    m_thumbs.emplace(key, Thumb{.png = {}, .use = m_thumbUse.begin()});
    ask().uponReplyInvoke([this, key, ticket = m_thumbTicket, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error,
                                                                             std::vector<std::uint8_t> png) {
          if (alive.expired() || !m_thumbEpoch.current(ticket)) return;
          const auto entry = m_thumbs.find(key);
          if (entry == m_thumbs.end()) return;
          if (error) {
            kLog.debug("image {}: {}", key, error->getMessage());
            m_thumbUse.erase(entry->second.use);
            m_thumbs.erase(entry); // the next bind of its tile asks again
            return;
          }
          m_thumbBytes += png.size() - entry->second.png.size();
          entry->second.png = std::move(png);
          while (m_thumbBytes > kThumbBytes && m_thumbUse.size() > 1) {
            const auto old = m_thumbs.find(m_thumbUse.back());
            m_thumbBytes -= old->second.png.size();
            m_thumbs.erase(old);
            m_thumbUse.pop_back();
          }
          if (m_thumbnailReady) m_thumbnailReady();
        });
    return nullptr;
  }

  void LinkClient::openFolder(const std::string& path) {
    const std::string device = presence().phone.id;
    if (device.empty()) return;
    const std::uint64_t serial = m_files.go(path);
    changed();
    m_proxy->callMethodAsync("ListFiles")
        .onInterface(kInterface)
        .withArguments(device, path)
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](
                             std::optional<bus::Error> error, std::vector<bus::Struct<std::string, bool, std::uint64_t, std::uint64_t>> list) {
          if (alive.expired()) return;
          if (error) {
            const std::string why = error->getName().ends_with(".Refused") ? error->getMessage() : std::string();
            kLog.warn("ListFiles: {}", error->getMessage());
            const std::string text = why == "not-allowed" || why == "denied"
                                         ? i18n::tr("link.client.files-off")
                                     : why == "not-found" ? i18n::tr("link.client.folder-gone")
                                     : why == "busy"      ? i18n::tr("link.client.busy")
                                                          : i18n::tr("link.client.no-answer");
            if (m_files.failed(serial, text)) changed();
            return;
          }
          std::vector<FileEntry> entries;
          entries.reserve(list.size());
          for (const auto& e : list) {
            entries.push_back({.name = e.get<0>(), .folder = e.get<1>(), .size = e.get<2>(),
                               .mtimeS = static_cast<std::int64_t>(e.get<3>())});
          }
          if (m_files.listed(serial, std::move(entries))) changed();
        });
  }

  namespace {
    // A failed DeleteFile or WriteFile as one line for the Files page (reasons: Link1.xml ListFiles).
    std::string fileError(const bus::Error& error, const std::string& name, bool exists) {
      const std::string why = error.getName().ends_with(".Refused") ? error.getMessage() : std::string();
      if (why == "not-allowed") return i18n::tr("link.client.files-off");
      if (why == "denied" && exists) return i18n::tr("link.client.exists", "name", name);
      if (why == "denied") return i18n::tr("link.client.denied", "name", name);
      if (why == "not-found") return i18n::tr("link.client.file-gone", "name", name);
      if (why == "busy") return i18n::tr("link.client.busy");
      if (why == "io") return i18n::tr("link.client.io");
      return i18n::tr("link.client.no-answer");
    }
  } // namespace

  void LinkClient::deleteFile(const std::string& phonePath) {
    const std::string device = presence().phone.id;
    if (device.empty()) return;
    const std::string name = phonePath.substr(phonePath.find_last_of('/') + 1);
    m_proxy->callMethodAsync("DeleteFile").onInterface(kInterface).withArguments(device, phonePath)
        .uponReplyInvoke([this, name, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error) {
          if (alive.expired()) return;
          if (error) {
            kLog.warn("DeleteFile: {}", error->getMessage());
            if (m_fileOp.kind != FileOp::Kind::Copying) m_fileOp = {.kind = FileOp::Kind::Failed, .error = fileError(*error, name, false)};
          }
          openFolder(m_files.path());
        });
  }

  struct LinkClient::Upload {
    std::string device;
    std::string folder; // the phone folder the copy started in
    std::vector<std::pair<std::filesystem::path, std::string>> files; // local path, phone path
    std::size_t index = 0;
    std::unique_ptr<std::FILE, int (*)(std::FILE*)> file{nullptr, &std::fclose};
    std::uint64_t offset = 0; // in the current file
  };

  void LinkClient::copyIntoFolder(const std::vector<std::filesystem::path>& paths) {
    const std::string device = presence().phone.id;
    if (device.empty() || m_fileOp.kind == FileOp::Kind::Copying) return;
    auto upload = std::make_shared<Upload>();
    upload->device = device;
    upload->folder = m_files.path();
    std::uint64_t total = 0;
    for (const auto& path : paths) {
      std::error_code error;
      if (!std::filesystem::is_regular_file(path, error)) continue;
      total += std::filesystem::file_size(path, error);
      upload->files.emplace_back(path, m_files.childPath(path.filename().string()));
    }
    if (upload->files.empty()) return;
    m_upload = upload;
    m_fileOp = {.kind = FileOp::Kind::Copying, .count = upload->files.size(), .total = total};
    writeChunk(upload);
  }

  void LinkClient::writeChunk(const std::shared_ptr<Upload>& upload) {
    constexpr std::size_t kChunk = 48 * 1024; // Link1 WriteFile's largest write
    const auto& [local, phonePath] = upload->files[upload->index];
    if (!upload->file) {
      upload->file.reset(std::fopen(local.c_str(), "rb"));
      upload->offset = 0;
      m_fileOp.index = upload->index;
      m_fileOp.name = local.filename().string();
      changed();
      if (!upload->file) {
        finishUpload(upload, i18n::tr("link.client.unreadable", "name", m_fileOp.name));
        return;
      }
    }
    std::vector<std::uint8_t> data(kChunk);
    data.resize(std::fread(data.data(), 1, kChunk, upload->file.get()));
    const bool last = data.size() < kChunk;
    if (data.empty() && upload->offset > 0) {
      upload->file.reset();
      if (++upload->index == upload->files.size()) {
        finishUpload(upload, std::nullopt);
      } else {
        writeChunk(upload);
      }
      return;
    }
    const std::uint64_t size = data.size();
    m_proxy->callMethodAsync("WriteFile")
        .onInterface(kInterface)
        .withArguments(upload->device, phonePath, upload->offset, data)
        .uponReplyInvoke([this, upload, size, last, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error,
                                                                                           std::uint64_t) {
          if (alive.expired()) return;
          if (upload != m_upload) {
            // Cancelled while its first chunk was on the way: the phone now holds part of the file.
            if (!error && upload->offset == 0 && !last) {
              m_proxy->callMethodAsync("DeleteFile").onInterface(kInterface)
                  .withArguments(upload->device, upload->files[upload->index].second)
                  .uponReplyInvoke([](std::optional<bus::Error> deleteError) {
                    if (deleteError) kLog.debug("DeleteFile failed: {}", deleteError->getMessage());
                  });
            }
            return;
          }
          if (error) {
            kLog.warn("WriteFile: {}", error->getMessage());
            finishUpload(upload, fileError(*error, m_fileOp.name, upload->offset == 0));
            return;
          }
          upload->offset += size;
          m_fileOp.sent += size;
          changed();
          if (last) {
            upload->file.reset();
            if (++upload->index == upload->files.size()) {
              finishUpload(upload, std::nullopt);
              return;
            }
          }
          writeChunk(upload);
        });
  }

  void LinkClient::finishUpload(const std::shared_ptr<Upload>& upload, std::optional<std::string> error) {
    // A file the phone holds only part of is deleted, unless the phone refused to create it at all.
    if (error && upload->index < upload->files.size() && upload->offset > 0) {
      m_proxy->callMethodAsync("DeleteFile").onInterface(kInterface)
          .withArguments(upload->device, upload->files[upload->index].second)
          .uponReplyInvoke([](std::optional<bus::Error> failed) {
            if (failed) kLog.warn("DeleteFile partial: {}", failed->getMessage());
          });
    }
    m_upload.reset();
    m_fileOp = error ? FileOp{.kind = FileOp::Kind::Failed, .error = std::move(*error)} : FileOp{};
    if (m_files.path() == upload->folder) openFolder(upload->folder);
    changed();
  }

  void LinkClient::cancelCopy() {
    if (!m_upload) return;
    const std::shared_ptr<Upload> upload = m_upload;
    finishUpload(upload, std::string());
    m_fileOp = {};
    changed();
  }

  void LinkClient::dismissFileOp() {
    if (m_fileOp.kind != FileOp::Kind::Failed) return;
    m_fileOp = {};
    changed();
  }

  struct LinkClient::Download {
    std::string device;
    std::string phonePath;
    std::string local;
    struct Close {
      void operator()(std::FILE* file) const { std::fclose(file); }
    };
    std::unique_ptr<std::FILE, Close> file;
    std::uint64_t got = 0;
    std::chrono::steady_clock::time_point shown;
    std::function<void(std::uint64_t got)> progress;
    std::function<void(bool ok)> finished; // after the file is closed (kept) or removed
    std::function<bool()> stale;           // true: stop reading and drop the file
  };

  void LinkClient::saveFile(const std::string& name) {
    const auto entry = std::ranges::find(m_files.entries(), name, &FileEntry::name);
    const std::string device = presence().phone.id;
    if (entry == m_files.entries().end() || entry->folder || device.empty()) return;
    const std::string phonePath = m_files.childPath(name);
    if (!m_files.saveStarted(phonePath, entry->size)) return;
    auto download = std::make_shared<Download>();
    download->device = device;
    download->phonePath = phonePath;
    // A free name in the downloads folder: "name", then "name (1).ext", ...
    const std::filesystem::path dir = g_get_user_special_dir(G_USER_DIRECTORY_DOWNLOAD) != nullptr
                                          ? g_get_user_special_dir(G_USER_DIRECTORY_DOWNLOAD)
                                          : std::filesystem::path(g_get_home_dir()) / "Downloads";
    download->progress = [this, phonePath](std::uint64_t got) { (void)m_files.saveProgress(phonePath, got); };
    download->finished = [this, d = download.get()](bool ok) {
      (void)(ok ? m_files.saveDone(d->phonePath, d->local) : m_files.saveFailed(d->phonePath));
    };
    changed();
    startDownload(download, [dir, name](Download& d) {
      const std::filesystem::path base(name);
      for (int n = 0; n < 1000 && !d.file; ++n) {
        const std::string candidate =
            n == 0 ? name : std::format("{} ({}){}", base.stem().string(), n, base.extension().string());
        d.local = (dir / candidate).string();
        d.file.reset(std::fopen(d.local.c_str(), "wbx"));
        if (!d.file && errno != EEXIST) break;
      }
      if (!d.file) kLog.warn("save {}: cannot create a file in {}", name, dir.string());
      return d.file != nullptr;
    });
  }

  // The file opens, takes each chunk and closes on the file-op thread (ordered); the next read starts after its write.
  void LinkClient::startDownload(const std::shared_ptr<Download>& download, std::function<bool(Download&)> open) {
    andromeda::runFileOpAsync([download, open = std::move(open)] { return open(*download); },
                              [this, download, alive = std::weak_ptr<bool>(m_alive)](bool ok) {
                                if (alive.expired()) return;
                                if (!ok) {
                                  download->finished(false);
                                  changed();
                                  return;
                                }
                                readChunk(download);
                              });
  }

  void LinkClient::readChunk(const std::shared_ptr<Download>& download) {
    constexpr std::uint32_t kChunk = 1024 * 1024; // Link1 ReadFile's largest read
    m_proxy->callMethodAsync("ReadFile")
        .onInterface(kInterface)
        .withArguments(download->device, download->phonePath, download->got, kChunk)
        .uponReplyInvoke([this, download, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error,
                                                                                 std::vector<std::uint8_t> data) {
          if (alive.expired()) return;
          const auto drop = [](Download& d) {
            d.file.reset();
            std::remove(d.local.c_str());
          };
          if (download->stale && download->stale()) {
            andromeda::runFileOpAsync([download, drop] {
              drop(*download);
              return true;
            });
            return;
          }
          if (error) kLog.warn("ReadFile: {}", error->getMessage());
          const std::size_t size = data.size();
          const bool last = size < kChunk;
          andromeda::runFileOpAsync(
              [download, drop, data = std::move(data), failed = error.has_value(), last] {
                if (failed || std::fwrite(data.data(), 1, data.size(), download->file.get()) != data.size()
                    || (last && std::fclose(download->file.release()) != 0)) {
                  drop(*download);
                  return false;
                }
                return true;
              },
              [this, download, alive, size, last](bool ok) {
                if (alive.expired()) return;
                if (!ok || last) {
                  download->got += ok ? size : 0;
                  download->finished(ok);
                  changed();
                  return;
                }
                download->got += size;
                // Progress repaints at most 4 times a second (rulebook §16); a read with no progress to show repaints only at its end.
                const auto now = std::chrono::steady_clock::now();
                if (download->progress) download->progress(download->got);
                if (download->progress && now - download->shown >= std::chrono::milliseconds(250)) {
                  download->shown = now;
                  changed();
                }
                readChunk(download);
              });
        });
  }

  void LinkClient::startMirror() {
    const std::string device = presence().phone.id;
    if (device.empty() || !mirrorStep(m_mirror, {.kind = MirrorEvent::Kind::Start})) return;
    m_mirrorDevice = device;
    m_mirrorCorner = 0;
    m_mirrorScreenOff = false;
    const std::uint64_t session = m_mirror.session;
    changed();
    openMirror(device, session, true);
  }

  // `withCodecs` asks for OpenMirrorWith; a linkd without it (UnknownMethod) gets OpenMirror, which is always H.264.
  void LinkClient::openMirror(const std::string& device, std::uint64_t session, bool withCodecs) {
    const auto opened = [this, session](bus::UnixFd fd, std::uint32_t width, std::uint32_t height, const std::string& name) {
      const std::vector<std::string>& listed = mirrorCodecs();
      const std::optional<MirrorCodec> codec = mirrorCodecFrom(name);
      if (!codec || (*codec != MirrorCodec::H264 && std::ranges::find(listed, name) == listed.end())) {
        kLog.warn("OpenMirror: the phone chose codec '{}', not offered", name);
        if (mirrorStep(m_mirror, {.kind = MirrorEvent::Kind::Failed, .session = session,
                                  .error = i18n::tr("link.client.bad-video")})) {
          changed();
        }
        return; // fd closes here
      }
      if (!mirrorStep(m_mirror, {.kind = MirrorEvent::Kind::Opened, .session = session, .width = width, .height = height})) {
        return; // stopped or restarted meanwhile: fd closes here
      }
      if (m_mirror.kind == Mirror::Kind::Failed) {
        changed(); // no screen size: fd closes here
        return;
      }
      m_decoder = std::make_unique<MirrorDecoder>(
          fd.release(), *codec,
          [this](MirrorDecoder::Frame frame) {
            if (m_mirrorFrame) m_mirrorFrame(std::move(frame));
          },
          [this, session](std::string why) {
            const MirrorEvent event = why.empty() ? MirrorEvent{.kind = MirrorEvent::Kind::Ended, .session = session}
                                                  : MirrorEvent{.kind = MirrorEvent::Kind::Failed, .session = session, .error = why};
            if (mirrorStep(m_mirror, event)) {
              m_decoder.reset();
              changed();
            }
          });
      changed();
    };
    const auto failed = [this, session, device, withCodecs](const bus::Error& error) {
      if (withCodecs && error.getName() == "org.freedesktop.DBus.Error.UnknownMethod") {
        openMirror(device, session, false);
        return;
      }
      kLog.warn("OpenMirror: {}", error.getMessage());
      const std::string why = error.getName().ends_with(".Refused") ? i18n::tr("link.client.share-refused")
                              : error.getName().ends_with(".NotConnected") ? i18n::tr("link.client.not-connected")
                                                                           : i18n::tr("link.client.share-failed");
      if (mirrorStep(m_mirror, {.kind = MirrorEvent::Kind::Failed, .session = session, .error = why})) changed();
    };
    // The phone's user has 60 s to allow capture; the default D-Bus timeout is shorter.
    if (withCodecs) {
      m_proxy->callMethodAsync("OpenMirrorWith")
          .onInterface(kInterface)
          .withArguments(device, mirrorCodecs())
          .withTimeout(std::chrono::seconds(70))
          .uponReplyInvoke([opened, failed, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error, bus::UnixFd fd,
                                                                                  std::uint32_t width, std::uint32_t height,
                                                                                  std::string codec) {
            if (alive.expired()) return;
            if (error) failed(*error);
            else opened(std::move(fd), width, height, codec);
          });
      return;
    }
    m_proxy->callMethodAsync("OpenMirror")
        .onInterface(kInterface)
        .withArguments(device)
        .withTimeout(std::chrono::seconds(70))
        .uponReplyInvoke([opened, failed, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error, bus::UnixFd fd,
                                                                                std::uint32_t width, std::uint32_t height) {
          if (alive.expired()) return;
          if (error) failed(*error);
          else opened(std::move(fd), width, height, "h264");
        });
  }

  void LinkClient::stopMirror() {
    if (mirrorScreenOff()) setMirrorScreenOff(false);
    if (!mirrorStep(m_mirror, {.kind = MirrorEvent::Kind::Stop})) return;
    m_decoder.reset();
    changed();
  }

  void LinkClient::mirrorAction(const std::string& action) {
    if (m_mirror.kind != Mirror::Kind::Showing) return;
    m_proxy->callMethodAsync("MirrorInput")
        .onInterface(kInterface)
        .withArguments(m_mirrorDevice, action, std::map<std::string, bus::Variant>{})
        .uponReplyInvoke([action](std::optional<bus::Error> error) {
          if (error) kLog.warn("MirrorInput {}: {}", action, error->getMessage());
        });
  }

  void LinkClient::mirrorInput(const std::string& action, std::map<std::string, bus::Variant> args) {
    if (m_mirror.kind != Mirror::Kind::Showing) return;
    m_proxy->callMethodAsync("MirrorInput")
        .onInterface(kInterface)
        .withArguments(m_mirrorDevice, action, args)
        .uponReplyInvoke([action](std::optional<bus::Error> error) {
          if (error) kLog.warn("MirrorInput {}: {}", action, error->getMessage());
        });
  }

  void LinkClient::mirrorText(const std::string& text) { mirrorInput("text", {{"text", bus::Variant(text)}}); }

  void LinkClient::mirrorKeyEvent(std::int32_t code, std::int32_t meta, bool down) {
    mirrorInput(down ? "key-down" : "key-up", {{"code", bus::Variant(code)}, {"meta", bus::Variant(meta)}});
  }

  void LinkClient::mirrorWheel(std::int32_t x, std::int32_t y, std::int32_t dx, std::int32_t dy) {
    mirrorInput("wheel", {{"x", bus::Variant(x)}, {"y", bus::Variant(y)}, {"x2", bus::Variant(dx)},
                          {"y2", bus::Variant(dy)}});
  }

  void LinkClient::setMirrorScreenOff(bool off) {
    if (m_mirror.kind != Mirror::Kind::Showing) return;
    m_mirrorScreenOff = off;
    mirrorInput(off ? "screen-off" : "screen-on", {});
    changed();
  }

  void LinkClient::mirrorKey(std::int32_t code) {
    if (m_mirror.kind != Mirror::Kind::Showing) return;
    for (const char* action : {"key-down", "key-up"}) {
      m_proxy->callMethodAsync("MirrorInput")
          .onInterface(kInterface)
          .withArguments(m_mirrorDevice, std::string(action),
                         std::map<std::string, bus::Variant>{{"code", bus::Variant(code)}, {"meta", bus::Variant(std::int32_t{0})}})
          .uponReplyInvoke([](std::optional<bus::Error> error) {
            if (error) kLog.warn("MirrorInput key: {}", error->getMessage());
          });
    }
  }

  void LinkClient::mirrorTouch(const std::string& action, std::int32_t x, std::int32_t y) {
    if (m_mirror.kind != Mirror::Kind::Showing) return;
    m_proxy->callMethodAsync("MirrorInput")
        .onInterface(kInterface)
        .withArguments(m_mirrorDevice, action, std::map<std::string, bus::Variant>{{"x", bus::Variant(x)}, {"y", bus::Variant(y)}})
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("MirrorInput touch: {}", error->getMessage());
        });
  }

  void LinkClient::setMirrorTarget(int width, int height) {
    if (m_decoder) m_decoder->setTarget(width, height);
  }

  void LinkClient::startPairing() {
    if (!pairStep(m_pair, {.kind = PairEvent::Kind::Start})) return;
    const LatestRequest::Ticket ticket = m_pairRequest.begin();
    m_proxy->callMethodAsync("StartPairing")
        .onInterface(kInterface)
        .uponReplyInvoke([this, ticket, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error,
                                                                               std::string code, std::string uri) {
          if (alive.expired() || !m_pairRequest.current(ticket)) return;
          if (error) {
            kLog.warn("StartPairing: {}", error->getMessage());
            (void)pairStep(m_pair, {.kind = PairEvent::Kind::Failed, .error = error->getMessage()});
          } else {
            (void)pairStep(m_pair, {.kind = PairEvent::Kind::Started, .code = std::move(code), .uri = std::move(uri)});
          }
          changed();
        });
  }

  void LinkClient::cancelPairing() {
    const bool open = m_pair.kind == Pair::Kind::Starting || m_pair.kind == Pair::Kind::Showing;
    if (!pairStep(m_pair, {.kind = PairEvent::Kind::Cancel})) return;
    m_pairRequest.cancel();
    if (open) {
      m_proxy->callMethodAsync("CancelPairing").onInterface(kInterface).uponReplyInvoke([](std::optional<bus::Error> error) {
        if (error) kLog.warn("CancelPairing: {}", error->getMessage());
      });
    }
    changed();
  }

  void LinkClient::disconnect(const std::string& deviceId) {
    m_proxy->callMethodAsync("Disconnect").onInterface(kInterface).withArguments(deviceId)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Disconnect: {}", error->getMessage());
        });
  }

  void LinkClient::connect(const std::string& deviceId) {
    m_proxy->callMethodAsync("Connect").onInterface(kInterface).withArguments(deviceId)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Connect: {}", error->getMessage());
        });
  }

  void LinkClient::setPhoneAudio(const std::string& deviceId, PhoneAudio audio, bool on) {
    const char* method =
        audio == PhoneAudio::Mic ? (on ? "StartMic" : "StopMic") : (on ? "StartPhoneSound" : "StopPhoneSound");
    m_proxy->callMethodAsync(method).onInterface(kInterface).withArguments(deviceId)
        .uponReplyInvoke([method](std::optional<bus::Error> error) {
          if (error) kLog.warn("{}: {}", method, error->getMessage());
        });
  }

  void LinkClient::setPhoneCamera(const std::string& deviceId, bool on) {
    auto call = m_proxy->callMethodAsync(on ? "StartCamera" : "StopCamera").onInterface(kInterface);
    const auto reply = [on](std::optional<bus::Error> error) {
      if (error) kLog.warn("{}: {}", on ? "StartCamera" : "StopCamera", error->getMessage());
    };
    if (on) {
      call.withArguments(deviceId, true).uponReplyInvoke(reply);
    } else {
      call.withArguments(deviceId).uponReplyInvoke(reply);
    }
  }

  void LinkClient::switchPhoneCamera(const std::string& deviceId) {
    m_proxy->callMethodAsync("SwitchCamera").onInterface(kInterface).withArguments(deviceId)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SwitchCamera: {}", error->getMessage());
        });
  }

  void LinkClient::dial(const std::string& deviceId, const std::string& number) {
    m_proxy->callMethodAsync("Dial").onInterface(kInterface).withArguments(deviceId, number)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Dial: {}", error->getMessage());
        });
  }

  const PhoneCall* LinkClient::call(const std::string& deviceId) const {
    const auto it = m_phoneCalls.find(deviceId);
    return it != m_phoneCalls.end() ? &it->second : nullptr;
  }

  void LinkClient::callAction(const std::string& deviceId, const std::string& action) {
    if (action == "mute") {
      if (auto it = m_phoneCalls.find(deviceId); it != m_phoneCalls.end() && !it->second.muted) {
        it->second.muted = true;
        changed();
      }
    }
    m_proxy->callMethodAsync("CallAction").onInterface(kInterface).withArguments(deviceId, action)
        .uponReplyInvoke([this, alive = std::weak_ptr<bool>(m_alive), deviceId, action](std::optional<bus::Error> error) {
          if (!error) return;
          kLog.warn("CallAction {}: {}", action, error->getMessage());
          if (alive.expired() || action != "mute") return;
          // The ringer still rings: offer Mute again.
          if (auto it = m_phoneCalls.find(deviceId); it != m_phoneCalls.end() && it->second.muted) {
            it->second.muted = false;
            changed();
          }
        });
  }

  void LinkClient::ring(const std::string& deviceId) {
    m_proxy->callMethodAsync("Ring").onInterface(kInterface).withArguments(deviceId, true)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Ring: {}", error->getMessage());
        });
  }

  void LinkClient::select(const std::string& deviceId) {
    if (deviceId == m_selected) return;
    m_selected = deviceId;
    andromeda::writeFileAsync(selectedFile(), deviceId + '\n');
    refresh();
    changed();
  }

  void LinkClient::sendTo(const std::string& number, const std::string& body) {
    m_proxy->callMethodAsync("SendText").onInterface(kInterface).withArguments(presence().phone.id, number, body)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SendText: {}", error->getMessage());
        });
  }

  void LinkClient::shareText(const std::string& deviceId, const std::string& text) {
    const bool link = (text.starts_with("http://") || text.starts_with("https://")) &&
                      std::ranges::none_of(text, [](unsigned char c) { return std::isspace(c) != 0; });
    m_proxy->callMethodAsync("Share").onInterface(kInterface)
        .withArguments(deviceId, std::string(link ? "link" : "text"), text)
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("Share: {}", error->getMessage());
        });
  }

  void LinkClient::sendFiles(const std::string& deviceId, const std::vector<std::filesystem::path>& paths) {
    std::vector<bus::Struct<bus::UnixFd, std::string>> files;
    std::vector<std::pair<std::string, std::uint64_t>> names;
    std::vector<std::string> sources;
    for (const auto& path : paths) {
      const int fd = ::open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NOCTTY);
      if (fd < 0) {
        kLog.warn("SendFiles: open {}: {}", path.string(), std::strerror(errno));
        continue;
      }
      struct stat st{};
      names.emplace_back(path.filename().string(), ::fstat(fd, &st) == 0 ? static_cast<std::uint64_t>(st.st_size) : 0);
      sources.push_back(path.string());
      files.emplace_back(bus::UnixFd{fd, bus::adopt_fd}, path.filename().string());
    }
    if (files.empty()) return;
    m_proxy->callMethodAsync("SendFiles").onInterface(kInterface).withArguments(deviceId, files)
        .uponReplyInvoke([this, deviceId, names = std::move(names), sources = std::move(sources)](
                             std::optional<bus::Error> error, std::string transfer) mutable {
          if (error) {
            kLog.warn("SendFiles: {}", error->getMessage());
            return;
          }
          m_activity.sent(transfer, deviceId, std::move(names), std::move(sources), nowMs());
          changed();
        });
  }

  void LinkClient::retryTransfer(const std::string& id) {
    const auto it = std::ranges::find(m_activity.items(), id, &TransferEntry::id);
    if (it == m_activity.items().end() || !Activity::retryable(*it)) return;
    const std::string deviceId = it->deviceId;
    const std::vector<std::filesystem::path> paths(it->sources.begin(), it->sources.end());
    m_activity.forget(id);
    changed();
    sendFiles(deviceId, paths);
  }

  void LinkClient::setDnd(const std::string& deviceId, bool on) { sendSystemCommand(deviceId, "set-dnd", on ? 1 : 0); }

  void LinkClient::sendSystemCommand(const std::string& deviceId, const std::string& command, std::uint32_t value) {
    const std::uint64_t serial = ++m_nextCommandId;
    m_command = SystemCommand{.device = deviceId, .command = command, .value = value, .serial = serial};
    changed();
    m_proxy->callMethodAsync("SendSystemCommand").onInterface(kInterface).withArguments(deviceId, command, value)
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error) {
          // A newer command replaced this one.
          if (alive.expired() || !m_command || m_command->serial != serial ||
              m_command->phase != SystemCommand::Phase::Pending) {
            return;
          }
          if (error) {
            kLog.warn("SendSystemCommand {}: {}", m_command->command, error->getMessage());
            m_command->phase = SystemCommand::Phase::Failed;
            m_command->error = error->getMessage();
            changed();
            return;
          }
          // The phone ran it; its next status confirms it, and this keeps the key from flicking back until then.
          const auto value = static_cast<int>(m_command->value);
          for (Phone& phone : m_phones) {
            if (phone.id != m_command->device) continue;
            if (m_command->command == "set-dnd") phone.dnd = value;
            if (m_command->command == "set-ringer") phone.ringer = value == 0 ? "silent" : value == 1 ? "vibrate" : "normal";
            if (m_command->command == "set-wifi") phone.wifi = value;
            if (m_command->command == "set-bluetooth") phone.bluetooth = value;
            if (m_command->command == "set-torch") phone.torch = value;
          }
          m_command.reset();
          changed();
        });
  }

  void LinkClient::playerCommand(const std::string& deviceId, const std::string& command, std::uint64_t value) {
    m_proxy->callMethodAsync("PhonePlayerCommand").onInterface(kInterface).withArguments(deviceId, command, value)
        .uponReplyInvoke([command](std::optional<bus::Error> error) {
          if (error) kLog.warn("PhonePlayerCommand {}: {}", command, error->getMessage());
        });
  }

  void LinkClient::setMediaVolume(const std::string& deviceId, int percent) {
    m_proxy->callMethodAsync("SendSystemCommand").onInterface(kInterface)
        .withArguments(deviceId, std::string("set-media-volume"), static_cast<std::uint32_t>(std::clamp(percent, 0, 100)))
        .uponReplyInvoke([](std::optional<bus::Error> error) {
          if (error) kLog.warn("SendSystemCommand set-media-volume: {}", error->getMessage());
        });
  }

  void LinkClient::openPhoto(const std::string& phonePath) {
    if (m_photoFile && m_photoFile->phonePath == phonePath && m_photoFile->kind != PhotoFile::Kind::Failed) return;
    closePhoto();
    const std::filesystem::path dir = std::filesystem::path(g_get_user_cache_dir()) / "andromeda" / "link-app" / "view";
    auto download = std::make_shared<Download>();
    download->device = m_photos.device();
    download->phonePath = phonePath;
    download->local = (dir / std::format("{}{}", m_photoOpens, std::filesystem::path(phonePath).extension().string())).string();
    m_photoFile = PhotoFile{.phonePath = phonePath};
    const std::uint64_t serial = m_photoOpens;
    download->stale = [this, serial] { return serial != m_photoOpens; };
    download->finished = [this, serial, d = download.get()](bool ok) {
      if (serial != m_photoOpens) return;
      m_photoFile->kind = ok ? PhotoFile::Kind::Ready : PhotoFile::Kind::Failed;
      if (ok) m_photoFile->local = d->local;
      m_photoFile->size = d->got;
    };
    changed();
    startDownload(download, [dir, phonePath, clearOld = m_photoOpens == 1](Download& d) {
      std::error_code ec;
      if (clearOld) std::filesystem::remove_all(dir, ec); // files a crash left behind
      std::filesystem::create_directories(dir, ec);
      d.file.reset(std::fopen(d.local.c_str(), "wb"));
      if (!d.file) kLog.warn("view {}: cannot create {}", phonePath, d.local);
      return d.file != nullptr;
    });
  }

  void LinkClient::closePhoto() {
    if (m_photoFile && !m_photoFile->local.empty()) std::remove(m_photoFile->local.c_str());
    m_photoFile.reset();
    ++m_photoOpens;
  }

  void LinkClient::savePhoto(const std::string& phonePath, std::function<void(const std::string&)> done) {
    m_proxy->callMethodAsync("SavePhoto")
        .onInterface(kInterface)
        .withArguments(m_photos.device(), phonePath)
        .uponReplyInvoke([done = std::move(done)](std::optional<bus::Error> error, std::string saved) {
          if (error) kLog.warn("SavePhoto: {}", error->getMessage());
          done(error ? std::string() : saved);
        });
  }

  void LinkClient::takePhoto() {
    const std::string device = presence().phone.id;
    if (device.empty() || m_takingPhoto) return;
    const std::uint64_t serial = ++m_nextTakeId;
    m_takingPhoto = serial;
    changed();
    m_proxy->callMethodAsync("TakePhoto")
        .onInterface(kInterface)
        .withTimeout(std::chrono::minutes(6))
        .withArguments(device)
        .uponReplyInvoke([this, serial, alive = std::weak_ptr<bool>(m_alive)](std::optional<bus::Error> error, std::string) {
          if (alive.expired() || m_takingPhoto != serial) return;
          if (error) kLog.warn("TakePhoto: {}", error->getMessage());
          m_takingPhoto.reset();
          if (!error) loadPhotos();
          changed();
        });
  }

  void LinkClient::cancelTakePhoto() {
    m_takingPhoto.reset();
    changed();
  }

  void LinkClient::clearAll() {
    std::vector<FeedItem> items = m_feed.items();
    for (const FeedItem& item : items) {
      if (item.kind == FeedItem::Kind::Offer && item.transfer == Transfer::Running) continue;
      dismiss(item);
    }
  }

} // namespace linkapp
