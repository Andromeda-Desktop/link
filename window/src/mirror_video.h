#pragma once

#include "mirror_decoder.h"

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>

namespace linkapp {

  // Where a platform video decoder hands its pictures: MirrorDecoder's worker side.
  class VideoSink {
  public:
    virtual ~VideoSink() = default;
    // Counts one decoded picture; false while the UI still holds the last one, so this one is dropped unconverted.
    virtual bool wantsFrame() = 0;
    virtual void post(MirrorDecoder::Frame frame) = 0;
    // The size to scale to, in pixels; the picture's own size when the UI set none.
    virtual void target(int pictureW, int pictureH, int& w, int& h) const = 0;
    [[nodiscard]] virtual bool rgbaOnly() const = 0;
    virtual void refuseDmabuf() = 0;
  };

  // One platform decoder per stream (mirror_decoder_av.cpp, mirror_decoder_vt.mm), on the worker thread.
  class VideoDecoder {
  public:
    // Extra zeroed bytes after each unit that `decode` may read past its end.
    static constexpr std::size_t kPadding = 64;
    virtual ~VideoDecoder() = default;
    // Decodes one Annex B unit; a non-empty result ends the stream with that reason.
    virtual std::string decode(const std::uint8_t* unit, std::size_t length, VideoSink& sink) = 0;
  };

  // Null with `why` set when this desktop cannot decode `codec`.
  std::unique_ptr<VideoDecoder> openVideoDecoder(MirrorCodec codec, std::string& why);

} // namespace linkapp
