#include "link_bus.h"

#include "core/deferred_call.h"
#include "core/log.h"

#include <link_ffi.h>
#include <unistd.h>

namespace linkapp::bus {

  namespace {

    constexpr Logger kLog("link");
    constexpr std::uint64_t kErrorOnly = std::uint64_t{1} << 63;

    // The end of the complete type that starts at `at`.
    std::size_t typeEnd(std::string_view signature, std::size_t at) {
      if (at >= signature.size()) return signature.size();
      switch (signature[at]) {
      case 'a':
        return typeEnd(signature, at + 1);
      case '(':
      case '{': {
        std::size_t next = at + 1;
        while (next < signature.size() && signature[next] != ')' && signature[next] != '}') next = typeEnd(signature, next);
        return next + 1;
      }
      default:
        return at + 1;
      }
    }

  } // namespace

  void UnixFd::reset() {
    if (m_fd >= 0) ::close(m_fd);
    m_fd = -1;
  }

  std::size_t Writer::alignment(std::string_view signature) {
    switch (signature.empty() ? 'y' : signature.front()) {
    case 'n':
    case 'q':
      return 2;
    case 'b':
    case 'i':
    case 'u':
    case 'h':
    case 's':
    case 'o':
    case 'a':
      return 4;
    case 'x':
    case 't':
    case 'd':
    case '(':
    case '{':
      return 8;
    default:
      return 1;
    }
  }

  void Writer::putString(std::string_view text) {
    putRaw<std::uint32_t>(static_cast<std::uint32_t>(text.size()));
    m_bytes.insert(m_bytes.end(), text.begin(), text.end());
    m_bytes.push_back(0);
  }

  void Writer::putSignature(std::string_view signature) {
    m_bytes.push_back(static_cast<std::uint8_t>(signature.size()));
    m_bytes.insert(m_bytes.end(), signature.begin(), signature.end());
    m_bytes.push_back(0);
  }

  void Writer::putVariant(const Variant& value) {
    putSignature(value.m_signature);
    if (m_bytes.size() % 8 == value.m_base % 8) {
      m_bytes.insert(m_bytes.end(), value.m_bytes.begin(), value.m_bytes.end());
      return;
    }
    Reader in(value.m_bytes.data(), value.m_bytes.size(), value.m_base);
    copy(in, value.m_signature);
  }

  std::size_t Writer::beginArray(std::size_t elementAlign) {
    putRaw<std::uint32_t>(0);
    const std::size_t lengthAt = m_bytes.size() - 4;
    align(elementAlign);
    m_arrays.push_back(lengthAt);
    return m_bytes.size();
  }

  void Writer::endArray(std::size_t start) {
    const std::size_t lengthAt = m_arrays.back();
    m_arrays.pop_back();
    const auto length = static_cast<std::uint32_t>(m_bytes.size() - start);
    std::memcpy(m_bytes.data() + lengthAt, &length, sizeof(length));
  }

  void Writer::copy(Reader& in, std::string_view signature) { in.copy(signature, this); }

  bool Reader::need(std::size_t n) {
    if (m_ok && m_size - m_pos >= n) return true;
    m_ok = false;
    m_pos = m_size;
    return false;
  }

  void Reader::align(std::size_t n) {
    const std::size_t pad = (n - (m_base + m_pos) % n) % n;
    if (need(pad)) m_pos += pad;
  }

  std::string Reader::getString() {
    const auto length = getRaw<std::uint32_t>();
    if (!need(std::size_t{length} + 1)) return {};
    std::string text(reinterpret_cast<const char*>(m_data + m_pos), length);
    m_pos += std::size_t{length} + 1;
    return text;
  }

  std::string Reader::getSignature() {
    if (!need(1)) return {};
    const std::size_t length = m_data[m_pos++];
    if (!need(length + 1)) return {};
    std::string text(reinterpret_cast<const char*>(m_data + m_pos), length);
    m_pos += length + 1;
    return text;
  }

  UnixFd Reader::getFd() {
    const auto index = getRaw<std::uint32_t>();
    if (m_fds == nullptr || index >= m_fds->size()) {
      m_ok = m_ok && m_fds == nullptr;
      return {};
    }
    return std::move((*m_fds)[index]);
  }

  void Reader::getVariant(Variant& out) {
    out.m_signature = getSignature();
    if (!m_ok || typeEnd(out.m_signature, 0) != out.m_signature.size()) {
      m_ok = false;
      return;
    }
    align(Writer::alignment(out.m_signature));
    const std::size_t start = m_pos;
    copy(out.m_signature, nullptr);
    if (!m_ok) return;
    out.m_base = (m_base + start) % 8;
    out.m_bytes.assign(m_data + start, m_data + m_pos);
  }

  std::size_t Reader::beginArray(std::size_t elementAlign) {
    const auto length = getRaw<std::uint32_t>();
    align(elementAlign);
    if (!need(length)) return m_pos;
    return m_pos + length;
  }

  void Reader::copy(std::string_view signature, Writer* out) {
    if (signature.empty() || !m_ok) return;
    switch (signature.front()) {
    case 'y': {
      const auto value = getRaw<std::uint8_t>();
      if (out != nullptr) out->put(value);
      return;
    }
    case 'n':
    case 'q': {
      const auto value = getRaw<std::uint16_t>();
      if (out != nullptr) out->put(value);
      return;
    }
    case 'b':
    case 'i':
    case 'u':
    case 'h': {
      const auto value = getRaw<std::uint32_t>();
      if (out != nullptr) out->put(value);
      return;
    }
    case 'x':
    case 't':
    case 'd': {
      const auto value = getRaw<std::uint64_t>();
      if (out != nullptr) out->put(value);
      return;
    }
    case 's':
    case 'o': {
      auto value = getString();
      if (out != nullptr) out->put(value);
      return;
    }
    case 'g': {
      auto value = getSignature();
      if (out != nullptr) out->putSignature(value);
      return;
    }
    case 'v': {
      Variant value;
      getVariant(value);
      if (out != nullptr) out->putVariant(value);
      return;
    }
    case 'a': {
      const std::string_view element = signature.substr(1, typeEnd(signature, 1) - 1);
      const std::size_t elementAlign = Writer::alignment(element);
      const std::size_t end = beginArray(elementAlign);
      const std::size_t start = out != nullptr ? out->beginArray(elementAlign) : 0;
      while (m_ok && m_pos < end) copy(element, out);
      if (out != nullptr) out->endArray(start);
      return;
    }
    case '(':
    case '{': {
      align(8);
      if (out != nullptr) out->align(8);
      std::size_t at = 1;
      const std::size_t end = typeEnd(signature, 0) - 1;
      while (m_ok && at < end) {
        const std::size_t next = typeEnd(signature, at);
        copy(signature.substr(at, next - at), out);
        at = next;
      }
      return;
    }
    default:
      m_ok = false;
    }
  }

  void logSignalMismatch(const std::string& member, const std::string& got, const std::string& expected) {
    kLog.warn("signal {} carries ({}), expected ({})", member, got, expected);
  }

  struct Link::Event {
    LinkKind kind;
    std::uint64_t token;
    std::string key; // iface '\n' member, for a signal
    Message message;
  };

  namespace {

    std::string text(const char* value) { return value == nullptr ? std::string() : std::string(value); }

  } // namespace

  struct Link::Target {
    Link* link;
    std::weak_ptr<bool> alive;
  };

  namespace {

    void post(void* ctx, const LinkEvent* raw) {
      const auto* target = static_cast<const Link::Target*>(ctx);
      auto event = std::make_shared<Link::Event>();
      event->kind = raw->kind;
      event->token = raw->token;
      for (std::size_t i = 0; i < raw->nfds; ++i) event->message.fds.emplace_back(raw->fds[i], adopt_fd);
      if (raw->kind == LINK_ERROR) event->message.error.emplace(text(raw->error_name), text(raw->error_message));
      if (raw->kind == LINK_SIGNAL) event->key = text(raw->iface) + '\n' + text(raw->member);
      event->message.signature = text(raw->signature);
      if (raw->len > 0) event->message.body.assign(raw->body, raw->body + raw->len);
      DeferredCall::callLaterIfAlive(target->alive, [link = target->link, event] { link->deliver(*event); });
    }

  } // namespace

  Link::Link(std::function<void(bool)> presence) : m_presence(std::move(presence)) {
    m_target = std::make_unique<Target>(Target{this, m_alive});
    m_conn = link_open(post, m_target.get());
    if (m_conn == nullptr) kLog.warn("no connection thread for Link1");
  }

  Link::~Link() {
    // No post runs after link_close returns; posts already queued see the expired token.
    if (m_conn != nullptr) link_close(m_conn);
    m_alive.reset();
  }

  void Link::deliver(Event& event) {
    switch (event.kind) {
    case LINK_UP:
    case LINK_DOWN:
      if (m_presence) m_presence(event.kind == LINK_UP);
      return;
    case LINK_REPLY:
    case LINK_ERROR: {
      if ((event.token & kErrorOnly) != 0) {
        const std::uint64_t index = event.token & ~kErrorOnly;
        if (index < m_errorOnly.size()) m_errorOnly[index].second(event.message);
        return;
      }
      const auto it = m_pending.find(event.token);
      if (it == m_pending.end()) return;
      auto reply = std::move(it->second);
      m_pending.erase(it);
      reply(event.message);
      return;
    }
    case LINK_SIGNAL: {
      const auto it = m_signals.find(event.key);
      if (it == m_signals.end()) return;
      for (auto& handler : it->second) handler(event.message);
      return;
    }
    }
  }

  void Link::deliverLater(std::uint64_t token, Message message) {
    auto event = std::make_shared<Event>(Event{.kind = message.error ? LINK_ERROR : LINK_REPLY, .token = token, .key = {},
        .message = std::move(message)});
    DeferredCall::callLaterIfAlive(m_alive, [this, event] { deliver(*event); });
  }

  std::uint64_t Link::errorOnlyToken(const std::string& member, std::function<void(Message&)> handler) {
    std::size_t index = 0;
    while (index < m_errorOnly.size() && m_errorOnly[index].first != member) ++index;
    if (index == m_errorOnly.size()) m_errorOnly.emplace_back(member, nullptr);
    m_errorOnly[index].second = std::move(handler);
    return kErrorOnly | index;
  }

  void MethodCall::send(std::function<void(Message&)> reply) {
    std::uint64_t token = 0;
    if (m_onlyOnError) {
      token = m_link.errorOnlyToken(m_member, std::move(reply));
    } else {
      token = m_link.m_nextToken++;
      m_link.m_pending.emplace(token, std::move(reply));
    }
    if (m_link.m_conn == nullptr) {
      m_link.deliverLater(token, Message{.error = Error("org.freedesktop.DBus.Error.Disconnected", "no connection thread for Link1"),
          .signature = {}, .body = {}, .fds = {}});
      return;
    }
    const auto& bytes = m_writer.bytes();
    const auto& fds = m_writer.fds();
    link_call(m_link.m_conn, token, m_iface.c_str(), m_member.c_str(), m_signature.c_str(), bytes.data(), bytes.size(),
        fds.data(), fds.size(), m_timeoutMs, m_onlyOnError ? LINK_CALL_ERROR_ONLY : 0U);
  }

} // namespace linkapp::bus
