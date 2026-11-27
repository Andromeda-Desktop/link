#include "core/deferred_call.h"
#include "mirror_decoder.h"

#include <fstream>
#include <iterator>
#include <memory>
#include <poll.h>
#include <print>
#include <string>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

  void pump(const bool& done) {
    pollfd wake{.fd = DeferredCall::wakeFd(), .events = POLLIN};
    for (int i = 0; i < 100 && !done; ++i) {
      if (::poll(&wake, 1, 100) > 0) DeferredCall::drainWakeFd();
      for (auto& fn : DeferredCall::takePending()) fn();
    }
  }

  // The ended callback may destroy its decoder, as LinkClient does when the phone stops sharing (built with ASan).
  bool endedDestroys() {
    int fds[2];
    if (::socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) return false;
    ::close(fds[1]); // the phone stopped: the worker reads EOF at once
    std::unique_ptr<linkapp::MirrorDecoder> decoder;
    bool ended = false;
    decoder = std::make_unique<linkapp::MirrorDecoder>(
        fds[0], linkapp::MirrorCodec::H264, [](linkapp::MirrorDecoder::Frame) {},
        [&decoder, flag = &ended](std::string) {
          decoder.reset();
          *flag = true;
        });
    pump(ended);
    if (!ended) std::println(stderr, "mirror_decoder_test: the ended callback did not run");
    return ended;
  }

  // A phone's stream (tests/data/make_mirror.py) decodes to pictures of the target size, and the stream ends cleanly.
  bool decodes(linkapp::MirrorCodec codec, const char* file) {
    std::ifstream in(std::string(MIRROR_DATA_DIR) + "/" + file, std::ios::binary);
    const std::vector<char> stream(std::istreambuf_iterator<char>(in), {});
    int fds[2];
    if (stream.empty() || ::socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) return false;
    int frames = 0;
    bool sized = true;
    bool ended = false;
    std::string why;
    linkapp::MirrorDecoder decoder(
        fds[0], codec,
        [&](linkapp::MirrorDecoder::Frame frame) {
          ++frames;
          const bool rgba = frame.rgba.size() == std::size_t{160} * 120 * 4;
          sized = sized && frame.width == 160 && frame.height == 120 && (rgba || frame.dmabuf.fd >= 0);
        },
        [&](std::string reason) {
          why = std::move(reason);
          ended = true;
        });
    decoder.setTarget(160, 120);
    std::thread phone([&] {
      const char* p = stream.data();
      for (std::size_t left = stream.size(); left > 0;) {
        const ssize_t n = ::write(fds[1], p, left);
        if (n <= 0) break;
        p += n;
        left -= static_cast<std::size_t>(n);
      }
      ::close(fds[1]);
    });
    pump(ended);
    phone.join();
    if (!ended || !why.empty() || frames == 0 || !sized) {
      std::println(stderr, "mirror_decoder_test: {}: ended {} ({}), {} frames, sized {}", file, ended, why, frames, sized);
      return false;
    }
    return true;
  }

} // namespace

int main() {
  bool ok = endedDestroys();
  ok = decodes(linkapp::MirrorCodec::H264, "mirror-h264.bin") && ok;
  ok = decodes(linkapp::MirrorCodec::Hevc, "mirror-hevc.bin") && ok;
  return ok ? 0 : 1;
}
