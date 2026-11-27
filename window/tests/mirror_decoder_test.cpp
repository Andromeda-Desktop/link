#include "core/deferred_call.h"
#include "mirror_decoder.h"

#include <memory>
#include <poll.h>
#include <print>
#include <sys/socket.h>
#include <unistd.h>

// The ended callback may destroy its decoder, as LinkClient does when the phone stops sharing (built with ASan).
int main() {
  int fds[2];
  if (::socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0) return 1;
  ::close(fds[1]); // the phone stopped: the worker reads EOF at once
  std::unique_ptr<linkapp::MirrorDecoder> decoder;
  bool ended = false;
  decoder = std::make_unique<linkapp::MirrorDecoder>(
      fds[0], linkapp::MirrorCodec::H264, [](linkapp::MirrorDecoder::Frame) {},
      [&decoder, flag = &ended](std::string) {
        decoder.reset();
        *flag = true;
      });
  pollfd wake{.fd = DeferredCall::wakeFd(), .events = POLLIN};
  for (int i = 0; i < 50 && !ended; ++i) {
    if (::poll(&wake, 1, 100) > 0) DeferredCall::drainWakeFd();
    for (auto& fn : DeferredCall::takePending()) fn();
  }
  if (!ended) {
    std::println(stderr, "mirror_decoder_test: the ended callback did not run");
    return 1;
  }
  return 0;
}
