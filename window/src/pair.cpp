// The Pair page: the pairing code and its QR on the forming sky.
#include "link_app.h"
#include "link_ui.h"

#include "i18n/i18n.h"
#include "ui/builders.h"
#include "ui/controls/flex.h"
#include "ui/controls/image.h"
#include "ui/palette.h"
#include "ui/style.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <qrencode.h>

namespace linkapp {

  std::unique_ptr<Node> LinkApp::pair(Renderer& renderer, float s) {
    const Pair& pair = m_client->pair();
    if (pair.kind == Pair::Kind::Failed) {
      auto actions = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
      if (!m_client->phones().empty()) {
        actions->addChild(ui::button({.text = i18n::tr("common.actions.cancel"), .variant = ButtonVariant::Tonal,
                                      .onClick = [this] { m_client->cancelPairing(); }}));
      }
      actions->addChild(ui::button({.text = i18n::tr("link.common.try-again"), .variant = ButtonVariant::Primary,
                                    .onClick = [this] { m_client->startPairing(); }}));
      return emptyState("device-mobile-off", i18n::tr("link.pair.failed"), pair.error, s, std::move(actions));
    }
    auto page = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP32 * s,
                         .flexGrow = 1.0F});
    // A scanner needs dark modules on a light ground in either scheme, so the QR view keeps the light tone.
    auto view = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .padding = Tokens::spaceP16 * s,
                            .fill = ColorSpec{.fixed = colorForRole(ColorRole::View, PaletteTone::Light)},
                            .radius = Style::scaledRadius(Tokens::radiusR16, s),
                            .border = colorSpecFromRole(ColorRole::Stroke), .borderWidth = 0.5F * s,
                            .width = Tokens::componentLinkQr * s, .height = Tokens::componentLinkQr * s});
    const float drawn = (Tokens::componentLinkQr - 2.0F * Tokens::spaceP16) * s;
    Image* image = nullptr;
    view->addChild(ui::image({.out = &image, .fit = ImageFit::Contain, .width = drawn, .height = drawn}));
    const std::unique_ptr<QRcode, decltype(&QRcode_free)> qr(
        pair.kind == Pair::Kind::Showing ? QRcode_encodeString(pair.uri.c_str(), 0, QR_ECLEVEL_M, QR_MODE_8, 1) : nullptr,
        &QRcode_free);
    if (qr != nullptr) {
      // Whole texels per module at twice the drawn size keep the module edges sharp when scaled.
      const int texels = std::max(1, static_cast<int>(std::ceil(2.0F * drawn / static_cast<float>(qr->width))));
      const int side = qr->width * texels;
      const Color ink = colorForRole(ColorRole::Label1, PaletteTone::Light);
      const std::array<std::uint8_t, 4> dark{static_cast<std::uint8_t>(ink.r * 255.0F), static_cast<std::uint8_t>(ink.g * 255.0F),
                                             static_cast<std::uint8_t>(ink.b * 255.0F), 0xFF};
      std::vector<std::uint8_t> rgba(static_cast<std::size_t>(side) * static_cast<std::size_t>(side) * 4, 0);
      for (int y = 0; y < side; ++y) {
        for (int x = 0; x < side; ++x) {
          if ((qr->data[((y / texels) * qr->width) + (x / texels)] & 1) == 0) continue;
          std::ranges::copy(dark, rgba.begin() + static_cast<std::ptrdiff_t>((static_cast<std::size_t>(y) * side + x) * 4));
        }
      }
      (void)image->setSourceRaw(renderer, rgba.data(), rgba.size(), side, side, side * 4, PixmapFormat::RGBA);
    }
    page->addChild(std::move(view));

    auto steps = ui::column({.align = FlexAlign::Start, .gap = Tokens::spaceP12 * s,
                             .maxWidth = Tokens::componentLinkPairText * s});
    steps->addChild(text(i18n::tr("link.pair.scan"), Tokens::typeTitle2, ColorRole::Label1, s));
    auto how = text(i18n::tr("link.pair.how"), Tokens::typeParagraph, ColorRole::Label2, s, 4);
    how->setMaxWidth(Tokens::componentLinkPairText * s);
    steps->addChild(std::move(how));
    steps->addChild(text(i18n::tr("link.pair.code"), Tokens::typeParagraph, ColorRole::Label1, s));
    auto code = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    for (const char digit : pair.code) {
      auto tile = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center,
                              .fill = colorSpecFromRole(ColorRole::Fill1), .radius = Style::scaledRadius(Tokens::radiusR8, s),
                              .width = Tokens::componentLinkCodeDigitW * s, .height = Tokens::componentLinkCodeDigitH * s});
      tile->addChild(text(std::string(1, digit), Tokens::typeTitle2, ColorRole::Label1, s));
      code->addChild(std::move(tile));
    }
    steps->addChild(std::move(code));
    if (!m_client->phones().empty()) {
      auto cancel = ui::row({});
      cancel->setPadding(Tokens::spaceP12 * s, 0.0F, 0.0F, 0.0F);
      cancel->addChild(ui::button({.text = i18n::tr("common.actions.cancel"), .variant = ButtonVariant::Tonal,
                                   .onClick = [this] { m_client->cancelPairing(); }}));
      steps->addChild(std::move(cancel));
    }
    page->addChild(std::move(steps));
    return page;
  }

} // namespace linkapp
