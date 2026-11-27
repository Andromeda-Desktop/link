#include "mirror_decoder.h"
#include "mirror_video.h"

#include "core/deferred_call.h"
#include "core/log.h"
#include "i18n/i18n.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <format>
#include <optional>
#include <sys/socket.h>
#include <unistd.h>
#include <utility>

namespace linkapp {

  namespace {

    constexpr std::size_t kHeader = 13;          // u32 length, u64 time, u8 flags
    constexpr std::uint32_t kMaxUnit = 4U << 20; // link-mirror's MAX_UNIT
    constexpr std::uint8_t kAudio = 4;
    constexpr std::uint8_t kEnd = 8;

    // Reads exactly `size` bytes; false at the end of the stream or on an error.
    bool readAll(int fd, std::uint8_t* data, std::size_t size) {
      while (size > 0) {
        const ssize_t n = ::read(fd, data, size);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        data += n;
        size -= static_cast<std::size_t>(n);
      }
      return true;
    }

  } // namespace

  std::optional<MirrorCodec> mirrorCodecFrom(std::string_view name) {
    if (name == "h264") return MirrorCodec::H264;
    if (name == "hevc") return MirrorCodec::Hevc;
    if (name == "av1") return MirrorCodec::Av1;
    return std::nullopt;
  }

  MirrorDecoder::MirrorDecoder(int fd, MirrorCodec codec, std::function<void(Frame)> frame, std::function<void(std::string)> ended)
      : m_fd(fd), m_codec(codec), m_frame(std::move(frame)), m_ended(std::move(ended)), m_thread([this] { run(); }) {}

  MirrorDecoder::~MirrorDecoder() {
    ::shutdown(m_fd, SHUT_RDWR); // wakes the worker's read
    m_thread.join();
    ::close(m_fd);
  }

  void MirrorDecoder::setTarget(int width, int height) {
    m_targetW = width;
    m_targetH = height;
  }

  void MirrorDecoder::run() {
    const auto end = [this](std::string why) {
      DeferredCall::callLater([this, why = std::move(why), alive = std::weak_ptr<bool>(m_alive)]() mutable {
        // ended may destroy this decoder: call a moved-out copy so its closure outlives the call.
        if (!alive.expired()) std::exchange(m_ended, {})(std::move(why));
      });
    };
    std::string why;
    const std::unique_ptr<VideoDecoder> decoder = openVideoDecoder(m_codec, why);
    if (decoder == nullptr) {
      end(std::move(why));
      return;
    }
    // The worker's side of the decoder: frame pacing and the hand-off to the UI thread.
    class Sink final : public VideoSink {
    public:
      explicit Sink(MirrorDecoder& owner) : m_owner(owner) {}
      bool wantsFrame() override {
        ++m_frames;
        const auto now = std::chrono::steady_clock::now();
        if (now - m_second >= std::chrono::seconds(1)) {
          m_owner.m_fps = m_frames;
          m_frames = 0;
          m_second = now;
        }
        return !m_owner.m_posted.load(); // the UI has not taken the last frame: drop this one
      }
      void post(Frame frame) override {
        m_owner.m_posted = true;
        DeferredCall::callLater([owner = &m_owner, frame = std::move(frame), alive = std::weak_ptr<bool>(m_owner.m_alive)]() mutable {
          if (alive.expired()) return;
          owner->m_posted = false;
          owner->m_frame(std::move(frame));
        });
      }
      void target(int pictureW, int pictureH, int& w, int& h) const override {
        w = m_owner.m_targetW.load();
        h = m_owner.m_targetH.load();
        if (w <= 0 || h <= 0) {
          w = pictureW;
          h = pictureH;
        }
      }
      [[nodiscard]] bool rgbaOnly() const override { return m_owner.m_rgbaOnly.load(); }
      void refuseDmabuf() override { m_owner.m_rgbaOnly = true; }

    private:
      MirrorDecoder& m_owner;
      int m_frames = 0;
      std::chrono::steady_clock::time_point m_second = std::chrono::steady_clock::now();
    } sink(*this);
    std::vector<std::uint8_t> unit;
    while (why.empty()) {
      std::array<std::uint8_t, kHeader> header{};
      if (!readAll(m_fd, header.data(), header.size())) break;
      const std::uint32_t length = (std::uint32_t{header[0]} << 24) | (std::uint32_t{header[1]} << 16) |
                                   (std::uint32_t{header[2]} << 8) | header[3];
      const std::uint8_t flags = header[12];
      if (length > kMaxUnit) {
        why = i18n::tr("link.client.broken-video");
        break;
      }
      unit.assign(length + VideoDecoder::kPadding, 0);
      if (!readAll(m_fd, unit.data(), length)) break;
      if ((flags & kEnd) != 0) break;
      if ((flags & kAudio) != 0) continue;
      why = decoder->decode(unit.data(), length, sink);
    }
    end(std::move(why));
  }

} // namespace linkapp
