// The Mirror page: the phone's screen on the sky, its keys and switches.
#include "link_app.h"
#include "link_ui.h"

#include "app/app_window.h"
#include "core/input/input_codes.h"
#include "core/input/key_modifiers.h"
#include "i18n/i18n.h"
#include "ui/controls/alert.h"
#include "ui/controls/image.h"
#include "ui/controls/input.h"
#include "ui/builders.h"
#include "ui/controls/flex.h"
#include "ui/style.h"

#include <algorithm>
#include <format>
#include <unistd.h>
#include <xkbcommon/xkbcommon-keysyms.h>

namespace linkapp {

  namespace {
    constexpr std::int32_t kAndroidPower = 26; // KeyEvent.KEYCODE_POWER
  } // namespace

  std::string LinkApp::mirrorStat() const {
    const Mirror& mirror = m_client->mirror();
    const std::string transport = m_client->presence().phone.transport == "bluetooth" ? "Bluetooth" : "Wi-Fi";
    const std::string keys = i18n::tr(m_mirrorArea != nullptr && m_mirrorArea->focused() ? "link.mirror.typing" : "link.mirror.click-to-type");
    return std::format("{} × {} · {} fps · {} · {}", mirror.width, mirror.height, m_client->mirrorFps(), transport, keys);
  }

  void LinkApp::mirrorKeyInput(const InputArea::KeyData& data) {
    if (data.preedit) return;
    if (!data.pressed) {
      const auto held = m_mirrorHeld.find(androidKeyCode(data.sym));
      if (held == m_mirrorHeld.end()) return;
      m_client->mirrorKeyEvent(held->first, held->second, false);
      m_mirrorHeld.erase(held);
      return;
    }
    const KeyMods mods{.shift = (data.modifiers & KeyMod::Shift) != 0, .ctrl = (data.modifiers & KeyMod::Ctrl) != 0,
                       .alt = (data.modifiers & KeyMod::Alt) != 0, .super = (data.modifiers & KeyMod::Super) != 0};
    if (data.sym == XKB_KEY_F11) {
      m_window->toggleFullscreen();
      return;
    }
    // Ctrl+V types this computer's clipboard on the phone.
    if (mods.ctrl && !mods.alt && !mods.super && (data.sym == XKB_KEY_v || data.sym == XKB_KEY_V)) {
      if (TextClipboard* clipboard = Input::textClipboard(); clipboard != nullptr) {
        clipboard->requestClipboardText([this](std::optional<std::string> text) {
          if (text.has_value() && !text->empty()) m_client->mirrorText(*text);
        });
      }
      return;
    }
    const MirrorKey key = mirrorKeyFor(data.sym, data.utf32, mods);
    if (key.kind == MirrorKey::Kind::Text) {
      m_client->mirrorText(key.text);
    } else if (key.kind == MirrorKey::Kind::Code) {
      m_mirrorHeld[key.code] = key.meta;
      m_client->mirrorKeyEvent(key.code, key.meta, true);
    }
  }

  void LinkApp::releaseMirrorKeys() {
    for (const auto& [code, meta] : m_mirrorHeld) m_client->mirrorKeyEvent(code, meta, false);
    m_mirrorHeld.clear();
  }

  void LinkApp::confirmLock() {
    m_window->presentAlert(
        {.title = i18n::tr("link.mirror.lock-title", "phone", m_client->presence().phone.name),
         .body = i18n::tr("link.mirror.lock-body"),
         .buttons = {{.text = i18n::tr("common.actions.cancel"), .onClick = [this] { m_window->dismissSheet(); }},
                     {.text = i18n::tr("link.mirror.lock"), .variant = ButtonVariant::Primary,
                      .onClick = [this] {
                        m_window->dismissSheet();
                        m_client->mirrorKey(kAndroidPower);
                      }}},
         .defaultIndex = 1},
        [this] { m_window->dismissSheet(); });
  }

  void LinkApp::retireMirrorFrame(bool all) {
    if (m_renderer == nullptr) return;
    TextureManager& textures = m_renderer->textureManager();
    if (m_mirrorHold != nullptr || all) {
      m_mirrorRetired.push_back({.hold = std::move(m_mirrorHold), .fence = textures.submittedWorkFence()});
      textures.unload(m_mirrorTexture);
    }
    // At most 2 frames wait; an older fence is from paints frames ago, so the wait is short.
    while (!m_mirrorRetired.empty()) {
      RetiredFrame& oldest = m_mirrorRetired.front();
      pollfd fd{.fd = oldest.fence, .events = POLLIN, .revents = 0};
      const bool waitOldest = all || m_mirrorRetired.size() > 2;
      if (oldest.fence >= 0 && ::poll(&fd, 1, waitOldest ? -1 : 0) == 0) break;
      if (oldest.fence >= 0) ::close(oldest.fence);
      m_mirrorRetired.erase(m_mirrorRetired.begin());
    }
  }

  void LinkApp::showMirrorFrame(MirrorDecoder::Frame frame) {
    if (m_renderer == nullptr) return;
    TextureManager& textures = m_renderer->textureManager();
    bool uploaded = false;
    if (frame.dmabuf.fd >= 0) {
      TextureHandle imported = textures.importDmabuf(frame.dmabuf);
      if (!imported.valid()) {
        m_client->refuseMirrorDmabuf();
        return;
      }
      retireMirrorFrame(false);
      textures.unload(m_mirrorTexture);
      m_mirrorTexture = imported;
      m_mirrorHold = std::move(frame.hold);
      uploaded = true;
    } else {
      retireMirrorFrame(false); // an imported texture must not take updateSubImage: it writes into the VA surface
      const bool same = m_mirrorTexture.valid() && m_mirrorTexture.width == frame.width && m_mirrorTexture.height == frame.height;
      uploaded =
          (same && textures.updateSubImage(m_mirrorTexture, frame.rgba.data(), 0, 0, frame.width, frame.height, TextureDataFormat::Rgba)) ||
          textures.replace(m_mirrorTexture, frame.rgba.data(), frame.width, frame.height, TextureDataFormat::Rgba);
    }
    if (!uploaded || m_mirrorImage == nullptr) return;
    m_mirrorImage->setExternalTexture(*m_renderer, m_mirrorTexture);
    m_mirrorImage->markPaintDirty();
    if (m_mirrorStat != nullptr) m_mirrorStat->setText(mirrorStat());
    m_window->surface()->requestRedraw();
  }

  // Mirror (flow.html#page=mirror): a dark stage, the phone's screen centred, its size and rate above, keys below.
  std::unique_ptr<Node> LinkApp::mirror(float width, float height) {
    const float s = m_scale;
    const Mirror& mirror = m_client->mirror();
    const std::string phone = m_client->presence().phone.name;
    std::vector<std::unique_ptr<Node>> caption;
    caption.push_back(text(i18n::tr("link.pages.mirror"), Tokens::typeHeadline, ColorRole::Label1, s));
    auto name = text(phone, Tokens::typeFootnote, ColorRole::Label2, s);
    name->setFlexGrow(1.0F);
    caption.push_back(std::move(name));
    if (mirror.kind == Mirror::Kind::Showing || mirror.kind == Mirror::Kind::Asking) {
      caption.push_back(ui::button({.text = i18n::tr("link.mirror.stop"), .variant = ButtonVariant::Ghost, .onClick = [this] { m_client->stopMirror(); }}));
    }

    const float bodyH = height - Tokens::componentWindowCaption * s;
    auto stage = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP16 * s,
                             .fill = colorSpecFromRole(ColorRole::View), .flexGrow = 1.0F});
    stage->setPaletteTone(PaletteTone::Dark);
    const auto start = [this](const char* label) {
      return ui::button({.text = i18n::tr(label), .variant = ButtonVariant::Primary, .onClick = [this] { m_client->startMirror(); }});
    };
    switch (mirror.kind) {
    case Mirror::Kind::Idle:
      stage->addChild(emptyState("device-mobile", i18n::tr("link.mirror.idle-title", "phone", phone), i18n::tr("link.mirror.idle-body"), s, start("link.mirror.start")));
      break;
    case Mirror::Kind::Asking:
      stage->addChild(emptyState("device-mobile", i18n::tr("link.mirror.asking-title"), i18n::tr("link.mirror.asking-body", "phone", phone), s));
      break;
    case Mirror::Kind::Ended:
      stage->addChild(emptyState("device-mobile-off", i18n::tr("link.mirror.ended-title"), i18n::tr("link.mirror.ended-body", "phone", phone), s, start("link.mirror.start-again")));
      break;
    case Mirror::Kind::Failed:
      stage->addChild(emptyState("device-mobile-off", i18n::tr("link.mirror.failed-title"), mirror.error, s, start("link.common.try-again")));
      break;
    case Mirror::Kind::Showing: {
      auto stat = text(mirrorStat(), Tokens::typeFootnote, ColorRole::Label2, s);
      m_mirrorStat = stat.get();
      stage->addChild(std::move(stat));
      // The video fills the stage height less the stat line, the key bar and their gaps, in the phone's aspect.
      const float aspect = static_cast<float>(mirror.width) / static_cast<float>(mirror.height);
      float videoH = std::max(1.0F, bodyH - Tokens::componentLinkMirrorChrome * s);
      float videoW = videoH * aspect;
      const float maxW = width - 2.0F * Tokens::spaceP24 * s;
      if (videoW > maxW) {
        videoW = maxW;
        videoH = videoW / aspect;
      }
      m_client->setMirrorTarget(static_cast<int>(std::lround(videoW)), static_cast<int>(std::lround(videoH)));
      Image* image = nullptr;
      auto area = ui::inputArea({.width = videoW, .height = videoH});
      // Keys reach the phone only while the video holds focus; a click gives it focus, Tab stays inside.
      InputArea* keys = area.get();
      m_mirrorArea = keys;
      area->setFocusable(true);
      area->setTabFocusKey("link-mirror");
      area->setCapturesAllKeys(true);
      area->setCapturesTab(true);
      area->setRetainsFocusOnPointerRelease(true);
      area->setOnKeyDown([this](const InputArea::KeyData& data) { mirrorKeyInput(data); });
      area->setOnKeyUp([this](const InputArea::KeyData& data) { mirrorKeyInput(data); });
      area->setOnFocusGain([this] {
        if (m_mirrorStat != nullptr) m_mirrorStat->setText(mirrorStat());
      });
      area->setOnFocusLoss([this] {
        releaseMirrorKeys();
        if (m_mirrorStat != nullptr) m_mirrorStat->setText(mirrorStat());
      });
      const float corner = m_client->mirrorCorner() > 0 ? videoW * static_cast<float>(m_client->mirrorCorner()) / 10000.0F
                                                        : Style::scaledRadius(Tokens::componentLinkMirrorRadius, s);
      area->addChild(ui::image({.out = &image, .fit = ImageFit::Contain, .radius = corner,
                                .width = videoW, .height = videoH}));
      m_mirrorImage = image;
      if (m_mirrorTexture.valid()) image->setExternalTexture(*m_renderer, m_mirrorTexture);
      // MirrorInput takes positions in 1/10000 of the video.
      const auto at = [videoW, videoH](float x, float y) {
        return std::pair{static_cast<std::int32_t>(std::clamp(x / videoW, 0.0F, 1.0F) * 10000.0F),
                         static_cast<std::int32_t>(std::clamp(y / videoH, 0.0F, 1.0F) * 10000.0F)};
      };
      area->setOnPress([this, at, keys](const InputArea::PointerData& data) {
        if (data.pressed && !keys->focused()) m_window->input().setFocus(keys);
        if (data.button != BTN_LEFT || data.pressed == m_touching) return;
        m_touching = data.pressed;
        const auto [x, y] = at(data.localX, data.localY);
        m_client->mirrorTouch(data.pressed ? "touch-down" : "touch-up", x, y);
      });
      area->setOnMotion([this, at](const InputArea::PointerData& data) {
        if (!m_touching) return;
        const auto [x, y] = at(data.localX, data.localY);
        m_client->mirrorTouch("touch-move", x, y);
      });
      area->setOnAxisHandler([this, at](const InputArea::PointerData& data) {
        // A detent is 120 in value120; a touchpad gives about 10 per detent-sized move.
        const double turn = data.axisValue120 != 0 ? data.axisValue120 * 100.0 / 120.0 : data.axisValue * 10.0;
        const auto step = static_cast<std::int32_t>(std::clamp(std::lround(turn), -10000L, 10000L));
        if (step == 0) return true;
        const auto [x, y] = at(data.localX, data.localY);
        constexpr std::uint32_t kVertical = 0; // WL_POINTER_AXIS_VERTICAL_SCROLL
        if (data.axis == kVertical) {
          m_client->mirrorWheel(x, y, 0, step);
        } else {
          m_client->mirrorWheel(x, y, step, 0);
        }
        return true;
      });
      stage->addChild(std::move(area));

      // FloatingCapsuleBar (handoff.yaml), dark glass: Back, Home, Recents | Volume, Lock.
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::Popover),
                          .radius = Tokens::sizeControlXl * s / 2.0F, .height = Tokens::sizeControlXl * s});
      bar->setPadding(0.0F, Tokens::spaceP8 * s, 0.0F, Tokens::spaceP8 * s);
      const auto key = [&](const char* glyph, const char* tip, std::function<void()> press,
                           std::function<void()> secondary = nullptr, bool on = false) {
        bar->addChild(ui::button({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s,
                                  .variant = on ? ButtonVariant::Primary : ButtonVariant::Ghost,
                                  .tooltip = i18n::tr(tip),
                                  .onClick = std::move(press), .onRightClick = std::move(secondary)}));
      };
      key("chevron-left", "link.common.back", [this] { m_client->mirrorAction("back"); });
      key("home", "link.mirror.home", [this] { m_client->mirrorAction("home"); });
      key("dots", "link.mirror.recents", [this] { m_client->mirrorAction("recents"); });
      bar->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .width = s, .height = Tokens::spaceP12 * s}));
      constexpr std::int32_t kVolumeUp = 24; // Android KeyEvent codes
      constexpr std::int32_t kVolumeDown = 25;
      key("volume", "link.mirror.volume", [this] { m_client->mirrorKey(kVolumeUp); },
          [this] { m_client->mirrorKey(kVolumeDown); });
      key("lock", "link.mirror.lock", [this] { confirmLock(); });
      const bool dark = m_client->mirrorScreenOff();
      key("device-mobile-off", dark ? "link.mirror.screen-on" : "link.mirror.screen-off",
          [this, dark] { m_client->setMirrorScreenOff(!dark); }, nullptr, dark);
      key("photo", "link.mirror.screenshot", [this] { m_client->mirrorAction("screenshot"); });
      stage->addChild(std::move(bar));
      break;
    }
    }
    auto cards = ui::row({.align = FlexAlign::Stretch, .width = width, .height = height});
    cards->addChild(card(std::move(caption), std::move(stage), width, true));
    return cards;
  }

} // namespace linkapp
