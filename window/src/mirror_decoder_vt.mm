#include "mirror_video.h"

#include "core/log.h"

#include <CoreFoundation/CoreFoundation.h>
#include <CoreMedia/CoreMedia.h>
#include <CoreVideo/CoreVideo.h>
#include <VideoToolbox/VideoToolbox.h>

#include <algorithm>
#include <array>
#include <format>
#include <vector>

namespace linkapp {

  namespace {

    constexpr Logger kLog("link-mirror");

    template <class T> struct CfRef {
      T ref = nullptr;
      CfRef() = default;
      CfRef(const CfRef&) = delete;
      CfRef& operator=(const CfRef&) = delete;
      ~CfRef() { reset(); }
      void reset(T next = nullptr) {
        if (ref != nullptr) CFRelease(ref);
        ref = next;
      }
    };

    struct Nal {
      const std::uint8_t* data;
      std::size_t size;
    };

    // The NAL units of one Annex B unit, start codes removed.
    std::vector<Nal> splitAnnexB(const std::uint8_t* p, std::size_t n) {
      std::vector<Nal> out;
      std::size_t start = SIZE_MAX;
      std::size_t i = 0;
      while (i + 3 <= n) {
        if (p[i] == 0 && p[i + 1] == 0 && p[i + 2] == 1) {
          if (start != SIZE_MAX) {
            std::size_t end = i;
            while (end > start && p[end - 1] == 0) --end; // a 4-byte start code's leading zero
            out.push_back({p + start, end - start});
          }
          i += 3;
          start = i;
        } else {
          ++i;
        }
      }
      if (start != SIZE_MAX && start < n) out.push_back({p + start, n - start});
      return out;
    }

    // VideoToolbox H.264 or HEVC: BGRA pictures from the hardware decoder, scaled by VTPixelTransferSession, then
    // swizzled to the RGBA bytes MirrorDecoder::Frame carries.
    class VtDecoder final : public VideoDecoder {
    public:
      explicit VtDecoder(MirrorCodec codec) : m_hevc(codec == MirrorCodec::Hevc) {}
      ~VtDecoder() override { closeSession(); }
      VtDecoder(const VtDecoder&) = delete;
      VtDecoder& operator=(const VtDecoder&) = delete;

      std::string decode(const std::uint8_t* unit, std::size_t length, VideoSink& sink) override {
        std::vector<std::uint8_t> sample;
        bool parametersChanged = false;
        for (const Nal& nal : splitAnnexB(unit, length)) {
          if (nal.size == 0) continue;
          if (const int slot = parameterSlot(nal); slot >= 0) {
            auto& stored = m_parameters[static_cast<std::size_t>(slot)];
            if (stored.size() != nal.size || !std::equal(stored.begin(), stored.end(), nal.data)) {
              stored.assign(nal.data, nal.data + nal.size);
              parametersChanged = true;
            }
            continue;
          }
          const auto size = static_cast<std::uint32_t>(nal.size);
          sample.insert(sample.end(), {static_cast<std::uint8_t>(size >> 24), static_cast<std::uint8_t>(size >> 16),
                                       static_cast<std::uint8_t>(size >> 8), static_cast<std::uint8_t>(size)});
          sample.insert(sample.end(), nal.data, nal.data + nal.size);
        }
        if (parametersChanged && !openSession()) return {};
        if (sample.empty()) return {};
        if (decodeSample(sample) == kVTInvalidSessionErr && openSession()) decodeSample(sample); // after sleep
        for (CVPixelBufferRef picture : m_pictures) {
          if (sink.wantsFrame()) post(picture, sink);
          CVPixelBufferRelease(picture);
        }
        m_pictures.clear();
        return {};
      }

    private:
      // H.264: SPS, PPS; HEVC: VPS, SPS, PPS. -1 for a slice or any other unit.
      [[nodiscard]] int parameterSlot(const Nal& nal) const {
        if (m_hevc) {
          const int type = (nal.data[0] >> 1) & 0x3f;
          return type >= 32 && type <= 34 ? type - 32 : -1;
        }
        const int type = nal.data[0] & 0x1f;
        return type == 7 ? 0 : type == 8 ? 1 : -1;
      }

      bool openSession() {
        closeSession();
        const std::size_t count = m_hevc ? 3 : 2;
        std::array<const std::uint8_t*, 3> sets{};
        std::array<std::size_t, 3> sizes{};
        for (std::size_t i = 0; i < count; ++i) {
          if (m_parameters[i].empty()) return false; // the next key frame brings them
          sets[i] = m_parameters[i].data();
          sizes[i] = m_parameters[i].size();
        }
        CMVideoFormatDescriptionRef format = nullptr;
        const OSStatus made =
            m_hevc ? CMVideoFormatDescriptionCreateFromHEVCParameterSets(kCFAllocatorDefault, count, sets.data(), sizes.data(),
                                                                         4, nullptr, &format)
                   : CMVideoFormatDescriptionCreateFromH264ParameterSets(kCFAllocatorDefault, count, sets.data(), sizes.data(),
                                                                         4, &format);
        if (made != noErr) {
          kLog.info("mirror: VideoToolbox refused the parameter sets ({})", made);
          return false;
        }
        m_format.reset(format);
        const std::int32_t bgra = kCVPixelFormatType_32BGRA;
        CfRef<CFNumberRef> pixelFormat;
        pixelFormat.reset(CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bgra));
        CfRef<CFDictionaryRef> ioSurface;
        ioSurface.reset(CFDictionaryCreate(kCFAllocatorDefault, nullptr, nullptr, 0, &kCFTypeDictionaryKeyCallBacks,
                                           &kCFTypeDictionaryValueCallBacks));
        std::array<const void*, 2> keys{kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferIOSurfacePropertiesKey};
        std::array<const void*, 2> values{pixelFormat.ref, ioSurface.ref};
        CfRef<CFDictionaryRef> attributes;
        attributes.reset(CFDictionaryCreate(kCFAllocatorDefault, keys.data(), values.data(), 2,
                                            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
        const VTDecompressionOutputCallbackRecord callback{.decompressionOutputCallback = &VtDecoder::output,
                                                           .decompressionOutputRefCon = this};
        VTDecompressionSessionRef session = nullptr;
        const OSStatus opened =
            VTDecompressionSessionCreate(kCFAllocatorDefault, m_format.ref, nullptr, attributes.ref, &callback, &session);
        if (opened != noErr) {
          kLog.info("mirror: VideoToolbox did not start a {} session ({})", m_hevc ? "HEVC" : "H.264", opened);
          return false;
        }
        m_session.ref = session;
        kLog.info("mirror: {} via VideoToolbox", m_hevc ? "HEVC" : "H.264");
        return true;
      }

      void closeSession() {
        if (m_session.ref != nullptr) VTDecompressionSessionInvalidate(m_session.ref);
        m_session.reset();
      }

      // Without the asynchronous flag, the output callback runs before this returns.
      OSStatus decodeSample(const std::vector<std::uint8_t>& sample) {
        if (m_session.ref == nullptr) return noErr;
        CMBlockBufferRef block = nullptr;
        if (CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, nullptr, sample.size(), kCFAllocatorDefault, nullptr, 0,
                                               sample.size(), kCMBlockBufferAssureMemoryNowFlag, &block) != noErr) {
          return noErr;
        }
        CfRef<CMBlockBufferRef> blockRef;
        blockRef.reset(block);
        if (CMBlockBufferReplaceDataBytes(sample.data(), block, 0, sample.size()) != noErr) return noErr;
        CMSampleBufferRef buffer = nullptr;
        const std::size_t size = sample.size();
        if (CMSampleBufferCreateReady(kCFAllocatorDefault, block, m_format.ref, 1, 0, nullptr, 1, &size, &buffer) != noErr) {
          return noErr;
        }
        CfRef<CMSampleBufferRef> bufferRef;
        bufferRef.reset(buffer);
        VTDecodeInfoFlags info = 0;
        return VTDecompressionSessionDecodeFrame(m_session.ref, buffer, 0, nullptr, &info);
      }

      static void output(void* self, void*, OSStatus status, VTDecodeInfoFlags, CVImageBufferRef picture, CMTime, CMTime) {
        if (status != noErr || picture == nullptr) return;
        static_cast<VtDecoder*>(self)->m_pictures.push_back(CVPixelBufferRetain(picture));
      }

      void post(CVPixelBufferRef picture, VideoSink& sink) {
        const int pictureW = static_cast<int>(CVPixelBufferGetWidth(picture));
        const int pictureH = static_cast<int>(CVPixelBufferGetHeight(picture));
        int w = 0;
        int h = 0;
        sink.target(pictureW, pictureH, w, h);
        CVPixelBufferRef source = picture;
        if (w != pictureW || h != pictureH) {
          if (!scaleTo(picture, w, h)) return;
          source = m_scaled.ref;
        }
        if (CVPixelBufferLockBaseAddress(source, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) return;
        const auto* base = static_cast<const std::uint8_t*>(CVPixelBufferGetBaseAddress(source));
        const std::size_t stride = CVPixelBufferGetBytesPerRow(source);
        const auto width = static_cast<std::size_t>(w);
        MirrorDecoder::Frame frame{.rgba = std::vector<std::uint8_t>(width * static_cast<std::size_t>(h) * 4), .width = w,
                                   .height = h};
        for (std::size_t y = 0; y < static_cast<std::size_t>(h); ++y) {
          const std::uint8_t* in = base + y * stride;
          std::uint8_t* out = frame.rgba.data() + y * width * 4;
          for (std::size_t x = 0; x < width; ++x, in += 4, out += 4) {
            out[0] = in[2];
            out[1] = in[1];
            out[2] = in[0];
            out[3] = in[3];
          }
        }
        CVPixelBufferUnlockBaseAddress(source, kCVPixelBufferLock_ReadOnly);
        sink.post(std::move(frame));
      }

      // Scales into m_scaled, which is copied out before the next picture.
      bool scaleTo(CVPixelBufferRef picture, int w, int h) {
        if (m_transfer.ref == nullptr) {
          VTPixelTransferSessionRef transfer = nullptr;
          if (VTPixelTransferSessionCreate(kCFAllocatorDefault, &transfer) != noErr) return false;
          m_transfer.ref = transfer;
        }
        if (m_scaled.ref == nullptr || static_cast<int>(CVPixelBufferGetWidth(m_scaled.ref)) != w ||
            static_cast<int>(CVPixelBufferGetHeight(m_scaled.ref)) != h) {
          CVPixelBufferRef scaled = nullptr;
          if (CVPixelBufferCreate(kCFAllocatorDefault, static_cast<std::size_t>(w), static_cast<std::size_t>(h),
                                  kCVPixelFormatType_32BGRA, nullptr, &scaled) != kCVReturnSuccess) {
            return false;
          }
          m_scaled.reset(scaled);
        }
        return VTPixelTransferSessionTransferImage(m_transfer.ref, picture, m_scaled.ref) == noErr;
      }

      bool m_hevc;
      std::array<std::vector<std::uint8_t>, 3> m_parameters;
      CfRef<CMVideoFormatDescriptionRef> m_format;
      CfRef<VTDecompressionSessionRef> m_session;
      CfRef<VTPixelTransferSessionRef> m_transfer;
      CfRef<CVPixelBufferRef> m_scaled;
      std::vector<CVPixelBufferRef> m_pictures;
    };

  } // namespace

  const std::vector<std::string>& mirrorCodecs() {
    static const std::vector<std::string> codecs = [] {
      std::vector<std::string> out;
      if (VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC)) out.emplace_back("hevc");
      return out;
    }();
    return codecs;
  }

  std::unique_ptr<VideoDecoder> openVideoDecoder(MirrorCodec codec, std::string& why) {
    if (codec == MirrorCodec::Av1) {
      why = "This desktop has no AV1 decoder.";
      return nullptr;
    }
    return std::make_unique<VtDecoder>(codec);
  }

} // namespace linkapp
