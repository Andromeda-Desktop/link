// Home and the sky over every page (mockups/link-desk-v2: concept-g.html, handoff.yaml home and band).
#include "link_app.h"
#include "link_ui.h"
#include "sky.h"

#include "app/app_window.h"
#include "core/text_clipboard.h"
#include "i18n/i18n.h"
#include "render/core/color.h"
#include "ui/builders.h"
#include "ui/controls/flex.h"
#include "ui/controls/input.h"
#include "ui/controls/progress_bar.h"
#include "ui/controls/segmented.h"
#include "ui/controls/slider.h"
#include "ui/style.h"

#include <algorithm>
#include <format>

namespace linkapp {

  namespace {
    // The sheet over the sky's bottom edge. A solid tail of `tail` px continues it below the column, so a sheet that
    // rides up with a morphing sky leaves no gap.
    std::unique_ptr<Flex> sheetHolder(std::unique_ptr<Flex> sheet, float width, float height, float overlap, float tail, float s) {
      const float margin = Tokens::spaceP12 * s;
      auto below = ui::box({.fill = colorSpecFromRole(ColorRole::SolidRegular), .width = width - 2.0F * margin, .height = tail});
      below->setParticipatesInLayout(false);
      below->setPosition(margin, height);
      auto holder = ui::row({.width = width, .height = height});
      holder->setPadding(0.0F, margin, 0.0F, margin);
      holder->setTranslation(0.0F, -overlap);
      holder->addChild(std::move(sheet));
      holder->addChild(std::move(below));
      return holder;
    }

    // Star anchors of the running things (handoff.yaml home StarLeaders).
    constexpr std::array<float, 3> kStarMedia{0.58F, 0.02F, 0.58F};
    constexpr std::array<float, 3> kStarCamera{0.72F, 0.02F, -0.12F};
    constexpr std::array<float, 3> kStarMic{-0.5F, 0.02F, 0.5F};
    constexpr std::array<float, 3> kStarSound{-0.66F, 0.02F, -0.2F};
    constexpr std::array<float, 3> kStarTransfer{-0.4F, 0.02F, 0.4F};
    // Band chips show their labels from this window width; a label is at most kBandChipLabel wide (pages.html .ch).
    constexpr float kBandChipLabels = 1240.0F;
    constexpr float kBandChipLabel = 120.0F;
    constexpr int kHomeWaiting = 4;
    constexpr int kCoolBattery = 15; // below it the core burns red (temp 1900 K)

    std::string transportName(std::string_view transport) {
      return transport == "bluetooth" ? i18n::tr("link.home.bluetooth") : i18n::tr("link.home.wifi");
    }

    std::string clockOf(std::int64_t ms) {
      const std::int64_t total = std::max<std::int64_t>(0, ms / 1000);
      return std::format("{}:{:02}", total / 60, total % 60);
    }

    Button::ButtonPalette chipPalette(bool on) {
      if (on) return playPalette();
      const Button::ButtonStateColors rest{.bg = colorSpecFromRole(ColorRole::Card), .border = colorSpecFromRole(ColorRole::Separator),
                                           .label = colorSpecFromRole(ColorRole::Label2)};
      const Button::ButtonStateColors hot{.bg = colorSpecFromRole(ColorRole::Fill2), .border = colorSpecFromRole(ColorRole::Separator),
                                          .label = colorSpecFromRole(ColorRole::Label1)};
      return {.borderWidth = 1.0F, .normal = rest, .hover = hot, .pressed = hot, .disabled = rest};
    }

    // A capsule with a glyph and a label: a phone switch (PhoneSwitchChip) or a pill action.
    std::unique_ptr<Button> capsule(std::string label, std::string glyph, bool on, float height, float s, std::function<void()> click,
                                    bool enabled = true) {
      std::optional<std::string> symbol;
      if (!glyph.empty()) symbol = std::move(glyph);
      return ui::button({.text = std::move(label), .glyph = std::move(symbol),
                         .fontSize = Tokens::typeBody.size * s, .glyphSize = Tokens::sizeSymbol * s, .controlHeight = height, .enabled = enabled,
                         .customPalette = chipPalette(on), .paddingH = Tokens::componentButtonInsetM * s, .gap = Tokens::spaceP8 * s,
                         .radius = height / 2.0F, .onClick = std::move(click)});
    }

    std::unique_ptr<Button> roundKey(const char* glyph, bool filled, float size, float s, std::function<void()> click, bool enabled = true) {
      return ui::button({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = size, .enabled = enabled,
                         .customPalette = filled ? playPalette() : chipPalette(false), .radius = size / 2.0F, .width = size,
                         .onClick = std::move(click)});
    }

    // A round glyph plate: core fill while on, a core ring while asking, fill_2 otherwise.
    std::unique_ptr<Node> plate(const char* glyph, float size, std::string_view state, float s) {
      const bool on = state == "on";
      const bool asking = state == "asking";
      auto box = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center,
                          .fill = on ? core() : asking ? clearColorSpec() : colorSpecFromRole(ColorRole::Fill2), .radius = size / 2.0F,
                          .border = asking ? std::optional(core()) : std::nullopt, .borderWidth = asking ? 2.0F * s : 0.0F,
                          .width = size, .height = size});
      box->addChild(ui::glyph({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s,
                               .color = on ? fixedColorSpec(rgbaHex(Tokens::componentLinkCoreInk))
                                        : asking ? core()
                                                 : colorSpecFromRole(ColorRole::Label2)}));
      return box;
    }

    std::unique_ptr<Node> dot(ColorSpec fill, bool ring, float size, float s) {
      return ui::box({.fill = ring ? clearColorSpec() : fill, .border = ring ? std::optional(fill) : std::nullopt,
                      .borderWidth = ring ? 1.5F * s : 0.0F, .radius = size / 2.0F, .width = size, .height = size});
    }

    std::unique_ptr<Node> twoLines(std::string top, std::string bottom, ColorRole bottomRole, float s) {
      auto col = ui::column({.align = FlexAlign::Stretch, .gap = 2.0F * s, .flexGrow = 1.0F});
      col->addChild(text(std::move(top), Tokens::typeHeadline, ColorRole::Label1, s));
      col->addChild(text(std::move(bottom), Tokens::typeFootnote, bottomRole, s));
      return col;
    }
  } // namespace

  // CallBar (handoff.yaml): who calls and the phone's call keys, centred over the sky; none without a call.
  std::unique_ptr<Node> LinkApp::callBar(Renderer& renderer, float width, float top) {
    const float s = m_scale;
    const std::string shown = m_client->presence().phone.id;
    const PhoneCall* call = m_client->call(shown);
    std::string device = shown;
    for (const Phone& phone : m_client->phones()) {
      if (call != nullptr) break;
      call = m_client->call(phone.id);
      device = phone.id;
    }
    if (call == nullptr) return nullptr;
    const float barWidth = std::min(Tokens::componentLinkCallbar * s, 0.7F * width);
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .padding = Tokens::spaceP8 * s,
                        .fill = colorSpecFromRole(ColorRole::SolidRegular), .radius = Style::scaledRadius(Tokens::radiusR24, s),
                        .border = colorSpecFromRole(ColorRole::Separator), .borderWidth = 1.0F, .width = barWidth});
    bar->setParticipatesInLayout(false);
    bar->setElevation(&Tokens::elevationE3, s);
    bar->setPosition((width - barWidth) / 2.0F, top);

    const float pulse = Tokens::sizeControlL * s;
    const float ring = Tokens::componentLinkCallRing * s;
    auto mark = ui::row({.width = pulse, .height = pulse});
    auto halo = ui::box({.fill = fixedColorSpec(rgbaHex(Tokens::componentLinkCallRingFill)), .radius = pulse / 2.0F + ring,
                         .width = pulse + 2.0F * ring, .height = pulse + 2.0F * ring});
    halo->setParticipatesInLayout(false);
    halo->setPosition(-ring, -ring);
    mark->addChild(std::move(halo));
    auto disc = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = fixedColorSpec(rgbaHex(Tokens::componentLinkCall)),
                         .radius = pulse / 2.0F, .width = pulse, .height = pulse});
    disc->addChild(ui::glyph({.glyph = "phone-call", .glyphSize = Tokens::sizeSymbol * s,
                              .color = fixedColorSpec(rgbaHex(Tokens::componentLinkCallInk))}));
    mark->addChild(std::move(disc));
    bar->addChild(std::move(mark));

    const bool ringing = call->state == PhoneCall::State::Ringing;
    const std::string who = !call->name.empty() ? call->name : !call->number.empty() ? call->number : !call->app.empty() ? call->app
                                                                                                           : i18n::tr("link.call.unknown");
    std::string sub = !call->name.empty() && !call->number.empty() ? call->number + " · " : std::string{};
    sub += i18n::tr(ringing ? "link.call.answer-on-phone" : "link.call.on-phone");
    auto lines = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    lines->setFlexShrink(1.0F);
    lines->addChild(text(i18n::tr(ringing ? "link.call.calling" : "link.call.with", "who", who), Tokens::typeHeadline, ColorRole::Label1, s));
    lines->addChild(text(std::move(sub), Tokens::typeFootnote, ColorRole::Label2, s));
    bar->addChild(std::move(lines));
    if (ringing && !call->muted) {
      bar->addChild(pill(i18n::tr("link.call.mute"), std::string("volume-off"), false, s, [this, device] { m_client->callAction(device, "mute"); }));
    }
    auto end = pill(i18n::tr(ringing ? "link.call.decline" : "link.call.end"), std::string("phone-off"), false, s,
                    [this, device] { m_client->callAction(device, "decline"); });
    const Button::ButtonStateColors red{.bg = colorSpecFromRole(ColorRole::Danger), .border = clearColorSpec(),
                                        .label = colorSpecFromRole(ColorRole::OnAccent)};
    end->setCustomPalette({.normal = red, .hover = red, .pressed = red, .disabled = red});
    bar->addChild(std::move(end));
    // Over the sky's front: out of its layout, laid out alone.
    bar->layout(renderer);
    return bar;
  }

  std::unique_ptr<Node> LinkApp::skyWithCall(Renderer& renderer, const SkyLook& look, float width, float height, std::unique_ptr<Node> front,
                                             std::vector<SkyAnchor> anchors) {
    const float top = (look.mode == SkyMode::Hero ? Tokens::componentLinkCallbarTop : Tokens::componentLinkCallbarBandTop) * m_scale;
    if (auto bar = callBar(renderer, width, top)) front->addChild(std::move(bar));
    return m_sky->build(look, width, height, m_window->headerHeight(), std::move(front), std::move(anchors));
  }

  SkyLife LinkApp::skyLife() const {
    if (m_client == nullptr) return SkyLife::Still;
    const Presence presence = m_client->presence();
    if (presence.kind != Presence::Kind::Connected && presence.kind != Presence::Kind::Locked) return SkyLife::Still;
    const PhonePlayer* p = player();
    if (m_client->call(presence.phone.id) != nullptr) return SkyLife::Call;
    if (p != nullptr && p->playing) return SkyLife::Music;
    const Phone& phone = presence.phone;
    const auto running = [](const std::string& state) { return state == "on"; };
    const bool transfer = std::ranges::any_of(m_client->activity().items(), [](const TransferEntry& e) { return e.state == Transfer::Running; });
    return running(phone.camera) || running(phone.mic) || running(phone.sound) || transfer ? SkyLife::Busy : SkyLife::Still;
  }

  SkyLook LinkApp::skyLook(SkyMode mode) const {
    const Presence presence = m_client != nullptr ? m_client->presence() : Presence{};
    const float dim = presence.kind == Presence::Kind::NoDaemon    ? 0.12F
                      : presence.kind == Presence::Kind::NotPaired ? 0.3F
                      : presence.kind == Presence::Kind::Away      ? 0.3F
                                                                   : 1.0F;
    const int battery = presence.phone.battery;
    return {.mode = mode, .dim = dim, .light = isResolvedLightTheme(), .cool = battery >= 0 && battery < kCoolBattery};
  }

  std::unique_ptr<Node> LinkApp::content(Renderer& renderer, float width, float height) {
    // The live card's nodes die with the old tree; only the new build may hand them to the tick.
    m_liveView = {};
    m_mirrorImage = nullptr;
    m_mirrorStat = nullptr;
    m_mirrorArea = nullptr;
    auto column = skyColumn(renderer, width, height);
    syncLiveTick();
    return column;
  }

  std::unique_ptr<Node> LinkApp::skyColumn(Renderer& renderer, float width, float height) {
    const float s = m_scale = m_window->uiScale();
    if (m_sky == nullptr) m_sky = std::make_unique<Sky>(*m_window);
    m_sky->setLife(skyLife());
    if (m_client == nullptr) {
      m_sky->hide();
      return pageBody(renderer, width, height);
    }
    const Presence presence = m_client->presence();
    if (m_page == Page::Home && !pairShown()) return home(renderer, width, height);
    if (presence.kind == Presence::Kind::NotPaired || pairShown()) {
      // pages.html PAGE pair: a young galaxy, seen from above.
      SkyLook look = skyLook(SkyMode::Full);
      look.forming = true;
      look.dim = 0.55F;
      look.camera = GalaxyCamera{.yaw = 0.9F, .pitch = 0.7F, .dist = 3.4F, .sx = 0.0F, .sy = 0.0F};
      return skyWithCall(renderer, look, width, height, pair(renderer, width, height), {});
    }
    if (m_page == Page::Mirror && (presence.kind == Presence::Kind::Connected || presence.kind == Presence::Kind::Locked)) {
      m_renderer = &renderer;
      // pages.html: the core behind the phone's screen.
      SkyLook look = skyLook(SkyMode::Full);
      look.camera = GalaxyCamera{.yaw = 0.55F, .pitch = 0.5F, .dist = 2.6F, .sx = Tokens::componentLinkMirrorCentre * 2.0F - 1.0F, .sy = 0.05F};
      if (m_client->mirror().kind != Mirror::Kind::Showing) look.dim = std::min(look.dim, 0.3F); // a message over the core, as on Away
      return skyWithCall(renderer, look, width, height, mirror(width, height), {});
    }
    const float band = Tokens::componentLinkBand * s;
    const float overlap = Tokens::componentLinkBandOverlap * s;
    const float margin = Tokens::spaceP12 * s;
    auto column = ui::column({.align = FlexAlign::Stretch, .fill = skyFill(), .width = width, .height = height});
    std::vector<SkyAnchor> anchors;
    auto front = bandFront(width, band, anchors);
    column->addChild(skyWithCall(renderer, skyLook(SkyMode::Band), width, band, std::move(front), std::move(anchors)));
    const float sheetHeight = height - band + overlap;
    const float padH = Tokens::spaceP24 * s;
    const float padTop = Tokens::spaceP20 * s;
    const float bodyWidth = std::max(0.0F, width - 2.0F * margin - 2.0F * padH);
    const float bodyHeight = std::max(0.0F, sheetHeight - padTop - padH);
    auto sheet = sheetPlane(width - 2.0F * margin, sheetHeight, false);
    if (splitPage()) {
      sheet->addChild(pageBody(renderer, width - 2.0F * margin, sheetHeight));
    } else {
      sheet->setPadding(padTop, padH, padH, padH);
      sheet->addChild(pageBody(renderer, bodyWidth, bodyHeight));
    }
    m_sky->setFollower(column->addChild(sheetHolder(std::move(sheet), width, sheetHeight, overlap, height, s)));
    return column;
  }

  ColorSpec LinkApp::skyFill() const {
    if (!isResolvedLightTheme()) return fixedColorSpec(rgbaHex(Tokens::componentLinkSky));
    const auto bottom = GalaxyLightSky{}.bottom;
    return fixedColorSpec(rgba(bottom[0], bottom[1], bottom[2]));
  }

  std::unique_ptr<Flex> LinkApp::sheetPlane(float width, float height, bool grab) {
    const float s = m_scale;
    const float r = Style::scaledRadius(Tokens::radiusR28, s);
    auto sheet = ui::column({.align = FlexAlign::Stretch, .fill = colorSpecFromRole(ColorRole::SolidRegular), .clipChildren = true,
                             .width = width, .height = height});
    sheet->setRadii(Radii{r, r, 0.0F, 0.0F});
    sheet->setClipRadii(Radii{r, r, 0.0F, 0.0F});
    if (grab) {
      auto mark = ui::box({.fill = colorSpecFromRole(ColorRole::Fill3), .radius = 2.0F * s, .width = 36.0F * s, .height = 4.0F * s});
      mark->setParticipatesInLayout(false);
      mark->setPosition((width - 36.0F * s) / 2.0F, Tokens::spaceP8 * s);
      sheet->addChild(std::move(mark));
    }
    return sheet;
  }

  // The band of a page: its title and the phone, then a chip per running item, each tied to its star.
  // PhoneMenuButton (pages.html .bt .phn): the phone's dot, its name with a chevron that opens the phone menu, then
  // `stat`.
  std::unique_ptr<Flex> LinkApp::phoneLine(std::unique_ptr<Node> stat) {
    const float s = m_scale;
    const Phone& phone = m_client->presence().phone;
    auto line = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    line->setPadding(Tokens::spaceP4 * s, 0.0F, 0.0F, 0.0F);
    const ColorSpec green = fixedColorSpec(rgbaHex(Tokens::componentLinkConnected));
    line->addChild(dot(phone.connected ? (phone.locked == 1 ? core() : green) : colorSpecFromRole(ColorRole::Label2), !phone.connected,
                       6.0F * s, s));
    const float height = 26.0F * s;
    // A context menu centres under its point: under the button's middle.
    auto at = std::make_shared<Flex*>(nullptr);
    auto menu = ui::row({.out = at.get(), .align = FlexAlign::Center, .gap = Tokens::spaceP4 * s, .paddingH = Tokens::spaceP8 * s,
                         .radius = height / 2.0F, .minHeight = height,
                         .onClick = [this, at] {
                           float x = 0.0F;
                           float y = 0.0F;
                           Node::absolutePosition(*at, x, y);
                           phoneMenu(x + (*at)->width() / 2.0F, y + (*at)->height());
                         },
                         .hoverFill = colorSpecFromRole(ColorRole::Fill2)});
    auto name = text(phone.name, Tokens::typeBody, ColorRole::Label1, s);
    name->setFontWeight(FontWeight::SemiBold);
    name->setMaxWidth(260.0F * s);
    menu->addChild(std::move(name));
    menu->addChild(ui::glyph({.glyph = "chevron-down", .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label2)}));
    line->addChild(std::move(menu));
    if (stat) line->addChild(std::move(stat));
    return line;
  }

  std::unique_ptr<Node> LinkApp::bandFront(float width, float height, std::vector<SkyAnchor>& anchors) {
    const float s = m_scale;
    auto front = ui::row({.align = FlexAlign::Start, .gap = Tokens::spaceP16 * s, .width = width, .height = height});
    front->setPadding(30.0F * s, 20.0F * s, 40.0F * s, 32.0F * s);
    auto title = ui::column({.align = FlexAlign::Start, .gap = Tokens::spaceP4 * s, .flexGrow = 1.0F});
    title->addChild(text(pageTitle(m_page), Tokens::typeLarge, ColorRole::Label1, s));
    const Presence presence = m_client->presence();
    const Phone& phone = presence.phone;
    std::string line;
    if (presence.kind == Presence::Kind::Away) {
      line = i18n::tr("link.home.away", "seen", lastSeenText(phone.lastSeen, nowMs() / 1000));
    } else if (!phone.transport.empty()) {
      line = transportName(phone.transport);
      if (phone.battery >= 0) line += std::format(" · {}%", phone.battery) + (phone.charging ? i18n::tr("link.home.charging") : std::string());
    }
    auto stat = text(std::move(line), Tokens::typeBody, ColorRole::Label2, s);
    title->addChild(phoneLine(std::move(stat)));
    front->addChild(std::move(title));
    if (presence.kind != Presence::Kind::Connected && presence.kind != Presence::Kind::Locked) return front;

    auto chips = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    chips->setPadding(14.0F * s, 0.0F, 0.0F, 0.0F);
    // The mockup's breakpoint is the window's width: narrower, a chip keeps only its glyph and its action.
    const bool labels = width + Tokens::componentSidebarWidth * s >= kBandChipLabels * s;
    const float h = Tokens::componentLinkBandChip * s;
    const auto chip = [&](std::unique_ptr<Node> glyph, std::string label, std::string sub, std::unique_ptr<Node> action,
                          const std::array<float, 3>& star) {
      auto c = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::SolidRegular),
                        .radius = h / 2.0F, .border = colorSpecFromRole(ColorRole::Separator), .borderWidth = 1.0F, .height = h});
      c->setPadding(0.0F, 6.0F * s, 0.0F, 6.0F * s);
      c->setElevation(&Tokens::elevationE3, s);
      c->addChild(std::move(glyph));
      if (labels) {
        auto name = text(std::move(label), Tokens::typeBody, ColorRole::Label1, s);
        name->setFontWeight(FontWeight::SemiBold);
        name->setMaxWidth(kBandChipLabel * s);
        c->addChild(std::move(name));
        if (!sub.empty()) {
          auto small = text(std::move(sub), Tokens::typeCaption, ColorRole::Label2, s);
          small->setMaxWidth(kBandChipLabel * s);
          c->addChild(std::move(small));
        }
      }
      c->addChild(std::move(action));
      anchors.push_back({.card = c.get(), .star = star});
      chips->addChild(std::move(c));
    };
    const std::string pid = phone.id;
    const float art = 32.0F * s;
    if (const PhonePlayer* p = player(); p != nullptr) {
      auto cover = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                            .radius = art / 2.0F, .width = art, .height = art});
      cover->addChild(ui::glyph({.glyph = std::string("music"), .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label2)}));
      chip(std::move(cover), p->title, {},
           roundKey(p->playing ? "player-pause" : "player-play", true, art, s, [this, pid] { m_client->playerCommand(pid, "play-pause"); }),
           kStarMedia);
    }
    if (phone.camera == "on") {
      chip(plate("camera", art, "on", s), i18n::tr("link.home.webcam"), {},
           capsule(i18n::tr("link.home.stop"), {}, false, Tokens::sizeControlM * s, s, [this, pid] { stopStream(pid, "camera"); }), kStarCamera);
    }
    const auto& items = m_client->activity().items();
    if (const auto t = std::ranges::find_if(items, [&](const TransferEntry& e) { return e.state == Transfer::Running; });
        t != items.end()) {
      auto cancel = ui::button({.glyph = std::string("x"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = 28.0F * s, .variant = ButtonVariant::Ghost,
                                .radius = 14.0F * s, .width = 28.0F * s, .onClick = [this, id = t->id] { m_client->cancelTransfer(id); }});
      chip(plate(t->incoming ? "file-download" : "file-upload", art, "on", s), std::format("{}%", t->total > 0 ? t->bytes * 100 / t->total : 0),
           transferName(*t), std::move(cancel), kStarTransfer);
    }
    front->addChild(std::move(chips));
    return front;
  }

  std::unique_ptr<Node> LinkApp::home(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const Presence presence = m_client->presence();
    // concept-g.html empty: the Full sky with a title, a line and, with no phone, Pair a phone.
    if (presence.kind == Presence::Kind::NoDaemon || presence.kind == Presence::Kind::NotPaired) {
      const bool unpaired = presence.kind == Presence::Kind::NotPaired;
      auto front = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP12 * s,
                               .width = width, .height = height});
      front->addChild(text(i18n::tr(unpaired ? "link.home.unpaired-title" : "link.home.nodaemon-title"), Tokens::typeDisplay, ColorRole::Label1, s));
      auto body = text(i18n::tr(unpaired ? "link.home.unpaired-body" : "link.home.nodaemon-body"), Tokens::typeTitle3, ColorRole::Label2, s, 3);
      body->setMaxWidth(std::min(520.0F * s, width * 0.8F));
      body->setTextAlign(TextAlign::Center);
      front->addChild(std::move(body));
      if (unpaired) {
        auto actions = ui::row({});
        actions->setPadding(Tokens::spaceP12 * s, 0.0F, 0.0F, 0.0F);
        actions->addChild(pill(i18n::tr("link.home.pair"), std::nullopt, true, s, [this] { m_client->startPairing(); }, Tokens::sizeControlL));
        front->addChild(std::move(actions));
      }
      return skyWithCall(renderer, skyLook(SkyMode::Full), width, height, std::move(front), {});
    }
    const float overlap = Tokens::componentLinkSheetOverlap * s;
    const float hero = std::max(Tokens::componentLinkHeroMin * s, Tokens::componentLinkHeroShare * height);
    const float margin = Tokens::spaceP12 * s;
    float sheetHeight = std::max(Tokens::componentLinkSheetMin * s, Tokens::componentLinkSheetShare * height + overlap);
    auto sheet = homeSheet(renderer, presence, width - 2.0F * margin, sheetHeight);
    sheetHeight = sheet->height();
    const float total = hero + sheetHeight - overlap;

    std::vector<SkyAnchor> anchors;
    auto front = ui::row({.align = FlexAlign::Start, .gap = Tokens::spaceP24 * s, .width = width, .height = hero});
    front->setPadding(36.0F * s, 24.0F * s, 52.0F * s, 32.0F * s);
    front->addChild(homeIdentity(presence, s));
    const float liveWidth = std::min(Tokens::componentLinkLiveWidth * s, Tokens::componentLinkLiveShare * width);
    front->addChild(homeLive(renderer, presence, liveWidth, s, anchors));

    auto page = ui::column({.align = FlexAlign::Stretch, .fill = skyFill(), .width = width, .height = total});
    page->addChild(skyWithCall(renderer, skyLook(SkyMode::Hero), width, hero, std::move(front), std::move(anchors)));
    m_sky->setFollower(page->addChild(sheetHolder(std::move(sheet), width, sheetHeight, overlap, height, s)));
    if (total <= height) return page;
    auto scroll = ui::scrollView({.state = &m_homeScroll, .contentScale = s, .scrollbarAutoHide = true,
                                  .contentAlign = ScrollView::ContentAlign::Start, .viewportPaddingV = 0.0F, .flexGrow = 1.0F});
    scroll->content()->addChild(std::move(page));
    return scroll;
  }

  std::unique_ptr<Node> LinkApp::homeIdentity(const Presence& presence, float s) {
    const Phone& phone = presence.phone;
    const bool away = presence.kind == Presence::Kind::Away;
    auto id = ui::column({.align = FlexAlign::Start, .flexGrow = 1.0F});

    auto tabs = ui::row({.align = FlexAlign::Center, .wrap = true, .gap = Tokens::spaceP4 * s});
    for (const Phone& p : m_client->phones()) {
      const bool shown = p.id == phone.id;
      auto tab = ui::row({.align = FlexAlign::Center, .gap = 6.0F * s, .paddingH = Tokens::spaceP12 * s,
                          .fill = shown ? colorSpecFromRole(ColorRole::Fill2) : clearColorSpec(), .radius = 14.0F * s,
                          .height = 28.0F * s, .onClick = [this, pid = p.id] { choosePhone(pid); },
                          .hoverFill = colorSpecFromRole(ColorRole::Fill1)});
      const ColorSpec green = fixedColorSpec(rgbaHex(Tokens::componentLinkConnected));
      tab->addChild(dot(p.connected ? (p.locked == 1 ? core() : green) : colorSpecFromRole(ColorRole::Label2), !p.connected, 6.0F * s, s));
      auto name = text(p.name, Tokens::typeFootnote, shown ? ColorRole::Label1 : ColorRole::Label2, s);
      if (shown) name->setFontWeight(FontWeight::SemiBold);
      name->setMaxWidth(150.0F * s);
      tab->addChild(std::move(name));
      tabs->addChild(std::move(tab));
    }
    tabs->addChild(ui::button({.glyph = std::string("plus"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = 28.0F * s, .variant = ButtonVariant::Ghost,
                               .tooltip = i18n::tr("link.phone-card.pair-another"), .radius = 14.0F * s, .width = 28.0F * s,
                               .onClick = [this] { m_client->startPairing(); }}));
    id->addChild(std::move(tabs));
    id->addChild(ui::box({.height = 14.0F * s}));

    auto name = ui::label({.text = phone.name, .fontSize = Tokens::componentLinkName * s, .fontWeight = FontWeight::SemiBold,
                           .color = colorSpecFromRole(ColorRole::Label1), .maxLines = 2, .ellipsize = TextEllipsize::End});
    name->setLineHeight(Tokens::componentLinkNameLine * s);
    id->addChild(std::move(name));

    // The status lines take title3's size and line at weight 400 (handoff "via", "facts").
    const auto line = [s](std::string value, ColorRole role) {
      auto label = text(std::move(value), Tokens::typeTitle3, role, s);
      label->setFontWeight(FontWeight::Normal);
      return label;
    };
    auto via = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    via->setPadding(Tokens::spaceP8 * s, 0.0F, 0.0F, 0.0F);
    const ColorSpec green = fixedColorSpec(rgbaHex(Tokens::componentLinkConnected));
    via->addChild(dot(away ? colorSpecFromRole(ColorRole::Label2) : green, away, 8.0F * s, s));
    if (away) {
      via->addChild(line(i18n::tr("link.home.away", "seen", lastSeenText(phone.lastSeen, nowMs() / 1000)), ColorRole::Label1));
      via->addChild(capsule(i18n::tr("link.away.connect"), {}, false, 26.0F * s, s, [this, pid = phone.id] { m_client->connect(pid); }));
    } else {
      via->addChild(line(i18n::tr("link.home.connected", "transport", transportName(phone.transport)), ColorRole::Label1));
    }
    id->addChild(std::move(via));

    std::string facts;
    if (away) {
      if (phone.battery >= 0) facts = i18n::tr("link.home.last-known", "battery", phone.battery);
    } else {
      facts = i18n::tr(phone.locked == 1 ? "link.home.locked" : "link.home.unlocked");
      if (phone.battery >= 0) facts += std::format(" · {}%", phone.battery) + (phone.charging ? i18n::tr("link.home.charging") : std::string());
      if (phone.signal) facts += " · " + (*phone.signal < 0 ? i18n::tr("link.home.no-service") : i18n::tr("link.home.signal", "bars", *phone.signal));
    }
    if (phone.storageFree) facts += (facts.empty() ? "" : " · ") + i18n::tr("link.home.storage-free", "size", byteSize(*phone.storageFree));
    if (!facts.empty()) {
      auto row = ui::row({.align = FlexAlign::Center});
      row->setPadding(2.0F * s, 0.0F, 0.0F, 0.0F);
      row->addChild(line(facts, ColorRole::Label2));
      id->addChild(std::move(row));
    }

    // Phone switches; the transport Link uses is locked on.
    // Fill width: a wrapping row breaks lines only against a known width (concept-g .sw wraps under the name).
    auto sw = ui::row({.align = FlexAlign::Center, .wrap = true, .gap = Tokens::spaceP8 * s, .fillWidth = true, .opacity = away ? 0.4F : 1.0F});
    sw->setPadding(22.0F * s, 0.0F, 0.0F, 0.0F);
    const float chip = Tokens::sizeControlL * s;
    const std::string pid = phone.id;
    // set-ringer values by segment: normal 2, vibrate 1, silent 0.
    constexpr std::array<std::string_view, 3> kRingers{"normal", "vibrate", "silent"};
    if (const auto at = std::ranges::find(kRingers, phone.ringer); at != kRingers.end()) {
      sw->addChild(ui::segmented({.options = std::vector<ui::SegmentedOption>{{.label = i18n::tr("link.home.ring"), .glyph = "bell"},
                                                                              {.label = i18n::tr("link.home.vibrate"), .glyph = "device-mobile-vibration"},
                                                                              {.label = i18n::tr("link.home.silent"), .glyph = "bell-off"}},
                                  .selectedIndex = static_cast<std::size_t>(at - kRingers.begin()), .scale = s, .enabled = !away,
                                  .surfaceRole = ColorRole::Card, .height = chip,
                                  .onChange = [this, pid](std::size_t i) { m_client->sendSystemCommand(pid, "set-ringer", static_cast<std::uint32_t>(2 - i)); },
                                  .configure = [](Segmented& seg) {
                                    seg.setThumbRoles(ColorRole::Label1, ColorRole::View);
                                    seg.setSurfaceStroke(ColorRole::Separator);
                                  }}));
    }
    if (phone.dnd >= 0) {
      sw->addChild(capsule(i18n::tr("link.home.dnd"), "moon", phone.dnd == 1, chip, s,
                           [this, pid, on = phone.dnd == 1] { m_client->setDnd(pid, !on); }, !away));
    }
    const auto system = [&](int state, const char* label, const char* glyph, const char* command, std::string_view transport) {
      if (state < 0) return;
      const bool lock = phone.transport == transport;
      auto button = capsule(i18n::tr(label), glyph, state == 1 || lock, chip, s,
                            [this, pid, command, on = state == 1] { m_client->sendSystemCommand(pid, command, on ? 0U : 1U); },
                            !away && !lock);
      if (lock) {
        button->setOpacity(0.78F);
        button->setTooltip(i18n::tr("link.home.lock-tip", "transport", i18n::tr(label)));
      }
      sw->addChild(std::move(button));
    };
    system(phone.wifi, "link.home.wifi", "wifi", "set-wifi", "wifi");
    system(phone.bluetooth, "link.home.bluetooth", "bluetooth", "set-bluetooth", "bluetooth");
    system(phone.torch, "link.home.torch", "bulb", "set-torch", "");
    sw->addChild(capsule(i18n::tr("link.home.find"), "bell-ringing", false, chip, s, [this, pid] { m_client->ring(pid); }, !away));
    id->addChild(std::move(sw));
    return id;
  }

  std::unique_ptr<Node> LinkApp::homeLive(Renderer& renderer, const Presence& presence, float width, float s, std::vector<SkyAnchor>& anchors) {
    auto live = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP8 * s, .width = width});
    live->setPadding(20.0F * s, 0.0F, 0.0F, 0.0F);
    if (presence.kind != Presence::Kind::Connected && presence.kind != Presence::Kind::Locked) return live;
    const Phone& phone = presence.phone;
    const std::string pid = phone.id;
    const PhonePlayer* p = player();
    constexpr std::array stars{kStarCamera, kStarMic, kStarSound}; // by kStreams
    const bool anyStream = std::ranges::any_of(kStreams, [&](const StreamKind& k) { return streamLive(phone.*k.state); });
    if (p == nullptr && !anyStream) return live;
    auto head = text(i18n::tr("link.home.running"), Tokens::typeFootnote, ColorRole::Label2, s);
    head->setFontWeight(FontWeight::SemiBold);
    auto headRow = ui::row({.align = FlexAlign::Center});
    headRow->setPadding(0.0F, 0.0F, 0.0F, Tokens::spaceP4 * s);
    headRow->addChild(std::move(head));
    live->addChild(std::move(headRow));
    const float radius = Style::scaledRadius(Tokens::radiusR16, s);
    const auto card = [&] {
      return ui::column({.align = FlexAlign::Stretch, .fill = colorSpecFromRole(ColorRole::SolidRegular), .radius = radius,
                         .border = colorSpecFromRole(ColorRole::Separator), .borderWidth = 1.0F,
                         .configure = [s](Flex& f) { f.setElevation(&Tokens::elevationE3, s); }});
    };

    if (p != nullptr) {
      auto media = card();
      media->setPadding(Tokens::spaceP8 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s, 10.0F * s);
      auto top = ui::row({.align = FlexAlign::Center, .gap = 10.0F * s});
      auto art = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                          .radius = Tokens::spaceP8 * s, .width = Tokens::componentLinkLiveArt * s, .height = Tokens::componentLinkLiveArt * s});
      art->addChild(ui::glyph({.glyph = std::string("music"), .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label2)}));
      if (!p->art.empty()) {
        // Over the glyph tile: a missing or unreadable file leaves the glyph.
        Image* cover = nullptr;
        art->addChild(ui::image({.out = &cover, .fit = ImageFit::Cover, .radius = Tokens::spaceP8 * s, .width = Tokens::componentLinkLiveArt * s,
                                 .height = Tokens::componentLinkLiveArt * s,
                                 .participatesInLayout = false}));
        (void)cover->setSourceFileAsync(renderer, m_viewCache, p->art, static_cast<int>(std::ceil(Tokens::componentLinkLiveArt * s)));
      }
      top->addChild(std::move(art));
      top->addChild(twoLines(p->title, p->artist.empty() ? p->app : p->artist + " · " + p->app, ColorRole::Label2, s));
      const float key = Tokens::sizeControlM * s;
      if (p->takes("previous")) top->addChild(roundKey("player-skip-back", false, key, s, [this, pid] { m_client->playerCommand(pid, "previous"); }));
      top->addChild(roundKey(p->playing ? "player-pause" : "player-play", true, key, s, [this, pid] { m_client->playerCommand(pid, "play-pause"); }));
      if (p->takes("next")) top->addChild(roundKey("player-skip-forward", false, key, s, [this, pid] { m_client->playerCommand(pid, "next"); }));
      media->addChild(std::move(top));
      if (p->lengthMs > 0) {
        auto seek = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
        seek->setPadding(Tokens::spaceP8 * s, 0.0F, 0.0F, 0.0F);
        const std::int64_t at = p->position(nowMs());
        auto position = text(clockOf(at), Tokens::typeCaption, ColorRole::Label2, s);
        m_liveView.position = position.get();
        seek->addChild(std::move(position));
        auto bar = ui::progressBar({.fill = core(), .track = colorSpecFromRole(ColorRole::Fill2),
                                    .radius = 2.0F * s, .progress = static_cast<float>(at) / static_cast<float>(p->lengthMs),
                                    .height = 4.0F * s, .flexGrow = 1.0F});
        m_liveView.seek = bar.get();
        seek->addChild(std::move(bar));
        seek->addChild(text(clockOf(p->lengthMs), Tokens::typeCaption, ColorRole::Label2, s));
        media->addChild(std::move(seek));
      }
      if (phone.mediaVolume >= 0) {
        auto volume = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
        volume->setPadding(6.0F * s, 0.0F, 0.0F, 0.0F);
        volume->addChild(ui::glyph({.glyph = std::string("volume"), .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label2)}));
        volume->addChild(ui::slider({.minValue = 0.0, .maxValue = 100.0, .step = 1.0, .value = static_cast<double>(phone.mediaVolume),
                                     .controlHeight = 6.0F * s, .flexGrow = 1.0F,
                                     .onValueChanged = [this, pid](double v) { m_client->setMediaVolume(pid, static_cast<int>(v)); },
                                     .configure = [](Slider& slider) {
                                       slider.setFilled(true);
                                       slider.setColors(ColorRole::Label1, ColorRole::Fill2);
                                     }}));
        volume->addChild(text(std::to_string(phone.mediaVolume), Tokens::typeFootnote, ColorRole::Label2, s));
        media->addChild(std::move(volume));
      }
      anchors.push_back({.card = media.get(), .star = kStarMedia});
      live->addChild(std::move(media));
    }

    if (anyStream) {
      auto group = card();
      group->setPadding(2.0F * s, 6.0F * s, 2.0F * s, 6.0F * s);
      for (std::size_t k = 0; k < kStreams.size(); ++k) {
        const auto& [glyph, name, on, field] = kStreams[k];
        const std::string* const state = &(phone.*field);
        const auto star = stars[k];
        if (!streamLive(*state)) continue;
        const bool asking = *state == "asking";
        auto row = ui::row({.align = FlexAlign::Center, .gap = 10.0F * s, .minHeight = 44.0F * s});
        row->addChild(plate(glyph, 28.0F * s, *state, s));
        row->addChild(twoLines(i18n::tr(name), i18n::tr(asking ? "link.home.asking" : on), ColorRole::Label2, s));
        const std::string_view g = glyph;
        if (!asking && g == "camera") {
          row->addChild(roundKey("camera-rotate", false, Tokens::sizeControlM * s, s, [this, pid] { m_client->switchPhoneCamera(pid); }));
        }
        row->addChild(capsule(i18n::tr(asking ? "link.home.cancel" : "link.home.stop"), {}, false, Tokens::sizeControlM * s, s,
                              [this, pid, g] { stopStream(pid, g); }));
        anchors.push_back({.card = row.get(), .star = star});
        group->addChild(std::move(row));
      }
      live->addChild(std::move(group));
    }
    return live;
  }

  void LinkApp::stopStream(const std::string& deviceId, std::string_view glyph) {
    if (glyph == "camera") m_client->setPhoneCamera(deviceId, false);
    else m_client->setPhoneAudio(deviceId, glyph == "microphone" ? LinkClient::PhoneAudio::Mic : LinkClient::PhoneAudio::Sound, false);
  }

  void LinkApp::syncLiveTick() {
    const PhonePlayer* p = player();
    if (p != nullptr && p->playing && m_liveView.seek != nullptr) {
      if (!m_liveTick.active()) m_liveTick.startRepeating(m_window->shownTimers(), std::chrono::seconds(1), [this] { tickLive(); });
    } else {
      m_liveTick.stop();
    }
  }

  void LinkApp::tickLive() {
    const PhonePlayer* p = player();
    if (p == nullptr || m_liveView.seek == nullptr || p->lengthMs <= 0) return;
    const std::int64_t at = p->position(nowMs());
    m_liveView.seek->setProgress(static_cast<float>(at) / static_cast<float>(p->lengthMs));
    if (m_liveView.position != nullptr) m_liveView.position->setText(clockOf(at));
  }

  std::unique_ptr<Flex> LinkApp::homeSheet(Renderer& renderer, const Presence& presence, float width, float height) {
    const float s = m_scale;
    auto sheet = sheetPlane(width, height, true);
    // The mockup's breakpoint is the window's width.
    const bool wide = width + 2.0F * Tokens::spaceP12 * s + Tokens::componentSidebarWidth * s >= Tokens::componentLinkSheetWide * s;
    auto grid = ui::row({.align = FlexAlign::Start, .wrap = !wide, .gap = Tokens::spaceP24 * s, .flexGrow = 1.0F});
    grid->setPadding(Tokens::spaceP8 * s + Tokens::spaceP16 * s, 20.0F * s, 0.0F, 20.0F * s);
    const float inner = width - 40.0F * s;
    const float gap = Tokens::spaceP24 * s;
    const float avail = inner - (wide ? 2.0F : 1.0F) * gap;
    const std::array<float, 3> widths = wide ? std::array{avail * 0.95F / 3.2F, avail * 1.25F / 3.2F, avail * 1.0F / 3.2F}
                                             : std::array{avail / 2.0F, avail / 2.0F, inner};
    const auto column = [&](std::string title, float w, std::unique_ptr<Node> trailing = nullptr) {
      auto col = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP8 * s, .width = w});
      auto head = ui::row({.align = FlexAlign::Center, .minHeight = 24.0F * s});
      head->setPadding(0.0F, Tokens::spaceP4 * s, 0.0F, Tokens::spaceP4 * s);
      auto label = text(std::move(title), Tokens::typeBody, ColorRole::Label2, s);
      label->setFontWeight(FontWeight::SemiBold);
      label->setFlexGrow(1.0F);
      head->addChild(std::move(label));
      if (trailing) head->addChild(std::move(trailing));
      col->addChild(std::move(head));
      return col;
    };
    const float radius = Style::scaledRadius(Tokens::radiusR16, s);
    const bool live = presence.kind == Presence::Kind::Connected || presence.kind == Presence::Kind::Locked;
    const Phone& phone = presence.phone;
    const std::string pid = phone.id;

    // Use the phone here: one row per stream; a row never moves, its state changes in place.
    auto use = column(i18n::tr("link.home.use-here"), widths[0]);
    auto group = ui::column({.align = FlexAlign::Stretch, .padding = Tokens::spaceP4 * s, .fill = colorSpecFromRole(ColorRole::Card),
                             .radius = radius});
    const auto streamRow = [&](const char* glyph, const char* name, const char* ready, const char* on, const std::string& rawState,
                               std::function<void()> start, const char* verb = "link.home.start") {
      if (rawState.empty()) return;
      const std::string state = live ? rawState : std::string("off");
      auto row = ui::row({.align = FlexAlign::Center, .gap = 10.0F * s, .minHeight = 48.0F * s});
      row->setPadding(5.0F * s, Tokens::spaceP8 * s, 5.0F * s, Tokens::spaceP8 * s);
      row->addChild(plate(glyph, Tokens::sizeControlM * s, state, s));
      const bool asking = state == "asking";
      const bool running = state == "on";
      row->addChild(twoLines(i18n::tr(name), i18n::tr(asking ? "link.home.asking" : running ? on : ready),
                             running ? ColorRole::AccentText : ColorRole::Label2, s));
      const std::string_view g = glyph;
      if (running || asking) {
        row->addChild(capsule(i18n::tr(asking ? "link.home.cancel" : "link.home.stop"), {}, false, Tokens::sizeControlM * s, s,
                              [this, pid, g] { stopStream(pid, g); }));
      } else {
        auto action = capsule(i18n::tr(verb), {}, false, Tokens::sizeControlM * s, s, std::move(start), live);
        if (!live) action->setOpacity(0.4F);
        row->addChild(std::move(action));
      }
      group->addChild(std::move(row));
    };
    static const std::string kNone;
    streamRow("camera", "link.home.webcam", "link.home.webcam-ready", "link.home.webcam-on", hasLoopbackCamera() ? phone.camera : kNone,
              [this, pid] { m_client->setPhoneCamera(pid, true); });
    streamRow("microphone", "link.home.mic", "link.home.mic-ready", "link.home.mic-on", phone.mic,
              [this, pid] { m_client->setPhoneAudio(pid, LinkClient::PhoneAudio::Mic, true); });
    streamRow("volume", "link.home.sound", "link.home.sound-ready", "link.home.sound-on", phone.sound,
              [this, pid] { m_client->setPhoneAudio(pid, LinkClient::PhoneAudio::Sound, true); });
    static const std::string kReady = "ready";
    streamRow("device-mobile", "link.home.mirror", "link.home.mirror-ready", "link.home.mirror-ready", kReady,
              [this] { show(Page::Mirror); }, "link.home.open");
    use->addChild(std::move(group));
    grid->addChild(std::move(use));

    // Waiting: the newest items that fit; a card that would not fit whole is left out.
    const auto& items = m_client->feed().items();
    auto seeAll = ui::button({.text = i18n::tr("link.home.see-all"), .controlHeight = 24.0F * s, .variant = ButtonVariant::Ghost, .onClick = [this] { show(Page::Waiting); }});
    auto waiting = column(i18n::tr("link.home.waiting", "count", items.size()), widths[1], std::move(seeAll));
    // Wide, the sheet keeps its height and a card that would not fit whole is left out; narrow, the sheet grows.
    float room = height - grid->paddingTop() - waiting->measure(renderer, LayoutConstraints::available(widths[1], 1.0e6F)).height;
    const int shown = std::min(kHomeWaiting, static_cast<int>(items.size()));
    for (int i = 0; i < shown; ++i) {
      const FeedItem& item = items[static_cast<std::size_t>(i)];
      auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .fill = colorSpecFromRole(ColorRole::Card),
                          .radius = radius, .minHeight = 64.0F * s});
      row->setPadding(10.0F * s, 10.0F * s, 10.0F * s, Tokens::spaceP12 * s);
      const char* glyph = item.kind == FeedItem::Kind::Text         ? "message"
                          : item.kind == FeedItem::Kind::MissedCall ? "phone-x"
                          : item.kind == FeedItem::Kind::Offer      ? "file"
                          : item.kind == FeedItem::Kind::Photos     ? "photo"
                                                                    : "bell";
      row->addChild(plate(glyph, 36.0F * s, "", s));
      auto lines = ui::column({.align = FlexAlign::Stretch, .gap = 1.0F * s, .flexGrow = 1.0F});
      lines->addChild(text(item.app.empty() ? clock(item.atMs) : item.app + " · " + clock(item.atMs), Tokens::typeFootnote, ColorRole::Label2, s));
      lines->addChild(text(item.who, Tokens::typeHeadline, item.kind == FeedItem::Kind::MissedCall ? ColorRole::DangerText : ColorRole::Label1, s));
      if (!item.body.empty()) lines->addChild(text(item.body, Tokens::typeBody, ColorRole::Label2, s));
      row->addChild(std::move(lines));
      const auto action = std::ranges::find_if(item.actions, [](const FeedAction& a) { return !a.reply; });
      const auto reply = std::ranges::find_if(item.actions, [](const FeedAction& a) { return a.reply; });
      if (reply != item.actions.end()) {
        row->addChild(capsule(reply->label, {}, false, Tokens::sizeControlM * s, s, [this] { show(Page::Waiting); }));
      } else if (action != item.actions.end()) {
        row->addChild(capsule(action->label, {}, false, Tokens::sizeControlM * s, s,
                              [this, item, id = action->id] { m_client->action(item, id); }));
      }
      row->addChild(ui::button({.glyph = std::string("x"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = 28.0F * s, .variant = ButtonVariant::Ghost,
                                .tooltip = i18n::tr("link.home.dismiss-tip"), .radius = 14.0F * s, .width = 28.0F * s,
                                .onClick = [this, item] { m_client->dismiss(item); }}));
      room -= row->measure(renderer, LayoutConstraints::available(widths[1], 1.0e6F)).height + Tokens::spaceP8 * s;
      if (wide && room < 0.0F) break;
      waiting->addChild(std::move(row));
    }
    grid->addChild(std::move(waiting));

    // Send to the phone.
    auto send = column(i18n::tr("link.home.send"), widths[2]);
    auto tiles = ui::row({.align = FlexAlign::Stretch, .gap = 6.0F * s});
    const auto tile = [&](const char* glyph, const char* name, std::string sub, std::function<void()> click) {
      auto t = ui::column({.align = FlexAlign::Start, .gap = 10.0F * s, .padding = Tokens::spaceP12 * s,
                           .fill = colorSpecFromRole(ColorRole::Card), .radius = radius, .flexGrow = 1.0F,
                           .onClick = live ? std::move(click) : nullptr, .hoverFill = colorSpecFromRole(ColorRole::Fill2)});
      t->addChild(plate(glyph, Tokens::sizeControlM * s, "", s));
      t->addChild(twoLines(i18n::tr(name), std::move(sub), ColorRole::Label2, s));
      tiles->addChild(std::move(t));
    };
    tile("clipboard", "link.home.clipboard", i18n::tr("link.home.clipboard-sub"), [this, pid] {
      if (TextClipboard* clipboard = Input::textClipboard(); clipboard != nullptr) {
        clipboard->requestClipboardText([this, pid](std::optional<std::string> value) {
          if (value.has_value() && !value->empty()) m_client->shareText(pid, *value);
        });
      }
    });
    tile("file-upload", "link.home.files", i18n::tr("link.home.files-sub"), [this, pid] { chooseAndSend(pid); });
    send->addChild(std::move(tiles));
    grid->addChild(std::move(send));
    sheet->addChild(std::move(grid));
    if (!wide) {
      sheet->setSize(width, std::max(height, sheet->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height + 20.0F * s));
    }
    return sheet;
  }

} // namespace linkapp
