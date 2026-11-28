#pragma once

#include "design_tokens.h"
#include "ui/builders.h"
#include "ui/controls/button.h"
#include "ui/controls/label.h"
#include "ui/palette.h"
#include "ui/style.h"
#include "ui/controls/flex.h"
#include "render/scene/input_area.h"

#include <chrono>
#include <ctime>
#include <filesystem>
#include <system_error>
#include <format>
#include <functional>
#include <optional>
#include <cstdint>
#include <memory>
#include <string>

// Small builders the Link window's pages share.
namespace linkapp {

  inline std::int64_t nowMs() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::system_clock::now().time_since_epoch()).count();
  }

  // A v4l2loopback device exists (link-session's loopbacks() reads the same directory).
  inline bool hasLoopbackCamera() {
    std::error_code ec;
    return !std::filesystem::is_empty("/sys/devices/virtual/video4linux", ec) && !ec;
  }

  // "14:05": the local clock time of ms since the epoch.
  inline std::string clock(std::int64_t ms) {
    const std::time_t t = static_cast<std::time_t>(ms / 1000);
    std::tm tm{};
    localtime_r(&t, &tm);
    return std::format("{:02}:{:02}", tm.tm_hour, tm.tm_min);
  }

  // The filled neutral key (play, the primary pill): label_1 fill, view ink.
  inline Button::ButtonPalette playPalette() {
    const Button::ButtonStateColors rest{.bg = colorSpecFromRole(ColorRole::Label1), .border = clearColorSpec(),
                                         .label = colorSpecFromRole(ColorRole::View)};
    const Button::ButtonStateColors hot{.bg = colorSpecFromRole(ColorRole::Label2), .border = clearColorSpec(),
                                        .label = colorSpecFromRole(ColorRole::View)};
    return {.normal = rest, .hover = hot, .pressed = hot, .disabled = rest};
  }

  // The galaxy core gold: running things, progress, unread marks.
  inline ColorSpec core() { return fixedColorSpec(rgbaHex(Tokens::componentLinkCore)); }

  // A key in fill_2 (pages.html .pill, .ib), or filled neutral (playPalette) for the main act.
  inline Button::ButtonPalette keyPalette(bool filled) {
    if (filled) return playPalette();
    const Button::ButtonStateColors rest{.bg = colorSpecFromRole(ColorRole::Fill2), .border = clearColorSpec(),
                                         .label = colorSpecFromRole(ColorRole::Label1)};
    const Button::ButtonStateColors hot{.bg = colorSpecFromRole(ColorRole::Fill3), .border = clearColorSpec(),
                                        .label = colorSpecFromRole(ColorRole::Label1)};
    return {.normal = rest, .hover = hot, .pressed = hot, .disabled = rest};
  }

  // A fill_2 key with danger_text ink (pages.html .pill.danger).
  inline Button::ButtonPalette dangerPalette() {
    Button::ButtonPalette palette = keyPalette(false);
    for (auto* state : {&palette.normal, &palette.hover, &palette.pressed, &palette.disabled}) state->label = colorSpecFromRole(ColorRole::DangerText);
    return palette;
  }

  // Round glyph key (pages.html .ib, .send).
  inline std::unique_ptr<Button> iconKey(std::string glyph, std::string tooltip, bool filled, float size, float s,
                                         std::function<void()> click) {
    return ui::button({.glyph = std::move(glyph), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = size,
                       .customPalette = keyPalette(filled), .tooltip = std::move(tooltip), .radius = size / 2.0F, .width = size,
                       .onClick = std::move(click)});
  }

  // Pill (pages.html .pill, padding snapped): a 32 capsule in fill_2, or filled neutral for the main act.
  inline std::unique_ptr<Button> pill(std::string label, std::optional<std::string> glyph, bool filled, float s,
                                      std::function<void()> click) {
    const float height = Tokens::sizeControlM * s;
    return ui::button({.text = std::move(label), .glyph = std::move(glyph), .fontSize = Tokens::typeHeadline.size * s,
                       .glyphSize = Tokens::sizeSymbol * s, .controlHeight = height, .customPalette = keyPalette(filled),
                       .paddingH = Tokens::spaceP12 * s, .gap = Tokens::spaceP8 * s, .radius = height / 2.0F, .onClick = std::move(click)});
  }

  inline std::unique_ptr<Label> text(std::string value, const Tokens::Type& type, ColorRole role, float s, int lines = 1) {
    auto label = ui::label({.text = std::move(value), .fontSize = type.size * s,
                            .fontWeight = static_cast<FontWeight>(type.weight), .color = colorSpecFromRole(role),
                            .maxLines = lines, .ellipsize = TextEllipsize::End});
    label->setLineHeight(type.line * s);
    return label;
  }

  // Rulebook §9.13: symbol 48 label_3, title_3, paragraph label_2, at most one button.
  inline std::unique_ptr<Node> emptyState(const std::string& glyph, const std::string& title, const std::string& body, float s,
                                   std::unique_ptr<Node> action = nullptr) {
    auto box = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP8 * s,
                           .flexGrow = 1.0F});
    box->addChild(ui::glyph({.glyph = glyph, .glyphSize = Tokens::componentEmptySymbol * s,
                             .color = colorSpecFromRole(ColorRole::Label3)}));
    box->addChild(text(title, Tokens::typeTitle3, ColorRole::Label1, s));
    auto paragraph = text(body, Tokens::typeParagraph, ColorRole::Label2, s, 3);
    paragraph->setMaxWidth(Tokens::componentEmptyMaxWidth * s);
    paragraph->setTextAlign(TextAlign::Center);
    box->addChild(std::move(paragraph));
    if (action) box->addChild(std::move(action));
    return box;
  }

  // A section head over a card (pages.html .sh): footnote semibold in label_2.
  inline std::unique_ptr<Flex> sectionHead(std::string title, float s) {
    auto line = ui::row({.align = FlexAlign::Center});
    line->setPadding(Tokens::spaceP12 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s);
    auto label = text(std::move(title), Tokens::typeFootnote, ColorRole::Label2, s);
    label->setFontWeight(FontWeight::SemiBold);
    line->addChild(std::move(label));
    return line;
  }

  // A card of setting rows (pages.html .card): card fill, r16, inset 4.
  inline std::unique_ptr<Flex> rowCard(float s) {
    return ui::column({.align = FlexAlign::Stretch, .padding = Tokens::spaceP4 * s, .fill = colorSpecFromRole(ColorRole::Card),
                       .radius = Style::scaledRadius(Tokens::radiusR16, s)});
  }

  // A setting row (pages.html .sr): glyph in label_2 | label over its line | control; a line above every row but the
  // first. A click, when given, covers the row.
  inline void settingRow(Flex& card, const std::string& glyph, std::string label, std::string sub, std::unique_ptr<Node> control,
                         float s, std::function<void()> click = nullptr) {
    if (!card.children().empty()) card.addChild(ui::separator({}));
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .paddingH = Tokens::spaceP12 * s, .minHeight = Tokens::sizeControlXl * s});
    row->addChild(ui::glyph({.glyph = glyph, .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label2)}));
    auto lines = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP2 * s, .flexGrow = 1.0F});
    lines->addChild(text(std::move(label), Tokens::typeBody, ColorRole::Label1, s, 2));
    if (!sub.empty()) lines->addChild(text(std::move(sub), Tokens::typeFootnote, ColorRole::Label2, s, 2));
    row->addChild(std::move(lines));
    if (control) row->addChild(std::move(control));
    if (click) row->setCover(ui::inputArea({.onClick = [click = std::move(click)](const InputArea::PointerData&) { click(); }}));
    card.addChild(std::move(row));
  }

} // namespace linkapp
