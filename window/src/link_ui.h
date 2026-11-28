#pragma once

#include "design_tokens.h"
#include "ui/builders.h"
#include "ui/controls/button.h"
#include "ui/controls/label.h"
#include "ui/palette.h"

#include <chrono>
#include <ctime>
#include <filesystem>
#include <system_error>
#include <format>
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

  inline std::unique_ptr<Label> text(std::string value, const Tokens::Type& type, ColorRole role, float s, int lines = 1) {
    auto label = ui::label({.text = std::move(value), .fontSize = type.size * s,
                            .fontWeight = static_cast<FontWeight>(type.weight), .color = colorSpecFromRole(role),
                            .maxLines = lines, .ellipsize = TextEllipsize::End});
    label->setLineHeight(type.line * s);
    return label;
  }

} // namespace linkapp
