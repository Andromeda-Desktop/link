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

  // pages.html PAGE pair: one card centred on the forming sky, the QR beside the steps.
  std::unique_ptr<Node> LinkApp::pair(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const Pair& pair = m_client->pair();
    // Without a phone, Home shows "No phone yet"; on any other page the app keeps a window open.
    const bool cancellable = !m_client->phones().empty() || m_page == Page::Home;
    const float pad = Tokens::spaceP24 * s;
    auto front = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .width = width, .height = height});
    auto card = ui::row({.align = FlexAlign::Center, .gap = pad, .padding = pad, .fill = colorSpecFromRole(ColorRole::SolidRegular),
                         .radius = Style::scaledRadius(Tokens::radiusR28, s), .border = colorSpecFromRole(ColorRole::Separator),
                         .borderWidth = 1.0F, .width = std::min(Tokens::componentLinkPairCard * s, 0.9F * width)});
    auto steps = ui::column({.align = FlexAlign::Start, .gap = Tokens::spaceP8 * s, .flexGrow = 1.0F});
    auto actions = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    actions->setPadding(Tokens::spaceP8 * s, 0.0F, 0.0F, 0.0F);
    const auto paragraph = [s](std::string value, ColorRole role) { return text(std::move(value), Tokens::typeParagraph, role, s, 3); };

    if (pair.kind == Pair::Kind::Failed) {
      steps->addChild(text(i18n::tr("link.pair.failed"), Tokens::typeTitle1, ColorRole::Label1, s));
      steps->addChild(paragraph(pair.error, ColorRole::Label2));
      actions->addChild(pill(i18n::tr("link.common.try-again"), std::nullopt, true, s, [this] { m_client->startPairing(); }));
      if (cancellable) actions->addChild(pill(i18n::tr("common.actions.cancel"), std::nullopt, false, s, [this] { m_client->cancelPairing(); }));
      steps->addChild(std::move(actions));
      card->addChild(std::move(steps));
      front->addChild(std::move(card));
      return front;
    }

    // A scanner needs dark modules on a light ground in either scheme, so the QR view keeps the light tone.
    const float inset = Tokens::spaceP12 * s;
    auto view = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .padding = inset,
                            .fill = ColorSpec{.fixed = colorForRole(ColorRole::View, PaletteTone::Light)},
                            .radius = Style::scaledRadius(Tokens::radiusR16, s),
                            .width = Tokens::componentLinkQr * s, .height = Tokens::componentLinkQr * s});
    const float drawn = Tokens::componentLinkQr * s - 2.0F * inset;
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
    card->addChild(std::move(view));

    steps->addChild(text(i18n::tr("link.pair.scan"), Tokens::typeTitle1, ColorRole::Label1, s));
    steps->addChild(paragraph(i18n::tr("link.pair.how"), ColorRole::Label2));
    steps->addChild(paragraph(i18n::tr("link.pair.code"), ColorRole::Label1));
    auto code = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    for (const char digit : pair.code) {
      auto tile = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center,
                              .fill = colorSpecFromRole(ColorRole::Fill1), .radius = Style::scaledRadius(Tokens::radiusR12, s),
                              .width = Tokens::componentLinkCodeDigitW * s, .height = Tokens::componentLinkCodeDigitH * s});
      tile->addChild(text(std::string(1, digit), Tokens::typeTitle1, ColorRole::Label1, s));
      code->addChild(std::move(tile));
    }
    steps->addChild(std::move(code));
    if (cancellable) {
      actions->addChild(pill(i18n::tr("common.actions.cancel"), std::nullopt, false, s, [this] { m_client->cancelPairing(); }));
      steps->addChild(std::move(actions));
    }
    card->addChild(std::move(steps));
    front->addChild(std::move(card));
    return front;
  }

} // namespace linkapp
