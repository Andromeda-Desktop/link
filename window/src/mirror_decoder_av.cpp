#include "mirror_video.h"

#include "core/log.h"

#include <algorithm>
#include <array>
#include <format>
#include <optional>

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>
#include <libavutil/hwcontext_drm.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_vaapi.h>
#include <libavutil/imgutils.h>
#include <libswscale/swscale.h>
}

namespace linkapp {

  namespace {

    constexpr Logger kLog("link-mirror");
    static_assert(VideoDecoder::kPadding >= AV_INPUT_BUFFER_PADDING_SIZE);

    struct VaDecode {
      bool hevc = false;
      bool av1 = false;
    };

    bool vaDecodes(VADisplay display, VAProfile profile) {
      std::vector<VAEntrypoint> entrypoints(static_cast<std::size_t>(vaMaxNumEntrypoints(display)));
      int count = 0;
      if (vaQueryConfigEntrypoints(display, profile, entrypoints.data(), &count) != VA_STATUS_SUCCESS) return false;
      return std::find(entrypoints.begin(), entrypoints.begin() + count, VAEntrypointVLD) != entrypoints.begin() + count;
    }

    const VaDecode& vaDecode() {
      static const VaDecode decode = [] {
        AVBufferRef* device = nullptr;
        if (av_hwdevice_ctx_create(&device, AV_HWDEVICE_TYPE_VAAPI, nullptr, nullptr, 0) < 0) return VaDecode{};
        const VADisplay display =
            static_cast<AVVAAPIDeviceContext*>(reinterpret_cast<AVHWDeviceContext*>(device->data)->hwctx)->display;
        const VaDecode found{.hevc = vaDecodes(display, VAProfileHEVCMain), .av1 = vaDecodes(display, VAProfileAV1Profile0)};
        av_buffer_unref(&device);
        return found;
      }();
      return decode;
    }

    struct Decoder {
      const AVCodec* codec = nullptr;
      bool vaapi = false;
      const char* name = "";
    };

    // HEVC is listed only with VA-API, so it always gets the device; software AV1 is dav1d (FFmpeg's own av1
    // decoder needs a hardware accelerator).
    Decoder decoderFor(MirrorCodec codec) {
      switch (codec) {
      case MirrorCodec::H264:
        return {.codec = avcodec_find_decoder(AV_CODEC_ID_H264), .vaapi = true, .name = "H.264"};
      case MirrorCodec::Hevc:
        return {.codec = avcodec_find_decoder(AV_CODEC_ID_HEVC), .vaapi = true, .name = "HEVC"};
      case MirrorCodec::Av1:
        return vaDecode().av1 ? Decoder{.codec = avcodec_find_decoder_by_name("av1"), .vaapi = true, .name = "AV1"}
                              : Decoder{.codec = avcodec_find_decoder_by_name("libdav1d"), .vaapi = false, .name = "AV1"};
      }
      return {};
    }

    AVPixelFormat pickFormat(AVCodecContext* ctx, const AVPixelFormat* formats) {
      for (const AVPixelFormat* f = formats; *f != AV_PIX_FMT_NONE; ++f) {
        if (*f == AV_PIX_FMT_VAAPI && ctx->hw_device_ctx != nullptr) return *f;
      }
      for (const AVPixelFormat* f = formats; *f != AV_PIX_FMT_NONE; ++f) {
        if ((av_pix_fmt_desc_get(*f)->flags & AV_PIX_FMT_FLAG_HWACCEL) == 0) return *f;
      }
      return AV_PIX_FMT_NONE;
    }

    // scale_vaapi from a decoder's VA surfaces to BGRA at one output size, on the GPU.
    struct Vpp {
      AVFilterGraph* graph = nullptr;
      AVFilterContext* source = nullptr;
      AVFilterContext* sink = nullptr;
      int inW = 0, inH = 0, outW = 0, outH = 0;

      Vpp() = default;
      Vpp(const Vpp&) = delete;
      Vpp& operator=(const Vpp&) = delete;
      ~Vpp() { avfilter_graph_free(&graph); }

      bool build(const AVFrame* in, int w, int h) {
        avfilter_graph_free(&graph);
        inW = in->width, inH = in->height, outW = w, outH = h;
        graph = avfilter_graph_alloc();
        if (graph == nullptr) return false;
        source = avfilter_graph_alloc_filter(graph, avfilter_get_by_name("buffer"), "in");
        AVFilterContext* scale = avfilter_graph_alloc_filter(graph, avfilter_get_by_name("scale_vaapi"), "scale");
        sink = avfilter_graph_alloc_filter(graph, avfilter_get_by_name("buffersink"), "out");
        AVBufferSrcParameters* params = av_buffersrc_parameters_alloc();
        if (source == nullptr || scale == nullptr || sink == nullptr || params == nullptr) {
          av_free(params);
          return false;
        }
        params->format = AV_PIX_FMT_VAAPI;
        params->width = in->width;
        params->height = in->height;
        params->time_base = {1, 1'000'000};
        params->color_space = in->colorspace;
        params->color_range = in->color_range;
        params->hw_frames_ctx = in->hw_frames_ctx;
        const int set = av_buffersrc_parameters_set(source, params);
        av_free(params);
        // The UI holds up to 3 output surfaces (shown, retiring, posted) beyond the filter's own.
        scale->extra_hw_frames = 4;
        const std::string args = std::format("w={}:h={}:format=bgra", w, h);
        return set >= 0 && avfilter_init_str(source, nullptr) >= 0 && avfilter_init_str(scale, args.c_str()) >= 0 &&
               avfilter_init_str(sink, nullptr) >= 0 && avfilter_link(source, 0, scale, 0) >= 0 &&
               avfilter_link(scale, 0, sink, 0) >= 0 && avfilter_graph_config(graph, nullptr) >= 0;
      }

      // `decoded` scaled to w x h as one ARGB8888 dmabuf plane; nullopt when VA-API cannot.
      std::optional<MirrorDecoder::Frame> frame(AVFrame* decoded, int w, int h) {
        if ((graph == nullptr || inW != decoded->width || inH != decoded->height || outW != w || outH != h) &&
            !build(decoded, w, h)) {
          return std::nullopt;
        }
        if (av_buffersrc_add_frame_flags(source, decoded, AV_BUFFERSRC_FLAG_KEEP_REF) < 0) return std::nullopt;
        AVFrame* scaled = av_frame_alloc();
        AVFrame* drm = av_frame_alloc();
        const auto drop = [&] {
          av_frame_free(&scaled);
          av_frame_free(&drm);
          return std::nullopt;
        };
        if (scaled == nullptr || drm == nullptr || av_buffersink_get_frame(sink, scaled) < 0) return drop();
        drm->format = AV_PIX_FMT_DRM_PRIME;
        if (av_hwframe_map(drm, scaled, AV_HWFRAME_MAP_READ) < 0) return drop();
        const auto* desc = reinterpret_cast<const AVDRMFrameDescriptor*>(drm->data[0]);
        if (desc->nb_layers != 1 || desc->layers[0].nb_planes != 1) return drop();
        const AVDRMPlaneDescriptor& plane = desc->layers[0].planes[0];
        const AVDRMObjectDescriptor& object = desc->objects[plane.object_index];
        MirrorDecoder::Frame out{
            .width = scaled->width,
            .height = scaled->height,
            .dmabuf = {.fd = object.fd, .width = scaled->width, .height = scaled->height, .fourcc = desc->layers[0].format,
                       .modifier = object.format_modifier, .stride = static_cast<std::uint32_t>(plane.pitch),
                       .offset = static_cast<std::uint32_t>(plane.offset)},
        };
        av_frame_free(&scaled); // the mapping keeps its own reference to the surface
        out.hold = std::shared_ptr<AVFrame>(drm, [](AVFrame* f) { av_frame_free(&f); });
        return out;
      }
    };

    // libav with VA-API when the GPU has it: BGRA dmabufs scaled on the GPU, else RGBA bytes from swscale.
    class AvDecoder final : public VideoDecoder {
    public:
      AvDecoder(const Decoder& decoder, AVCodecContext* ctx) : m_name(decoder.name), m_ctx(ctx) {}
      ~AvDecoder() override {
        sws_freeContext(m_scale);
        av_frame_free(&m_mapped);
        av_frame_free(&m_decoded);
        av_packet_free(&m_packet);
        avcodec_free_context(&m_ctx);
      }
      AvDecoder(const AvDecoder&) = delete;
      AvDecoder& operator=(const AvDecoder&) = delete;

      [[nodiscard]] bool ready() const { return m_packet != nullptr && m_decoded != nullptr && m_mapped != nullptr; }

      std::string decode(const std::uint8_t* unit, std::size_t length, VideoSink& sink) override {
        m_packet->data = const_cast<std::uint8_t*>(unit); // libav reads a packet it does not own; it never writes it
        m_packet->size = static_cast<int>(length);
        if (avcodec_send_packet(m_ctx, m_packet) < 0) return {};
        while (avcodec_receive_frame(m_ctx, m_decoded) == 0) {
          if (!sink.wantsFrame()) continue;
          if (m_decoded->format == AV_PIX_FMT_VAAPI && !sink.rgbaOnly()) {
            int w = 0;
            int h = 0;
            sink.target(m_decoded->width, m_decoded->height, w, h);
            if (auto frame = m_vpp.frame(m_decoded, w, h)) {
              sink.post(std::move(*frame));
              continue;
            }
            kLog.info("mirror: VA-API scaling or export failed, copying frames to the CPU");
            sink.refuseDmabuf();
          }
          AVFrame* source = m_decoded;
          if (m_decoded->format == AV_PIX_FMT_VAAPI) {
            av_frame_unref(m_mapped);
            if (av_hwframe_transfer_data(m_mapped, m_decoded, 0) < 0) continue;
            source = m_mapped;
          }
          int w = 0;
          int h = 0;
          sink.target(source->width, source->height, w, h);
          m_scale = sws_getCachedContext(m_scale, source->width, source->height, static_cast<AVPixelFormat>(source->format), w,
                                         h, AV_PIX_FMT_RGBA, SWS_BILINEAR, nullptr, nullptr, nullptr);
          if (m_scale == nullptr) continue;
          MirrorDecoder::Frame frame{
              .rgba = std::vector<std::uint8_t>(static_cast<std::size_t>(w) * static_cast<std::size_t>(h) * 4), .width = w,
              .height = h};
          std::array<std::uint8_t*, 4> planes{frame.rgba.data(), nullptr, nullptr, nullptr};
          std::array<int, 4> strides{w * 4, 0, 0, 0};
          sws_scale(m_scale, source->data, source->linesize, 0, source->height, planes.data(), strides.data());
          sink.post(std::move(frame));
        }
        return {};
      }

    private:
      std::string m_name;
      AVCodecContext* m_ctx;
      AVPacket* m_packet = av_packet_alloc();
      AVFrame* m_decoded = av_frame_alloc();
      AVFrame* m_mapped = av_frame_alloc();
      SwsContext* m_scale = nullptr;
      Vpp m_vpp;
    };

  } // namespace

  const std::vector<std::string>& mirrorCodecs() {
    static const std::vector<std::string> codecs = [] {
      std::vector<std::string> out;
      if (vaDecode().hevc && avcodec_find_decoder(AV_CODEC_ID_HEVC) != nullptr) out.emplace_back("hevc");
      if (decoderFor(MirrorCodec::Av1).codec != nullptr) out.emplace_back("av1");
      return out;
    }();
    return codecs;
  }

  std::unique_ptr<VideoDecoder> openVideoDecoder(MirrorCodec codec, std::string& why) {
    const Decoder decoder = decoderFor(codec);
    AVCodecContext* ctx = decoder.codec != nullptr ? avcodec_alloc_context3(decoder.codec) : nullptr;
    if (ctx == nullptr) {
      why = std::format("This desktop has no {} decoder.", decoder.name);
      return nullptr;
    }
    if (decoder.vaapi && av_hwdevice_ctx_create(&ctx->hw_device_ctx, AV_HWDEVICE_TYPE_VAAPI, nullptr, nullptr, 0) < 0) {
      kLog.info("mirror: no VA-API device, decoding in software");
    }
    kLog.info("mirror: {} via {}{}", decoder.name, decoder.codec->name, ctx->hw_device_ctx != nullptr ? " + VA-API" : "");
    ctx->get_format = pickFormat;
    ctx->thread_count = 0;
    auto out = std::make_unique<AvDecoder>(decoder, ctx);
    if (!out->ready() || avcodec_open2(ctx, decoder.codec, nullptr) < 0) {
      why = std::format("The {} decoder did not start.", decoder.name);
      return nullptr;
    }
    return out;
  }

} // namespace linkapp
