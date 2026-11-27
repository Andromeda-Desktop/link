#pragma once

// Link1 for the window through link-window-ffi (link_ffi.h): an sdbus-c++-shaped proxy over raw D-Bus bodies, so one
// client serves the session bus, the macOS socket and the Windows pipe. Replies and signals run on the main loop.

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <vector>

struct LinkConn;

namespace linkapp::bus {

  struct adopt_fd_t {
    explicit adopt_fd_t() = default;
  };
  inline constexpr adopt_fd_t adopt_fd{};

  class UnixFd {
  public:
    UnixFd() = default;
    UnixFd(int fd, adopt_fd_t) : m_fd(fd) {}
    UnixFd(UnixFd&& other) noexcept : m_fd(std::exchange(other.m_fd, -1)) {}
    UnixFd& operator=(UnixFd&& other) noexcept {
      if (this != &other) {
        reset();
        m_fd = std::exchange(other.m_fd, -1);
      }
      return *this;
    }
    UnixFd(const UnixFd&) = delete;
    UnixFd& operator=(const UnixFd&) = delete;
    ~UnixFd() { reset(); }

    [[nodiscard]] int get() const { return m_fd; }
    [[nodiscard]] bool isValid() const { return m_fd >= 0; }
    int release() { return std::exchange(m_fd, -1); }
    void reset();

  private:
    int m_fd = -1;
  };

  template <typename... T>
  struct Struct : std::tuple<T...> {
    using Tuple = std::tuple<T...>;
    using std::tuple<T...>::tuple;
    Struct() = default;
    template <std::size_t I>
    auto& get() {
      return std::get<I>(*this);
    }
    template <std::size_t I>
    const auto& get() const {
      return std::get<I>(*this);
    }
  };

} // namespace linkapp::bus

// Structured bindings for bus::Struct, as sdbus::Struct has them.
template <typename... T>
struct std::tuple_size<linkapp::bus::Struct<T...>> : std::integral_constant<std::size_t, sizeof...(T)> {};
template <std::size_t I, typename... T>
struct std::tuple_element<I, linkapp::bus::Struct<T...>> : std::tuple_element<I, std::tuple<T...>> {};

namespace linkapp::bus {

  class Error {
  public:
    Error(std::string name, std::string message) : m_name(std::move(name)), m_message(std::move(message)) {}
    [[nodiscard]] const std::string& getName() const { return m_name; }
    [[nodiscard]] const std::string& getMessage() const { return m_message; }

  private:
    std::string m_name;
    std::string m_message;
  };

  class Variant;
  class Writer;
  class Reader;

  template <typename T>
  struct IsVector : std::false_type {};
  template <typename T>
  struct IsVector<std::vector<T>> : std::true_type {};
  template <typename T>
  struct IsMap : std::false_type {};
  template <typename K, typename V>
  struct IsMap<std::map<K, V>> : std::true_type {};
  template <typename T>
  struct IsStruct : std::false_type {};
  template <typename... T>
  struct IsStruct<Struct<T...>> : std::true_type {};

  template <typename T>
  std::string signatureOf();

  template <typename... T>
  std::string signaturesOf() {
    return (std::string{} + ... + signatureOf<T>());
  }

  template <typename T>
  std::string signatureOf() {
    if constexpr (std::is_same_v<T, bool>) return "b";
    else if constexpr (std::is_same_v<T, std::uint8_t>) return "y";
    else if constexpr (std::is_same_v<T, std::int16_t>) return "n";
    else if constexpr (std::is_same_v<T, std::uint16_t>) return "q";
    else if constexpr (std::is_same_v<T, std::int32_t>) return "i";
    else if constexpr (std::is_same_v<T, std::uint32_t>) return "u";
    else if constexpr (std::is_same_v<T, std::int64_t>) return "x";
    else if constexpr (std::is_same_v<T, std::uint64_t>) return "t";
    else if constexpr (std::is_same_v<T, double>) return "d";
    else if constexpr (std::is_same_v<T, std::string>) return "s";
    else if constexpr (std::is_same_v<T, UnixFd>) return "h";
    else if constexpr (std::is_same_v<T, Variant>) return "v";
    else if constexpr (IsVector<T>::value) return "a" + signatureOf<typename T::value_type>();
    else if constexpr (IsMap<T>::value)
      return "a{" + signatureOf<typename T::key_type>() + signatureOf<typename T::mapped_type>() + "}";
    else if constexpr (IsStruct<T>::value)
      return []<typename... U>(const Struct<U...>*) { return "(" + signaturesOf<U...>() + ")"; }(static_cast<const T*>(nullptr));
    else static_assert(sizeof(T) == 0, "no D-Bus type for this C++ type");
  }

  // A value of any type with its signature, kept in wire form; `base` is its start offset modulo 8 in that form.
  class Variant {
  public:
    Variant() = default;
    template <typename T>
      requires(!std::is_same_v<std::decay_t<T>, Variant>)
    explicit Variant(const T& value);

    template <typename T>
    [[nodiscard]] bool containsValueOfType() const {
      return m_signature == signatureOf<T>();
    }
    template <typename T>
    [[nodiscard]] T get() const;

  private:
    friend class Writer;
    friend class Reader;
    std::string m_signature;
    std::vector<std::uint8_t> m_bytes;
    std::size_t m_base = 0;
  };

  class Writer {
  public:
    void align(std::size_t n) {
      while (m_bytes.size() % n != 0) m_bytes.push_back(0);
    }

    template <typename T>
    void put(const T& value) {
      if constexpr (std::is_same_v<T, bool>) putRaw<std::uint32_t>(value ? 1U : 0U);
      else if constexpr (std::is_arithmetic_v<T>) putRaw<T>(value);
      else if constexpr (std::is_same_v<T, std::string>) putString(value);
      else if constexpr (std::is_same_v<T, UnixFd>) {
        putRaw<std::uint32_t>(static_cast<std::uint32_t>(m_fds.size()));
        m_fds.push_back(value.get());
      } else if constexpr (std::is_same_v<T, Variant>) putVariant(value);
      else if constexpr (IsVector<T>::value) {
        const std::size_t start = beginArray(alignment(signatureOf<typename T::value_type>()));
        if constexpr (std::is_same_v<typename T::value_type, std::uint8_t>) {
          m_bytes.insert(m_bytes.end(), value.begin(), value.end());
        } else {
          for (const auto& item : value) put(item);
        }
        endArray(start);
      } else if constexpr (IsMap<T>::value) {
        const std::size_t start = beginArray(8);
        for (const auto& [key, item] : value) {
          align(8);
          put(key);
          put(item);
        }
        endArray(start);
      } else if constexpr (IsStruct<T>::value) {
        align(8);
        std::apply([this](const auto&... field) { (put(field), ...); }, static_cast<const typename T::Tuple&>(value));
      } else static_assert(sizeof(T) == 0, "no D-Bus type for this C++ type");
    }

    // Copies one complete value of `signature` from `in`, re-aligning it for this position.
    void copy(Reader& in, std::string_view signature);

    [[nodiscard]] const std::vector<std::uint8_t>& bytes() const { return m_bytes; }
    [[nodiscard]] std::vector<std::uint8_t>& bytes() { return m_bytes; }
    [[nodiscard]] const std::vector<int>& fds() const { return m_fds; }

    static std::size_t alignment(std::string_view signature);

  private:
    template <typename T>
    void putRaw(T value) {
      align(sizeof(T));
      const std::size_t at = m_bytes.size();
      m_bytes.resize(at + sizeof(T));
      std::memcpy(m_bytes.data() + at, &value, sizeof(T)); // little endian on every target
    }
    friend class Reader;
    void putString(std::string_view text);
    void putSignature(std::string_view signature);
    void putVariant(const Variant& value);
    std::size_t beginArray(std::size_t elementAlign);
    void endArray(std::size_t start);

    std::vector<std::uint8_t> m_bytes;
    std::vector<int> m_fds;           // borrowed: the caller keeps them open until the call is sent
    std::vector<std::size_t> m_arrays; // where the open arrays keep their length
  };

  class Reader {
  public:
    Reader(const std::uint8_t* data, std::size_t size, std::size_t base = 0, std::vector<UnixFd>* fds = nullptr)
        : m_data(data), m_size(size), m_base(base), m_fds(fds) {}

    [[nodiscard]] bool ok() const { return m_ok; }
    [[nodiscard]] bool atEnd() const { return m_pos == m_size; }

    template <typename T>
    void get(T& out) {
      if constexpr (std::is_same_v<T, bool>) out = getRaw<std::uint32_t>() != 0;
      else if constexpr (std::is_arithmetic_v<T>) out = getRaw<T>();
      else if constexpr (std::is_same_v<T, std::string>) out = getString();
      else if constexpr (std::is_same_v<T, UnixFd>) out = getFd();
      else if constexpr (std::is_same_v<T, Variant>) getVariant(out);
      else if constexpr (IsVector<T>::value) {
        out.clear();
        const std::size_t end = beginArray(Writer::alignment(signatureOf<typename T::value_type>()));
        if constexpr (std::is_same_v<typename T::value_type, std::uint8_t>) {
          if (m_ok) out.assign(m_data + m_pos, m_data + end);
          m_pos = end;
        } else {
          while (m_ok && m_pos < end) get(out.emplace_back());
        }
      } else if constexpr (IsMap<T>::value) {
        out.clear();
        const std::size_t end = beginArray(8);
        while (m_ok && m_pos < end) {
          align(8);
          typename T::key_type key{};
          typename T::mapped_type item{};
          get(key);
          get(item);
          out.insert_or_assign(std::move(key), std::move(item));
        }
      } else if constexpr (IsStruct<T>::value) {
        align(8);
        std::apply([this](auto&... field) { (get(field), ...); }, static_cast<typename T::Tuple&>(out));
      } else static_assert(sizeof(T) == 0, "no D-Bus type for this C++ type");
    }

    void align(std::size_t n);
    // Reads one complete value of `signature`; `out` (when set) gets a copy.
    void copy(std::string_view signature, Writer* out);
    [[nodiscard]] std::string getString();
    [[nodiscard]] std::string getSignature();

  private:
    template <typename T>
    T getRaw() {
      align(sizeof(T));
      if (!need(sizeof(T))) return T{};
      T value;
      std::memcpy(&value, m_data + m_pos, sizeof(T));
      m_pos += sizeof(T);
      return value;
    }
    bool need(std::size_t n);
    UnixFd getFd();
    void getVariant(Variant& out);
    std::size_t beginArray(std::size_t elementAlign);

    const std::uint8_t* m_data;
    std::size_t m_size;
    std::size_t m_base;
    std::vector<UnixFd>* m_fds;
    std::size_t m_pos = 0;
    bool m_ok = true;
  };

  template <typename T>
    requires(!std::is_same_v<std::decay_t<T>, Variant>)
  Variant::Variant(const T& value) : m_signature(signatureOf<T>()) {
    Writer writer;
    writer.put(value);
    m_bytes = std::move(writer.bytes());
  }

  template <typename T>
  T Variant::get() const {
    T out{};
    if (!containsValueOfType<T>()) return out;
    Reader reader(m_bytes.data(), m_bytes.size(), m_base);
    reader.get(out);
    return out;
  }

  // One reply or signal body as the main loop receives it.
  struct Message {
    std::optional<Error> error;
    std::string signature;
    std::vector<std::uint8_t> body;
    std::vector<UnixFd> fds;
  };

  template <typename F>
  struct Callable : Callable<decltype(&F::operator())> {};
  template <typename C, typename R, typename... A>
  struct Callable<R (C::*)(A...) const> {
    using Args = std::tuple<std::decay_t<A>...>;
  };
  template <typename C, typename R, typename... A>
  struct Callable<R (C::*)(A...)> {
    using Args = std::tuple<std::decay_t<A>...>;
  };

  // Decodes the message into Args; nullopt when its signature is not theirs.
  template <typename... A>
  std::optional<std::tuple<A...>> decode(Message& message) {
    if (message.signature != signaturesOf<A...>()) return std::nullopt;
    std::tuple<A...> values;
    Reader reader(message.body.data(), message.body.size(), 0, &message.fds);
    std::apply([&reader](auto&... value) { (reader.get(value), ...); }, values);
    if (!reader.ok()) return std::nullopt;
    return values;
  }

  class Link;

  class MethodCall {
  public:
    MethodCall(Link& link, std::string member) : m_link(link), m_member(std::move(member)) {}
    MethodCall& onInterface(std::string_view iface) {
      m_iface = iface;
      return *this;
    }
    template <typename... A>
    MethodCall& withArguments(const A&... args) {
      m_signature = signaturesOf<A...>();
      (m_writer.put(args), ...);
      return *this;
    }
    template <typename Rep, typename Period>
    MethodCall& withTimeout(std::chrono::duration<Rep, Period> timeout) {
      m_timeoutMs = static_cast<std::uint32_t>(std::chrono::duration_cast<std::chrono::milliseconds>(timeout).count());
      return *this;
    }
    // The handler runs only on failure; a success posts nothing (input events at pointer rate).
    MethodCall& onlyOnError() {
      m_onlyOnError = true;
      return *this;
    }
    // F takes std::optional<Error> and then the reply's values.
    template <typename F>
    void uponReplyInvoke(F handler);

  private:
    void send(std::function<void(Message&)> reply);

    Link& m_link;
    std::string m_member;
    std::string m_iface;
    std::string m_signature;
    Writer m_writer;
    std::uint32_t m_timeoutMs = 0;
    bool m_onlyOnError = false;
  };

  class SignalSubscription {
  public:
    SignalSubscription(Link& link, std::string member) : m_link(link), m_member(std::move(member)) {}
    SignalSubscription& onInterface(std::string_view iface) {
      m_iface = iface;
      return *this;
    }
    // F takes the signal's values.
    template <typename F>
    void call(F handler);

  private:
    Link& m_link;
    std::string m_member;
    std::string m_iface;
  };

  class Link {
  public:
    // `presence` runs with true when linkd serves Link1 and false when it stops.
    explicit Link(std::function<void(bool)> presence);
    ~Link();
    Link(const Link&) = delete;
    Link& operator=(const Link&) = delete;

    MethodCall callMethodAsync(std::string member) { return {*this, std::move(member)}; }
    SignalSubscription uponSignal(std::string member) { return {*this, std::move(member)}; }

    // The connection thread's handoff to the main loop (link_bus.cpp).
    struct Event;
    struct Target;
    void deliver(Event& event);

  private:
    friend class MethodCall;
    friend class SignalSubscription;
    void deliverLater(std::uint64_t token, Message message);
    std::uint64_t errorOnlyToken(const std::string& member, std::function<void(Message&)> handler);

    std::unique_ptr<Target> m_target;
    LinkConn* m_conn = nullptr;
    std::shared_ptr<bool> m_alive = std::make_shared<bool>(true);
    std::function<void(bool)> m_presence;
    std::uint64_t m_nextToken = 1;
    std::unordered_map<std::uint64_t, std::function<void(Message&)>> m_pending;
    // Error-only calls post nothing on success, so their handler stays, one per member: token kErrorOnly | index.
    std::vector<std::pair<std::string, std::function<void(Message&)>>> m_errorOnly;
    std::unordered_map<std::string, std::vector<std::function<void(Message&)>>> m_signals;
  };

  template <typename F>
  void MethodCall::uponReplyInvoke(F handler) {
    using Args = typename Callable<F>::Args;
    send([handler = std::move(handler), member = m_member](Message& message) mutable {
      [&]<typename E, typename... R>(std::tuple<E, R...>*) {
        static_assert(std::is_same_v<E, std::optional<Error>>, "a reply handler takes std::optional<Error> first");
        if (message.error) {
          handler(std::move(message.error), R{}...);
          return;
        }
        auto values = decode<R...>(message);
        if (!values) {
          handler(Error("org.freedesktop.DBus.Error.InvalidSignature",
                      member + " replied (" + message.signature + "), expected (" + signaturesOf<R...>() + ")"),
              R{}...);
          return;
        }
        std::apply([&](auto&... value) { handler(std::nullopt, std::move(value)...); }, *values);
      }(static_cast<Args*>(nullptr));
    });
  }

  void logSignalMismatch(const std::string& member, const std::string& got, const std::string& expected);

  template <typename F>
  void SignalSubscription::call(F handler) {
    using Args = typename Callable<F>::Args;
    m_link.m_signals[m_iface + '\n' + m_member].push_back([handler = std::move(handler), member = m_member](Message& message) mutable {
      [&]<typename... A>(std::tuple<A...>*) {
        auto values = decode<A...>(message);
        if (!values) {
          logSignalMismatch(member, message.signature, signaturesOf<A...>());
          return;
        }
        std::apply([&](auto&... value) { handler(std::move(value)...); }, *values);
      }(static_cast<Args*>(nullptr));
    });
  }

} // namespace linkapp::bus
