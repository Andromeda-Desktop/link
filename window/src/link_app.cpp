#include "link_app.h"
#include "link_ui.h"

#include "app/app_window.h"
#include "i18n/i18n.h"
#include "protocols/async_file_writer.h"
#include "protocols/uri.h"
#include "core/log.h"
#include "core/text_clipboard.h"
#include "core/input/input_codes.h"
#include "core/input/key_modifiers.h"
#include "ui/controls/alert.h"
#include "ui/controls/input.h"
#include "ui/controls/image.h"
#include "ui/builders.h"
#include "ui/controls/flex.h"
#include "ui/controls/selection_list.h"
#include "ui/controls/status_row.h"
#include "ui/palette.h"
#include "ui/style.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <ctime>
#include <filesystem>
#include <sstream>
#include <format>
#include <fstream>
#include <gio/gio.h>
#include <glib.h>
#include <poll.h>
#include <unistd.h>
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wshadow"
#pragma GCC diagnostic ignored "-Wsign-conversion"
// Parse errors come back as values.
#define TOML_HEADER_ONLY 1
#define TOML_EXCEPTIONS 0
#pragma GCC diagnostic pop
#include <xkbcommon/xkbcommon-keysyms.h>

namespace linkapp {

  namespace {

    struct PageInfo {
      Page page;
      const char* name;
      const char* title;
      const char* glyph;
    };

    constexpr std::int32_t kPairEntry = 1'000'000; // the phone menu's Pair entry; phone entries are their index
    constexpr std::array kPages{
        PageInfo{Page::Home, "Home", "link.pages.home", "home"},
        PageInfo{Page::Waiting, "Waiting", "link.pages.waiting", "bell"},
        PageInfo{Page::Messages, "Messages", "link.pages.messages", "message"},
        PageInfo{Page::Calls, "Calls", "link.pages.calls", "phone"},
        PageInfo{Page::Photos, "Photos", "link.pages.photos", "photo"},
        PageInfo{Page::Files, "Files", "link.pages.files", "folder"},
        PageInfo{Page::Activity, "Activity", "link.pages.activity", "arrows-down-up"},
        PageInfo{Page::Mirror, "Mirror", "link.pages.mirror", "device-mobile"},
        PageInfo{Page::Settings, "Settings", "link.pages.settings", "settings"},
    };

    // A list row's time: the clock today, else "Yesterday", a short weekday, or the date.
    std::string when(std::int64_t ms) {
      const std::int64_t now = nowMs();
      return daysAgo(ms, now) == 0 ? clock(ms) : dayLabel(ms, now, true);
    }

    std::string initials(const std::string& name) {
      std::string out;
      bool start = true;
      for (const char c : name) {
        if (c == ' ') {
          start = true;
        } else if (start && out.size() < 2 && std::isalpha(static_cast<unsigned char>(c)) != 0) {
          out += static_cast<char>(std::toupper(static_cast<unsigned char>(c)));
          start = false;
        } else {
          start = false;
        }
      }
      return out;
    }


    // Avatar (handoff.yaml): a 32 circle in fill_2 around initials or a symbol.
    std::unique_ptr<Flex> avatar(std::unique_ptr<Node> inner, float s) {
      const float size = Tokens::sizeControlM * s;
      auto circle = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                             .radius = size / 2.0F, .width = size, .height = size});
      circle->addChild(std::move(inner));
      return circle;
    }

    std::unique_ptr<Flex> initialsAvatar(const std::string& name, float s) {
      const std::string letters = initials(name);
      auto label = text(letters.empty() ? "#" : letters, Tokens::typeCaption, ColorRole::Label2, s);
      label->setFontWeight(FontWeight::SemiBold);
      return avatar(std::move(label), s);
    }

    std::unique_ptr<Flex> glyphAvatar(const char* glyph, float s, ColorRole ink = ColorRole::Label2) {
      return avatar(ui::glyph({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s,
                               .color = colorSpecFromRole(ink)}),
                    s);
    }

    // Reply bar (pages.html .reply): a 40 field in card with a stroke, then the filled send key.
    std::unique_ptr<Flex> replyBar(const std::string& value, const std::string& placeholder, float s,
                                   std::function<void(const std::string&)> change, std::function<void()> send) {
      const float height = Tokens::sizeControlL * s;
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
      bar->setPadding(Tokens::spaceP8 * s, Tokens::spaceP20 * s, Tokens::spaceP16 * s, Tokens::spaceP20 * s);
      auto field = ui::row({.align = FlexAlign::Center, .paddingH = Tokens::spaceP16 * s, .fill = colorSpecFromRole(ColorRole::Card),
                            .radius = height / 2.0F, .border = colorSpecFromRole(ColorRole::Separator), .borderWidth = s,
                            .height = height, .flexGrow = 1.0F});
      field->addChild(ui::input({.value = value, .placeholder = placeholder, .fontSize = Tokens::typeBody.size * s,
                                 .frameVisible = false, .flexGrow = 1.0F, .onChange = std::move(change),
                                 .onSubmit = [send](const std::string&) { send(); }}));
      bar->addChild(std::move(field));
      bar->addChild(iconKey("send", i18n::tr("link.common.send"), true, height, s, std::move(send)));
      return bar;
    }

    // The columns of a Files row and its header (pages.html .tbl): padding 12, gap 8.
    std::unique_ptr<Flex> fileColumns(float s, const ColorSpec& fill, float height) {
      return ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingH = Tokens::spaceP12 * s, .fill = fill,
                      .radius = Style::scaledRadius(Tokens::radiusR8, s), .height = height});
    }

    // A file's glyph by its extension.
    const char* fileGlyph(std::string_view name) {
      const auto dot = name.rfind('.');
      std::string ext(dot == std::string_view::npos ? std::string_view() : name.substr(dot + 1));
      std::ranges::transform(ext, ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
      if (ext == "jpg" || ext == "jpeg" || ext == "png" || ext == "heic" || ext == "webp" || ext == "gif") return "photo";
      if (ext == "mp4" || ext == "mkv" || ext == "webm" || ext == "mov" || ext == "3gp") return "video";
      if (ext == "mp3" || ext == "m4a" || ext == "ogg" || ext == "opus" || ext == "flac" || ext == "wav") return "music";
      if (ext == "zip" || ext == "7z" || ext == "rar" || ext == "tar" || ext == "gz") return "file-zip";
      if (ext == "txt" || ext == "pdf" || ext == "md" || ext == "doc" || ext == "docx" || ext == "odt") return "file-text";
      return "file";
    }

    std::uint64_t callKey(const CallEntry& c) { return std::hash<std::string>{}(std::format("{}|{}", c.number, c.atMs)); }

    // "Missed", "Outgoing · 4:02".
    std::string callKind(const CallEntry& c) {
      using K = CallEntry::Kind;
      const std::string kind = c.kind == K::Missed     ? i18n::tr("link.calls.missed")
                               : c.kind == K::Outgoing ? i18n::tr("link.calls.outgoing")
                                                       : i18n::tr("link.calls.incoming");
      return c.kind == K::Missed || c.seconds == 0 ? kind : std::format("{} · {}", kind, duration(c.seconds));
    }

    // A call in the .row form: direction glyph | title (danger_text when missed) and an optional sub line | time at the top.
    std::unique_ptr<Flex> callRow(const CallEntry& c, const std::string& title, const std::string& sub, const std::string& time,
                                  const ColorSpec& fill, Renderer& renderer, float width, float s) {
      using K = CallEntry::Kind;
      auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .padding = Tokens::spaceP8 * s, .fill = fill,
                          .radius = Style::scaledRadius(Tokens::radiusR12, s)});
      row->addChild(glyphAvatar(c.kind == K::Missed ? "phone-x" : c.kind == K::Outgoing ? "phone-outgoing" : "phone-incoming", s,
                                ColorRole::Label1));
      auto at = text(time, Tokens::typeFootnote, ColorRole::Label3, s);
      const float atWidth = static_cast<Node&>(*at).measure(renderer, LayoutConstraints::available(width, 1.0e6F)).width;
      const float bodyWidth = std::max(0.0F, width - 2.0F * Tokens::spaceP8 * s - Tokens::sizeControlM * s -
                                                 2.0F * Tokens::spaceP12 * s - atWidth);
      auto body = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      auto name = text(title, Tokens::typeHeadline, c.kind == K::Missed ? ColorRole::DangerText : ColorRole::Label1, s);
      name->setMaxWidth(bodyWidth);
      body->addChild(std::move(name));
      if (!sub.empty()) {
        auto line = text(sub, Tokens::typeFootnote, ColorRole::Label2, s);
        line->setMaxWidth(bodyWidth);
        body->addChild(std::move(line));
      }
      row->addChild(std::move(body));
      row->addChild(ui::column({.align = FlexAlign::End, .paddingV = Tokens::spaceP2 * s, .height = Tokens::sizeControlM * s},
                               std::move(at)));
      return row;
    }

    std::string lockedText() { return i18n::tr("link.feed.locked"); }

  } // namespace

  std::size_t FeedAdapter::itemCount() const {
    const std::vector<FeedItem>& items = m_client.feed().items();
    m_index.clear();
    for (std::size_t i = 0; i < items.size(); ++i) {
      if (sectionOf(items[i]) == m_section) m_index.push_back(i);
    }
    return m_index.size();
  }

  const FeedItem& FeedAdapter::item(std::size_t index) const { return m_client.feed().items()[m_index[index]]; }

  std::uint64_t FeedAdapter::itemKey(std::size_t index) const { return std::hash<std::string>{}(item(index).key); }

  float FeedAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(nullptr, index, width)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> FeedAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void FeedAdapter::bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool /*hovered*/) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(&renderer, index, width));
  }

  // Waiting row (pages.html .row, sizes snapped): avatar 32 | who headline + line footnote | time or actions; below,
  // from the text column: the reply field and quick replies, the photo strip, or the transfer bar.
  std::unique_ptr<Node> FeedAdapter::build(Renderer* renderer, std::size_t index, float width) {
    const float s = m_scale;
    const FeedItem& item = this->item(index);
    const bool locked = m_client.presence().kind == Presence::Kind::Locked;
    const bool offer = item.kind == FeedItem::Kind::Offer;
    const bool photos = item.kind == FeedItem::Kind::Photos;
    const bool missed = item.kind == FeedItem::Kind::MissedCall;
    const bool notification = item.kind == FeedItem::Kind::Notification;
    const float pad = Tokens::spaceP8 * s;
    const float gap = Tokens::spaceP12 * s;
    const float avatarSize = Tokens::sizeControlM * s;
    const float indent = avatarSize + gap;
    auto block = ui::column({.align = FlexAlign::Stretch});
    if (index > 0) block->addChild(ui::separator({}));
    auto cell = ui::column({.align = FlexAlign::Stretch, .gap = pad, .paddingV = pad, .paddingH = pad});

    auto row = ui::row({.align = FlexAlign::Center, .gap = gap});
    switch (item.kind) {
    case FeedItem::Kind::Text: row->addChild(initialsAvatar(m_client.messages().nameFor(item.who), s)); break;
    case FeedItem::Kind::Offer: row->addChild(glyphAvatar("file-text", s)); break;
    case FeedItem::Kind::MissedCall: row->addChild(glyphAvatar("phone-x", s)); break;
    case FeedItem::Kind::Notification: row->addChild(glyphAvatar("bell", s)); break;
    case FeedItem::Kind::Photos: row->addChild(glyphAvatar("photo", s)); break;
    }

    const std::string who = offer ? (item.files.size() == 1 ? item.files.front().first
                                                            : i18n::trp("link.common.files-count", static_cast<long>(item.files.size())))
                            : item.kind == FeedItem::Kind::Text ? m_client.messages().nameFor(item.who)
                            : item.app.empty()                  ? item.who
                                                                : item.app + " · " + item.who;
    const std::string line = offer    ? i18n::tr("link.feed.offered", "size", byteSize(item.total))
                             : missed ? i18n::tr("link.feed.missed", "time", clock(item.atMs))
                             : photos ? i18n::tr("link.feed.drag-hint")
                             : locked ? lockedText()
                                      : item.body;
    auto lines = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    lines->addChild(text(who, Tokens::typeHeadline, missed ? ColorRole::DangerText : ColorRole::Label1, s));
    lines->addChild(text(line, Tokens::typeFootnote, ColorRole::Label2, s));
    row->addChild(std::move(lines));

    // The time sits at the top of the row (pages.html .tm align-self start).
    const auto time = [&](std::string value) {
      auto top = ui::column({.align = FlexAlign::End, .height = avatarSize});
      top->addChild(text(std::move(value), Tokens::typeFootnote, ColorRole::Label2, s));
      return top;
    };
    const FeedItem copy = item;
    if (offer && item.transfer == Transfer::Offered) {
      row->addChild(pill(i18n::tr("link.common.decline"), std::nullopt, false, s, [this, copy] { m_client.decline(copy); }));
      row->addChild(pill(i18n::tr("link.common.save"), std::nullopt, true, s, [this, copy] { m_client.accept(copy); }));
    } else if (missed) {
      for (const FeedAction& action : item.actions) {
        if (action.reply) continue;
        if (m_hooks.textTo) {
          row->addChild(pill(i18n::tr("link.feed.text"), std::string("message"), false, s,
                             [this, number = action.id] { m_hooks.textTo(number); }));
        }
        row->addChild(pill(action.label, std::string("phone"), false, s, [this, copy, id = action.id] { m_client.action(copy, id); }));
      }
    } else {
      for (const FeedAction& action : item.actions) {
        if (action.reply || !notification) continue;
        row->addChild(pill(action.label, std::nullopt, false, s, [this, copy, id = action.id] { m_client.action(copy, id); }));
      }
      row->addChild(time(offer ? byteSize(item.total) : photos ? i18n::tr("link.day.today") : clock(item.atMs)));
      if (notification) {
        row->addChild(ui::button({.glyph = std::string("x"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = 28.0F * s,
                                  .variant = ButtonVariant::Ghost, .tooltip = i18n::tr("link.common.dismiss"), .radius = 14.0F * s,
                                  .width = 28.0F * s, .onClick = [this, copy] { m_client.dismiss(copy); }}));
      }
    }
    cell->addChild(std::move(row));

    auto below = ui::column({.align = FlexAlign::Stretch});
    below->setPadding(0.0F, 0.0F, 0.0F, indent);
    const bool canReply = item.kind == FeedItem::Kind::Text || std::ranges::any_of(item.actions, &FeedAction::reply);
    if (offer && item.transfer != Transfer::Offered) {
      const float progress = item.total > 0 ? static_cast<float>(item.bytes) / static_cast<float>(item.total) : 0.0F;
      below->addChild(ui::progressBar({.progress = item.transfer == Transfer::Done ? 1.0F : progress,
                                       .height = Tokens::spaceP4 * s}));
    } else if (canReply && !locked) {
      // pages.html .qr: the reply field (fill_2, send inside) takes the room the quick replies (outlined chips) leave.
      const float height = Tokens::sizeControlM * s;
      auto qr = ui::row({.align = FlexAlign::Center, .gap = pad});
      auto field = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP4 * s, .fill = colorSpecFromRole(ColorRole::Fill2),
                            .radius = height / 2.0F, .minWidth = Tokens::componentLinkReplyMin * s, .height = height, .flexGrow = 1.0F});
      field->setPadding(0.0F, Tokens::spaceP4 * s, 0.0F, gap);
      const std::string key = item.key;
      const auto send = [this, copy, key] {
        const std::string value = m_drafts[key];
        if (value.empty()) return;
        m_drafts.erase(key);
        m_client.reply(copy, value);
      };
      field->addChild(ui::input({.value = m_drafts[key], .placeholder = i18n::tr("link.feed.reply"), .fontSize = Tokens::typeBody.size * s,
                                 .frameVisible = false, .flexGrow = 1.0F,
                                 .onChange = [this, key](const std::string& value) { m_drafts[key] = value; },
                                 .onSubmit = [send](const std::string&) { send(); }}));
      const float key24 = Tokens::sizeControlS * s;
      field->addChild(ui::button({.glyph = std::string("send"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = key24,
                                  .variant = ButtonVariant::Ghost, .tooltip = i18n::tr("link.common.send"), .radius = key24 / 2.0F,
                                  .width = key24, .onClick = send}));
      qr->addChild(std::move(field));
      const Button::ButtonStateColors rest{.bg = clearColorSpec(), .border = colorSpecFromRole(ColorRole::Separator),
                                           .label = colorSpecFromRole(ColorRole::Label2)};
      const Button::ButtonStateColors hot{.bg = colorSpecFromRole(ColorRole::Fill1), .border = colorSpecFromRole(ColorRole::Separator),
                                          .label = colorSpecFromRole(ColorRole::Label1)};
      for (const std::string& reply : item.replies) {
        qr->addChild(ui::button({.text = reply, .fontSize = Tokens::typeBody.size * s, .controlHeight = height,
                                 .customPalette = Button::ButtonPalette{.borderWidth = 1.0F, .normal = rest, .hover = hot, .pressed = hot,
                                                                        .disabled = rest},
                                 .paddingH = gap, .radius = height / 2.0F,
                                 .onClick = [this, copy, reply] { m_client.reply(copy, reply); }}));
      }
      below->addChild(std::move(qr));
    } else if (photos) {
      // pages.html .strip: thumbnails (link.strip_thumb) at gap 4, as many as fit the text column.
      const float side = Tokens::componentLinkStripThumb * s;
      const float stripGap = Tokens::spaceP4 * s;
      const float stripWidth = width - 2.0F * pad - indent;
      const auto fit = static_cast<std::size_t>(std::max(1.0F, std::floor((stripWidth + stripGap) / (side + stripGap))));
      const float radius = Style::scaledRadius(Tokens::radiusR12, s);
      auto strip = ui::row({.gap = stripGap});
      for (const std::string& path : item.photos | std::views::take(fit)) {
        InputArea* area = nullptr;
        auto pick = ui::inputArea({.out = &area, .cursorShape = CursorShape::Pointer, .width = side, .height = side,
                                   .onPress = [this](const InputArea::PointerData& data) {
                                     m_press = Press{.x = data.sceneX, .y = data.sceneY};
                                   },
                                   .onClick = [this](const InputArea::PointerData&) { m_hooks.openPhotos(); }});
        area->setOnMotion([this, area, path, s](const InputArea::PointerData& data) {
          if (!area->pressed() || !m_press) return;
          if (std::hypot(data.sceneX - m_press->x, data.sceneY - m_press->y) < Style::dragStartThreshold * s) return;
          m_press.reset();
          m_hooks.dragPhoto(path);
        });
        pick->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Fill1), .radius = radius, .width = side, .height = side}));
        Image* image = nullptr;
        pick->addChild(ui::image({.out = &image, .fit = ImageFit::Cover, .radius = radius, .width = side, .height = side}));
        const std::vector<std::uint8_t>* png = renderer != nullptr ? m_client.thumbnail(path) : nullptr;
        if (png != nullptr) (void)image->setSourceBytes(*renderer, png->data(), png->size());
        strip->addChild(std::move(pick));
      }
      below->addChild(std::move(strip));
    }
    if (!below->children().empty()) cell->addChild(std::move(below));
    block->addChild(std::move(cell));
    return block;
  }

  std::size_t ThreadAdapter::itemCount() const {
    return m_search ? m_search->threads.size() : m_client.messages().threads().size();
  }

  std::uint64_t ThreadAdapter::itemKey(std::size_t index) const { return m_client.messages().threads()[at(index)].id; }

  std::uint64_t ThreadAdapter::itemRevision(std::size_t index) const {
    const Thread& t = m_client.messages().threads()[at(index)];
    return std::hash<std::string>{}(std::format("{}|{}|{}|{}|{}|{}", t.name, t.last, t.atMs, t.unread, m_selected == t.id,
                                                static_cast<int>(m_client.presence().kind)));
  }

  float ThreadAdapter::measureItem(Renderer& /*renderer*/, std::size_t /*index*/, float /*width*/) {
    return (Tokens::sizeControlM + 2.0F * Tokens::spaceP8) * m_scale;
  }

  std::unique_ptr<Node> ThreadAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  // Thread row (pages.html .row): avatar | name + core unread dot, last line | time at the top; selected = card fill.
  void ThreadAdapter::bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) {
    const float s = m_scale;
    const Thread& t = m_client.messages().threads()[at(index)];
    const bool locked = m_client.presence().kind == Presence::Kind::Locked;
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    const ColorSpec fill = m_selected == t.id ? colorSpecFromRole(ColorRole::Card)
                           : hovered          ? colorSpecFromRole(ColorRole::Fill1)
                                              : clearColorSpec();
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .padding = Tokens::spaceP8 * s, .fill = fill,
                        .radius = Style::scaledRadius(Tokens::radiusR12, s), .height = (Tokens::sizeControlM + 2.0F * Tokens::spaceP8) * s});
    row->addChild(initialsAvatar(t.name, s));
    auto time = text(when(t.atMs), Tokens::typeFootnote, ColorRole::Label3, s);
    const float timeWidth = static_cast<Node&>(*time).measure(renderer, LayoutConstraints::available(width, 1.0e6F)).width;
    const float gap = Tokens::spaceP12 * s;
    const float dot = Tokens::spaceP8 * s;
    const float bodyWidth = std::max(0.0F, width - 2.0F * Tokens::spaceP8 * s - Tokens::sizeControlM * s - 2.0F * gap - timeWidth);
    auto body = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP4 * s});
    auto name = text(t.name, Tokens::typeHeadline, ColorRole::Label1, s);
    name->setMaxWidth(std::max(0.0F, bodyWidth - (t.unread > 0 ? dot + Tokens::spaceP4 * s : 0.0F)));
    head->addChild(std::move(name));
    if (t.unread > 0) head->addChild(ui::box({.fill = core(), .radius = dot / 2.0F, .width = dot, .height = dot}));
    body->addChild(std::move(head));
    auto last = text(locked && t.unread > 0 ? lockedText() : t.last, Tokens::typeFootnote, ColorRole::Label2, s);
    last->setMaxWidth(bodyWidth);
    body->addChild(std::move(last));
    row->addChild(std::move(body));
    row->addChild(ui::column({.align = FlexAlign::End, .paddingV = Tokens::spaceP2 * s, .height = Tokens::sizeControlM * s},
                             std::move(time)));
    holder.addChild(std::move(row));
  }

  void ThreadAdapter::onActivate(std::size_t index) { m_open(m_client.messages().threads()[at(index)].id); }

  std::uint64_t ConversationAdapter::itemRevision(std::size_t index) const {
    const ConversationRow& row = m_rows[index];
    return (static_cast<std::uint64_t>(row.kind) << 2U) | (row.runEnd ? 1U : 0U) |
           (m_client.presence().kind == Presence::Kind::Locked ? 2U : 0U);
  }

  bool ConversationAdapter::itemInteractive(std::size_t index) const {
    return m_rows[index].kind == ConversationRow::Kind::Failed;
  }

  float ConversationAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(index, width, nullptr)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> ConversationAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void ConversationAdapter::bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool /*hovered*/) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(index, width, &renderer));
  }

  void ConversationAdapter::onActivate(std::size_t index) {
    if (m_rows[index].kind == ConversationRow::Kind::Failed) m_client.retryText(m_rows[index].serial);
  }

  // Bubbles (pages.html .bub): in fill_2 left, out label_1 with view ink right, r16 with a r4 tail corner, at most
  // bubble_max wide; the run's time or state under its last bubble; day lines as centred footnotes. An MMS image is
  // a cover inside a p4 bubble.
  std::unique_ptr<Node> ConversationAdapter::build(std::size_t index, float width, Renderer* renderer) const {
    using K = ConversationRow::Kind;
    using N = ConversationRow::Note;
    const float s = m_scale;
    const ConversationRow& row = m_rows[index];
    const float inset = Tokens::spaceP20 * s;
    if (row.kind == K::Day) {
      auto day = ui::row({.justify = FlexJustify::Center}, text(row.text, Tokens::typeFootnote, ColorRole::Label3, s));
      day->setPadding(Tokens::spaceP8 * s, 0.0F, Tokens::spaceP2 * s, 0.0F);
      return day;
    }
    const bool in = row.kind == K::In;
    const bool hidden = in && m_client.presence().kind == Presence::Kind::Locked;
    auto block = ui::column({.align = in ? FlexAlign::Start : FlexAlign::End, .gap = Tokens::spaceP4 * s, .paddingH = inset});
    const float padH = Tokens::spaceP12 * s;
    const ColorSpec fill = colorSpecFromRole(in ? ColorRole::Fill2 : ColorRole::Label1);
    const float r = Style::scaledRadius(Tokens::componentLinkBubbleRadius, s);
    const float tail = Style::scaledRadius(Tokens::radiusR4, s);
    const Radii radii = in ? Radii{r, r, r, tail} : Radii{r, r, tail, r};
    if (row.image != 0 && !hidden) {
      const float pad = Tokens::spaceP4 * s;
      const float inner = Style::scaledRadius(Tokens::componentLinkBubbleRadius - Tokens::spaceP4, s);
      auto bubble = ui::column({.padding = pad, .fill = fill});
      bubble->setRadii(radii);
      Image* image = nullptr;
      bubble->addChild(ui::image({.out = &image, .fit = ImageFit::Cover, .radius = inner,
                                  .width = Tokens::componentLinkMmsImageW * s, .height = Tokens::componentLinkMmsImageH * s}));
      const std::vector<std::uint8_t>* png = renderer != nullptr ? m_client.textImage(row.image) : nullptr;
      if (png != nullptr) (void)image->setSourceBytes(*renderer, png->data(), png->size());
      block->addChild(std::move(bubble));
    } else {
      auto bubble = ui::column({.paddingV = Tokens::spaceP8 * s, .paddingH = padH, .fill = fill});
      bubble->setRadii(radii);
      bubble->addChild(ui::label({.text = hidden ? lockedText() : row.text, .fontSize = Tokens::typeParagraph.size * s,
                                  .fontWeight = static_cast<FontWeight>(Tokens::typeParagraph.weight),
                                  .color = colorSpecFromRole(hidden ? ColorRole::Label2 : in ? ColorRole::Label1 : ColorRole::View),
                                  .maxWidth = std::max(0.0F, (width - 2.0F * inset) * Tokens::componentLinkBubbleMax - 2.0F * padH)}));
      block->addChild(std::move(bubble));
    }
    const bool failed = row.kind == K::Failed || row.note == N::NotSent;
    if (row.runEnd || failed) {
      const std::string meta = row.kind == K::Sending || row.note == N::Sending ? i18n::tr("link.messages.sending")
                               : row.kind == K::Failed                         ? i18n::tr("link.messages.not-sent-retry")
                               : row.note == N::NotSent                        ? i18n::tr("link.messages.not-sent")
                               : row.note == N::Delivered                      ? i18n::tr("link.messages.delivered")
                                                                               : clock(row.atMs);
      block->addChild(text(meta, Tokens::typeCaption, failed ? ColorRole::DangerText : ColorRole::Label3, s));
    }
    return block;
  }

  std::uint64_t CallAdapter::itemKey(std::size_t index) const { return callKey(m_client.calls()[at(index)]); }

  std::uint64_t CallAdapter::itemRevision(std::size_t index) const {
    const CallEntry& c = m_client.calls()[at(index)];
    return std::hash<std::string>{}(std::format("{}|{}|{}|{}|{}", c.name, c.seconds, static_cast<int>(c.kind), dayHead(index),
                                                m_selected == callKey(c)));
  }

  std::string CallAdapter::dayHead(std::size_t index) const {
    const auto& calls = m_client.calls();
    const std::int64_t now = nowMs();
    std::string day = dayLabel(calls[at(index)].atMs, now);
    if (index > 0 && dayLabel(calls[at(index - 1)].atMs, now) == day) return {};
    return day;
  }

  float CallAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(renderer, index, width, false)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> CallAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void CallAdapter::bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(renderer, index, width, hovered));
  }

  void CallAdapter::onActivate(std::size_t index) { m_open(callKey(m_client.calls()[at(index)])); }

  // Call row (pages.html .row, .sh): day head, then direction glyph | name (missed in danger_text) + kind · duration |
  // time at the top; selected = card fill.
  std::unique_ptr<Node> CallAdapter::build(Renderer& renderer, std::size_t index, float width, bool hovered) const {
    const float s = m_scale;
    const CallEntry& c = m_client.calls()[at(index)];
    auto block = ui::column({.align = FlexAlign::Stretch});
    if (const std::string day = dayHead(index); !day.empty()) {
      auto head = text(day, Tokens::typeFootnote, ColorRole::Label3, s);
      head->setFontWeight(FontWeight::SemiBold);
      auto line = ui::row({}, std::move(head));
      line->setPadding(Tokens::spaceP12 * s, Tokens::spaceP8 * s, Tokens::spaceP4 * s, Tokens::spaceP8 * s);
      block->addChild(std::move(line));
    }
    const ColorSpec fill = m_selected == callKey(c) ? colorSpecFromRole(ColorRole::Card)
                           : hovered                ? colorSpecFromRole(ColorRole::Fill1)
                                                    : clearColorSpec();
    block->addChild(callRow(c, c.name.empty() ? m_client.messages().nameFor(c.number) : c.name, callKind(c), clock(c.atMs), fill,
                            renderer, width, s));
    return block;
  }

  TransferAdapter::Section TransferAdapter::sectionOf(const TransferEntry& e) {
    switch (e.state) {
    case Transfer::Offered: return Section::Waiting;
    case Transfer::Running: return Section::Running;
    case Transfer::Done: return Section::Done;
    case Transfer::Failed: return e.status == "cancelled" ? Section::Done : Section::Failed;
    }
    return Section::Done;
  }

  std::size_t TransferAdapter::itemCount() const {
    const Activity& activity = m_client.activity();
    m_rows.clear();
    for (std::size_t i = 0; i < activity.rowCount(); ++i) {
      const Activity::Row row = activity.row(i);
      if (const auto* const* clip = std::get_if<const ClipEntry*>(&row)) {
        if (m_section == Section::Done) m_rows.emplace_back(*clip);
      } else if (const TransferEntry* e = std::get<const TransferEntry*>(row); sectionOf(*e) == m_section) {
        m_rows.emplace_back(e);
      }
    }
    if (m_section == Section::Running) {
      const Presence presence = m_client.presence();
      if (presence.kind == Presence::Kind::Connected || presence.kind == Presence::Kind::Locked) {
        m_phone = presence.phone;
        for (const StreamKind& k : kStreams) {
          if (streamLive(m_phone.*k.state)) m_rows.emplace_back(&k);
        }
      }
    }
    return m_rows.size();
  }

  std::uint64_t TransferAdapter::itemKey(std::size_t index) const {
    return std::visit(
        [](const auto* e) {
          using T = std::remove_cvref_t<decltype(*e)>;
          if constexpr (std::is_same_v<T, TransferEntry>) return std::hash<std::string>{}(e->id);
          else if constexpr (std::is_same_v<T, ClipEntry>) return std::hash<std::string>{}(std::format("c:{}", e->id));
          else return std::hash<std::string>{}(std::format("s:{}", e->glyph));
        },
        m_rows[index]);
  }

  float TransferAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(index, false)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> TransferAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void TransferAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool hovered) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(index, hovered));
  }

  namespace {
    // "2 min", "1 h", then the day: the age of a Done row.
    std::string ageText(std::int64_t atMs, std::int64_t now) {
      const std::int64_t s = std::max<std::int64_t>(0, (now - atMs) / 1000);
      if (s < 60) return i18n::tr("link.activity.age-now");
      if (s < 3600) return i18n::tr("link.activity.age-minutes", "count", s / 60);
      if (daysAgo(atMs, now) == 0) return i18n::tr("link.activity.age-hours", "count", s / 3600);
      return dayLabel(atMs, now);
    }

    std::string folderOf(const std::string& path) {
      const auto slash = path.find_last_of('/');
      const std::string dir = slash == std::string::npos ? std::string() : path.substr(0, slash);
      return dir.substr(dir.find_last_of('/') + 1);
    }
  } // namespace

  // Activity row (pages.html .row in a .card): avatar 32 | headline + footnote (a running transfer puts its core
  // progress bar between them) | pills, or the age at the top right of a Done row.
  std::unique_ptr<Node> TransferAdapter::build(std::size_t index, bool hovered) const {
    const float s = m_scale;
    const std::int64_t now = nowMs();
    const float pad = Tokens::spaceP8 * s;
    const float avatarSize = Tokens::sizeControlM * s;
    auto block = ui::column({.align = FlexAlign::Stretch});
    if (index > 0) block->addChild(ui::separator({}));
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = pad, .paddingH = pad,
                        .radius = Style::scaledRadius(Tokens::radiusR12, s)}); // nested in the r16 card, inset 4
    auto lines = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    const auto headline = [&](std::string value) { lines->addChild(text(std::move(value), Tokens::typeHeadline, ColorRole::Label1, s)); };
    const auto footnote = [&](std::string value, ColorRole role = ColorRole::Label2) {
      lines->addChild(text(std::move(value), Tokens::typeFootnote, role, s));
    };
    std::vector<std::unique_ptr<Node>> trailing;

    if (const auto* const* stream = std::get_if<const StreamKind*>(&m_rows[index])) {
      const StreamKind& k = **stream;
      const bool asking = m_phone.*k.state == "asking";
      auto plate = avatar(ui::glyph({.glyph = std::string(k.glyph), .glyphSize = Tokens::sizeSymbol * s,
                                     .color = fixedColorSpec(rgbaHex(Tokens::componentLinkCoreInk))}),
                          s);
      plate->setFill(core());
      row->addChild(std::move(plate));
      headline(i18n::tr(k.name));
      footnote(i18n::tr(asking ? "link.home.asking" : k.on));
      trailing.push_back(pill(i18n::tr(asking ? "link.home.cancel" : "link.home.stop"), std::nullopt, false, s,
                              [this, id = m_phone.id, glyph = std::string(k.glyph)] { m_hooks.stop(id, glyph); }));
    } else if (const auto* const* clip = std::get_if<const ClipEntry*>(&m_rows[index])) {
      const ClipEntry& e = **clip;
      row->addChild(glyphAvatar("clipboard", s));
      headline(i18n::tr("link.activity.clip-received"));
      std::size_t cut = std::min(e.text.find('\n'), std::size_t{200});
      while (cut < e.text.size() && (static_cast<unsigned char>(e.text[cut]) & 0xC0U) == 0x80U) --cut; // a UTF-8 boundary
      footnote(e.text.substr(0, cut));
      trailing.push_back(text(ageText(e.atMs, now), Tokens::typeFootnote, ColorRole::Label2, s));
    } else {
      const TransferEntry& e = *std::get<const TransferEntry*>(m_rows[index]);
      const std::string name = transferName(e);
      const std::string id = e.id;
      switch (sectionOf(e)) {
      case Section::Running: {
        row->addChild(glyphAvatar(e.incoming ? "file-download" : "file-upload", s));
        headline(i18n::tr(e.incoming ? "link.activity.from-the-phone" : "link.activity.to-the-phone", "name", name));
        const float progress = e.total > 0 ? static_cast<float>(e.bytes) / static_cast<float>(e.total) : 0.0F;
        auto bar = ui::row({.align = FlexAlign::Center, .paddingV = Tokens::spaceP4 * s});
        bar->addChild(ui::progressBar({.fill = core(), .track = colorSpecFromRole(ColorRole::Fill2), .progress = progress,
                                       .height = Tokens::spaceP4 * s, .flexGrow = 1.0F}));
        lines->addChild(std::move(bar));
        std::string status = i18n::tr("link.activity.progress", "pct", e.total > 0 ? e.bytes * 100 / e.total : 0, "total", byteSize(e.total));
        if (const double rate = transferRate(e, now); rate > 0.0) {
          const auto left = static_cast<std::uint64_t>(static_cast<double>(e.total - std::min(e.bytes, e.total)) / rate);
          status += std::format(" · {}/s · {}", byteSize(static_cast<std::uint64_t>(rate)),
                                left < 60 ? i18n::tr("link.activity.seconds-left", "count", left)
                                          : i18n::tr("link.activity.minutes-left", "count", (left + 59) / 60));
        }
        footnote(std::move(status));
        trailing.push_back(pill(i18n::tr("common.actions.cancel"), std::nullopt, false, s, [this, id] { m_client.cancelTransfer(id); }));
        break;
      }
      case Section::Waiting:
        row->addChild(glyphAvatar("file-download", s));
        headline(name);
        footnote(i18n::tr("link.activity.offered", "size", byteSize(e.total)));
        trailing.push_back(pill(i18n::tr("link.common.decline"), std::nullopt, false, s, [this, id] { m_client.declineTransfer(id); }));
        trailing.push_back(pill(i18n::tr("link.common.save"), std::nullopt, true, s, [this, id] { m_client.acceptTransfer(id); }));
        break;
      case Section::Failed:
        row->addChild(glyphAvatar("alert-triangle", s));
        headline(name);
        footnote(e.status == "no-space"    ? i18n::tr("link.activity.failed-no-space")
                 : e.status == "too-large" ? i18n::tr("link.activity.failed-too-large")
                 : e.status == "busy"      ? i18n::tr("link.activity.failed-busy")
                                           : i18n::tr("link.activity.failed-broke-off", "pct", e.total > 0 ? e.bytes * 100 / e.total : 0),
                 ColorRole::DangerText);
        if (Activity::retryable(e)) {
          trailing.push_back(pill(i18n::tr("link.common.try-again"), std::nullopt, false, s, [this, id] { m_client.retryTransfer(id); }));
        }
        break;
      case Section::Done: {
        const bool cancelled = e.state == Transfer::Failed;
        row->addChild(glyphAvatar(e.files.size() == 1 ? fileGlyph(e.files.front().first) : "files", s));
        headline(i18n::tr(e.incoming ? "link.activity.received" : "link.activity.sent", "name", name));
        footnote(cancelled          ? i18n::tr("link.activity.cancelled")
                 : e.paths.empty() ? byteSize(e.total)
                                   : i18n::tr("link.activity.done-in", "size", byteSize(e.total), "folder", folderOf(e.paths.front())));
        trailing.push_back(text(ageText(e.endMs, now), Tokens::typeFootnote, ColorRole::Label2, s));
        break;
      }
      }
    }
    row->addChild(std::move(lines));
    if (m_section == Section::Done) {
      auto top = ui::column({.align = FlexAlign::End, .height = avatarSize}); // pages.html .tm align-self start
      top->addChild(std::move(trailing.front()));
      row->addChild(std::move(top));
    } else {
      for (auto& node : trailing) row->addChild(std::move(node));
    }
    if (hovered && activation(index)) row->setFill(colorSpecFromRole(ColorRole::Fill1));
    block->addChild(std::move(row));
    return block;
  }

  std::function<void()> TransferAdapter::activation(std::size_t index) const {
    if (const auto* const* clip = std::get_if<const ClipEntry*>(&m_rows[index])) {
      return [text = (*clip)->text] {
        if (TextClipboard* clipboard = Input::textClipboard(); clipboard != nullptr) clipboard->setClipboardText(text);
      };
    }
    const auto* const* entry = std::get_if<const TransferEntry*>(&m_rows[index]);
    if (entry == nullptr || sectionOf(**entry) != Section::Done) return nullptr;
    const TransferEntry& e = **entry;
    if (e.incoming && e.paths.size() == 1) return [this, path = e.paths.front()] { m_hooks.open(path); };
    if (!e.paths.empty()) return [this, paths = e.paths] { m_client.showInFolder(paths); };
    return nullptr;
  }

  void TransferAdapter::onActivate(std::size_t index) {
    if (const auto run = activation(index)) run();
  }

  // A transfer's name: its one file, or the count of its files.
  std::string transferName(const TransferEntry& e) {
    return e.files.size() == 1 ? e.files.front().first
           : e.files.empty()   ? i18n::tr("link.activity.desktop-files")
                               : i18n::trp("link.common.files-count", static_cast<long>(e.files.size()));
  }

  LinkApp::LinkApp()
      : DeskApp({.id = kAppId,
                 .name = "Link",
                 .updateManifest = "https://github.com/Andromeda-Desktop/desktop/releases/latest/download/"
                                   "Andromeda-Link-macos-arm64.json"}) {
    i18n::Service::instance().addDomain("link", ANDROMEDA_TRANSLATIONS_DIR);
  }

  LinkApp::~LinkApp() {
    if (m_dropSurface != nullptr && m_window != nullptr && m_dropSurface == m_window->surface()) {
      loop().display().clearDropTarget(*m_dropSurface);
    }
    // The viewer's image holds a texture of m_viewCache: the windows go before it.
    destroyWindows();
    loop().removeSource(&m_viewCache);
    if (m_client) m_client->closePhoto();
  }

  LinkApp::DropCue LinkApp::dropCueHere() const {
    if (!m_client) return DropCue::None;
    const Presence::Kind kind = m_client->presence().kind;
    if (kind != Presence::Kind::Connected && kind != Presence::Kind::Locked) return DropCue::None;
    if (pairShown()) return DropCue::None;
    // "/" lists the phone's roots; Link1 writes only inside a root.
    const bool folder = m_page == Page::Files && m_client->files().load() == Files::Load::Ready &&
                        m_client->files().path() != "/" && m_client->fileOp().kind != FileOp::Kind::Copying;
    return folder ? DropCue::CopyIn : DropCue::Send;
  }

  // Files dropped on the window: on an open phone folder they are copied into it, anywhere else sent to the phone.
  void LinkApp::registerDropTarget() {
    Surface* surface = m_window->surface();
    if (surface == nullptr || surface == m_dropSurface) return;
    const auto cue = [this](DropCue next) {
      if (next == m_dropCue) return;
      m_dropCue = next;
      m_window->requestRebuild();
    };
    const bool registered = loop().display().setDropTarget(*surface, {uri::kUriListMime}, {.enter = [](const std::vector<std::string>& mimes) {
                               return std::ranges::contains(mimes, std::string(uri::kUriListMime));
                             },
                             .motion = [this, cue](float, float) -> std::uint32_t {
                               cue(dropCueHere());
                               return m_dropCue == DropCue::None ? DropNone : DropCopy;
                             },
                             .leave = [cue] { cue(DropCue::None); },
                             .drop = [this, cue](float, float, std::uint32_t, const std::string& mime, std::string data) {
                               cue(DropCue::None);
                               if (mime == uri::kUriListMime) dropped(data);
                             }});
    if (registered) m_dropSurface = surface;
  }

  void LinkApp::dropped(const std::string& uris) {
    std::vector<std::filesystem::path> paths;
    std::istringstream lines(uris);
    for (std::string line; std::getline(lines, line);) {
      if (!line.empty() && line.back() == '\r') line.pop_back();
      if (line.empty() || line.front() == '#') continue;
      if (auto path = uri::pathFromFileUri(line)) paths.emplace_back(*path);
    }
    if (paths.empty()) return;
    switch (dropCueHere()) {
    case DropCue::None: break;
    case DropCue::Send: m_client->sendFiles(m_client->presence().phone.id, paths); break;
    case DropCue::CopyIn: m_client->copyIntoFolder(paths); break;
    }
  }

  void LinkApp::openLocalFile(const std::string& path) {
    GError* error = nullptr;
    const bool absolute = path.starts_with('/');
    const std::string target = absolute ? uri::fileUri(path) : std::string{};
    std::unique_ptr<GAppLaunchContext, void (*)(gpointer)> context(g_app_launch_context_new(), g_object_unref);
    if (const std::string token = m_window->activationToken(); !token.empty()) {
      g_app_launch_context_setenv(context.get(), "XDG_ACTIVATION_TOKEN", token.c_str());
    }
    if (!absolute || !g_app_info_launch_default_for_uri(target.c_str(), context.get(), &error)) {
      Logger("link").warn("open {}: {}", path, error != nullptr ? error->message : "?");
    }
    g_clear_error(&error);
  }

  // The phone menu: every paired phone, then Pair another phone.
  void LinkApp::phoneMenu(float sceneX, float sceneY) {
    const std::vector<Phone> phones = m_client->phones();
    std::vector<ContextMenuControlEntry> entries;
    for (std::size_t i = 0; i < phones.size(); ++i) entries.push_back({.id = static_cast<std::int32_t>(i), .label = phones[i].name});
    entries.push_back({.separator = true});
    entries.push_back({.id = kPairEntry, .label = i18n::tr("link.settings.pair-another")});
    m_window->openContextMenu(std::move(entries), sceneX, sceneY, [this, phones](std::int32_t id) {
      if (id == kPairEntry) {
        m_client->startPairing();
      } else if (id >= 0 && static_cast<std::size_t>(id) < phones.size()) {
        choosePhone(phones[static_cast<std::size_t>(id)].id);
      }
    });
  }

  void LinkApp::fileMenu(std::size_t index, float sceneX, float sceneY) {
    const Files& files = m_client->files();
    if (index >= files.entries().size()) return;
    const FileEntry entry = files.entries()[index];
    const FileSave* save = entry.folder ? nullptr : files.save(files.childPath(entry.name));
    const bool saved = save != nullptr && save->state == FileSave::State::Done;
    enum : std::int32_t { kOpen, kDelete };
    std::vector<ContextMenuControlEntry> entries{
        {.id = kOpen, .label = entry.folder ? i18n::tr("link.common.open") : saved ? i18n::tr("link.common.show-in-folder") : i18n::tr("link.common.save-to-downloads")},
        {.separator = true},
        {.id = kDelete, .label = i18n::tr("link.files.delete-menu")}};
    m_window->openContextMenu(std::move(entries), sceneX, sceneY, [this, index, entry](std::int32_t id) {
      if (id == kOpen) m_files->open(index);
      if (id == kDelete) confirmDelete(entry);
    });
  }

  void LinkApp::confirmDelete(const FileEntry& entry) {
    const std::string phone = m_client->presence().phone.name;
    const std::string path = m_client->files().childPath(entry.name);
    const std::string body = entry.folder
                                 ? i18n::tr("link.files.delete-folder-body", "phone", phone)
                                 : i18n::tr("link.files.delete-file-body", "phone", phone);
    m_window->presentAlert(
        {.title = i18n::tr("link.files.delete-title", "name", entry.name),
         .body = body,
         .buttons = {{.text = i18n::tr("common.actions.cancel"), .onClick = [this] { m_window->dismissSheet(); }},
                     {.text = i18n::tr("link.files.delete"), .variant = ButtonVariant::Destructive,
                      .onClick = [this, path] {
                        m_window->dismissSheet();
                        m_client->deleteFile(path);
                      }}}},
        [this] { m_window->dismissSheet(); });
  }

  AppWindow& LinkApp::createWindow() {
    AppWindow& window = addWindow({.appId = info().id, .title = i18n::tr("link.pages.waiting"), .width = 1280, .height = 800, .minWidth = 760,
                                   .sidebarWidth = Tokens::componentSidebarWidth,
                                   .material = AppWindow::Material::Solid});
    m_window = &window;
    // The sky reaches the window's top: no header, the sky holds the window buttons.
    window.setContentCards(true);
    if (m_client == nullptr) {
      m_client = std::make_unique<LinkClient>([this](const std::vector<std::string>& paths) { showInFolder(paths); });
      const FeedAdapter::Hooks hooks{.textTo =
                                         [this](const std::string& number) {
                                           openCompose(number);
                                           show(Page::Messages);
                                         },
                                     .openPhotos = [this] { show(Page::Photos); },
                                     .dragPhoto = [this](const std::string& path) { dragPhoto(path); }};
      for (const auto section : {FeedAdapter::Section::Answer, FeedAdapter::Section::Notifications}) {
        m_feed[static_cast<std::size_t>(section)] = std::make_unique<FeedAdapter>(*m_client, m_scale, section, hooks);
      }
      m_threads = std::make_unique<ThreadAdapter>(*m_client, m_scale, m_thread, m_search, [this](std::uint64_t id) { openThread(id); });
      m_conversation = std::make_unique<ConversationAdapter>(*m_client, m_scale);
      m_calls = std::make_unique<CallAdapter>(*m_client, m_scale, m_call, m_search, [this](std::uint64_t key) {
        m_call = key;
        m_window->requestRebuild();
      });
      m_photos = std::make_unique<PhotoAdapter>(*m_client, m_scale, m_search, [this](std::size_t item) {
        viewPhoto(m_client->photos().items()[item].path);
      });
      m_viewCache.initialize(&loop().gl());
      m_viewCache.setMakeCurrentCallback([this] { (void)loop().renderContext().backend().makeCurrentNoSurface(); });
      m_viewCache.releaseUnusedOn(loop().renderContext().unusedCacheRelease());
      loop().addSource(&m_viewCache);
      m_files = std::make_unique<FileAdapter>(*m_client, m_scale, m_search,
                                              [this](std::size_t index, float x, float y) { fileMenu(index, x, y); });
      m_client->setOnMirrorFrame([this](MirrorDecoder::Frame frame) { showMirrorFrame(std::move(frame)); });
      m_client->setOnThumbnail([this] {
        if (m_photoGrid != nullptr) m_photoGrid->notifyDataChanged();
        for (VirtualListView* list : m_feedList) {
          if (list != nullptr) list->notifyDataChanged();
        }
        if (m_conversationList != nullptr) m_conversationList->notifyDataChanged();
      });
      const TransferAdapter::Hooks transferHooks{.open = [this](const std::string& path) { openLocalFile(path); },
                                                 .stop = [this](const std::string& id, const std::string& glyph) { stopStream(id, glyph); }};
      for (const auto section : {TransferAdapter::Section::Running, TransferAdapter::Section::Waiting, TransferAdapter::Section::Failed,
                                 TransferAdapter::Section::Done}) {
        m_transfers[static_cast<std::size_t>(section)] = std::make_unique<TransferAdapter>(*m_client, m_scale, section, transferHooks);
      }
      m_client->setOnPaired([this](const std::string& id) { choosePhone(id); });
      DeskApp::commands().add({.id = "edit.find", .label = i18n::tr("link.commands.find"), .group = "Edit",
                               .shortcut = KeyChord{.sym = XKB_KEY_f, .modifiers = KeyMod::Ctrl},
                               .enabled = [this] { return m_searchInput != nullptr; },
                               .run = [this] { m_window->input().setFocus(m_searchInput->inputArea()); }});
      DeskApp::commands().add({.id = "photo.previous", .label = i18n::tr("link.commands.previous-photo"), .group = "View",
                               .shortcut = KeyChord{.sym = XKB_KEY_Left, .modifiers = 0},
                               .enabled = [this] { return m_viewing && m_page == Page::Photos && photoStep(-1); },
                               .run = [this] { viewPhoto(photoStep(-1)); }});
      DeskApp::commands().add({.id = "photo.next", .label = i18n::tr("link.commands.next-photo"), .group = "View",
                               .shortcut = KeyChord{.sym = XKB_KEY_Right, .modifiers = 0},
                               .enabled = [this] { return m_viewing && m_page == Page::Photos && photoStep(1); },
                               .run = [this] { viewPhoto(photoStep(1)); }});
      DeskApp::commands().add({.id = "view.close", .label = i18n::tr("link.commands.close"), .group = "View",
                      .shortcut = KeyChord{.sym = XKB_KEY_Escape, .modifiers = 0},
                      .enabled = [this] { return dismissable(); }, .run = [this] { dismiss(); }});
      m_client->setOnChanged([this] {
        if (m_window == nullptr) return;
        m_commands.listed(m_client->desktopSettings().commands);
        if (m_client->mirror().kind != Mirror::Kind::Showing && m_mirrorTexture.valid()) retireMirrorFrame(true);
        sync();
        syncHotspotSheet();
        updateCaption();
        refreshSearch();
        if (m_viewing && std::ranges::find(m_client->photos().items(), *m_viewing, &Photo::path) == m_client->photos().items().end()) {
          m_viewing.reset();
          m_client->closePhoto();
        }
        m_window->requestRebuild();
      });
      // The snapshot feed is there before Link1 answers.
      sync();
      updateCaption();
    }
    window.setSidebarBuilder([this](Renderer&, float width, float) { return sidebar(width); });
    window.setContentBuilder([this](Renderer& renderer, float width, float height) {
      registerDropTarget();
      auto page = content(renderer, width, height);
      if (m_dropCue == DropCue::None) return page;
      // The drop cue covers the content while files are dragged over the window.
      const float s = m_scale;
      auto root = ui::column({.align = FlexAlign::Stretch, .width = width, .height = height});
      root->addChild(std::move(page));
      auto cue = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP8 * s,
                             .fill = colorSpecFromRole(ColorRole::Accent, 0.14F),
                             .radius = Style::scaledRadius(Tokens::componentWindowCardRadius, s),
                             .border = colorSpecFromRole(ColorRole::Accent), .borderWidth = 2.0F * s});
      cue->setParticipatesInLayout(false);
      cue->setPosition(0.0F, 0.0F);
      cue->setSize(width, height);
      const bool copy = m_dropCue == DropCue::CopyIn;
      const std::vector<std::string> crumbs = m_client->files().crumbs();
      cue->addChild(ui::glyph({.glyph = std::string(copy ? "folder-down" : "arrow-up"), .glyphSize = Tokens::sizeSymbol * 2.0F * s,
                               .color = colorSpecFromRole(ColorRole::AccentText)}));
      cue->addChild(text(copy ? i18n::tr("link.files.drop-copy", "folder", crumbs.empty() ? i18n::tr("link.files.phone-root") : crumbs.back())
                              : i18n::tr("link.files.drop-send", "phone", m_client->presence().phone.name),
                         Tokens::typeTitle3, ColorRole::Label1, s));
      root->addChild(std::move(cue));
      return std::unique_ptr<Node>(std::move(root));
    });
    return window;
  }

  void LinkApp::action(const std::string& name, const std::vector<std::string>& parameter, const std::string& activationToken) {
    // "page" takes the page name ("commands": Settings › Commands; "pair": pairs another phone), then optionally the
    // id of the phone to show.
    if (name == "page" && parameter.size() == 1 && parameter.front() == "pair") {
      activate(activationToken);
      if (m_client) m_client->startPairing();
      return;
    }
    if (name == "page" && (parameter.size() == 1 || parameter.size() == 2) && parameter.front() == "commands") {
      activate(activationToken);
      if (parameter.size() == 2) choosePhone(parameter[1]);
      if (m_window == nullptr) return;
      show(Page::Settings);
      m_settingsView = SettingsView::Commands;
      sync();
      m_window->requestRebuild();
      return;
    }
    if (name == "page" && (parameter.size() == 1 || parameter.size() == 2)) {
      const auto it = std::ranges::find_if(kPages, [&](const PageInfo& info) {
        return std::ranges::equal(std::string_view(info.name), parameter.front(),
                                  [](char a, char b) { return std::tolower(a) == std::tolower(b); });
      });
      if (it != kPages.end()) {
        activate(activationToken);
        if (parameter.size() == 2) choosePhone(parameter[1]);
        if (m_window != nullptr) show(it->page);
        return;
      }
    }
    DeskApp::action(name, parameter, activationToken);
  }

  void LinkApp::openThread(std::optional<std::uint64_t> id) {
    if (m_thread == id && !m_compose) return;
    m_compose.reset();
    m_thread = id;
    m_conversationScroll = {};
    sync();
    m_window->requestRebuild();
  }

  void LinkApp::openCompose(const std::string& to) {
    const std::string number = to.empty() ? std::string() : m_client->messages().numberFor(to);
    if (const auto thread = number.empty() ? std::nullopt : m_client->messages().threadFor(number)) {
      openThread(thread);
      return;
    }
    m_thread.reset();
    m_compose = Compose{.to = to};
    sync();
    m_window->requestRebuild();
  }

  void LinkApp::dragPhoto(const std::string& path) {
    if (m_window == nullptr || m_window->surface() == nullptr) return;
    const std::filesystem::path folder = std::filesystem::path(g_get_user_cache_dir()) / "andromeda" / "link-app" / "drag";
    std::error_code error;
    std::filesystem::create_directories(folder, error);
    // Earlier drags' copies: a drop target has read them long ago.
    const auto old = std::filesystem::file_time_type::clock::now() - std::chrono::hours(24);
    for (const auto& entry : std::filesystem::directory_iterator(folder, error)) {
      if (entry.last_write_time(error) < old) std::filesystem::remove(entry.path(), error);
    }
    // The URI list once the save answered (empty when it failed); drop targets that ask before wait in `waiting`.
    struct Drop {
      std::optional<std::string> uris;
      std::vector<std::function<void(const std::string&)>> waiting;
    };
    auto drop = std::make_shared<Drop>();
    m_client->savePhotoTo(path, folder.string(), [drop](const std::string& saved) {
      std::string uris;
      if (saved.starts_with('/')) uris = uri::joinUriList({uri::fileUri(saved)});
      drop->uris = uris;
      for (const auto& reply : std::exchange(drop->waiting, {})) reply(uris);
    });
    (void)loop().display().startDrag(*m_window->surface(),
                         {.mimeTypes = {uri::kUriListMime},
                          .dataLater =
                              [drop](const std::string&, std::function<void(const std::string&)> reply) {
                                if (drop->uris) {
                                  reply(*drop->uris);
                                } else {
                                  drop->waiting.push_back(std::move(reply));
                                }
                              },
                          .fileName = std::filesystem::path(path).filename().string(),
                          .actions = DropCopy | DropMove});
  }

  void LinkApp::chooseFiles(std::function<void(std::vector<std::filesystem::path>)> done) {
    if (m_window == nullptr || bus() == nullptr) return;
    if (m_chooser == nullptr) m_chooser = std::make_unique<desk::FileChooser>(bus());
    m_window->exportHandle([this, done = std::move(done)](const std::string& parent) {
      m_chooser->openFiles(parent, i18n::tr("link.common.send-files-title"), [done](std::vector<std::filesystem::path> paths) {
        if (!paths.empty()) done(std::move(paths));
      });
    });
  }

  void LinkApp::chooseAndSend(const std::string& deviceId) {
    chooseFiles([this, deviceId](std::vector<std::filesystem::path> paths) { m_client->sendFiles(deviceId, paths); });
  }

  void LinkApp::choosePhone(const std::string& deviceId) {
    if (!m_client || m_client->presence().phone.id == deviceId) return;
    if (m_page == Page::Mirror) m_client->stopMirror();
    m_client->select(deviceId);
    if (m_window == nullptr) return;
    if (m_page == Page::Photos) m_client->loadPhotos();
    if (m_page == Page::Files) m_client->openFolder("/");
    if (m_page == Page::Mirror) m_client->startMirror();
    sync();
    updateCaption();
    m_window->requestRebuild();
  }

  bool LinkApp::pairShown() const {
    return m_client && m_client->pair().kind != Pair::Kind::Idle &&
           m_client->presence().kind != Presence::Kind::NoDaemon;
  }

  bool LinkApp::dismissable() const { return pairShown() || m_confirmForget || m_viewing || m_search; }

  void LinkApp::dismiss() {
    if (m_viewing) {
      viewPhoto(std::nullopt);
      return;
    }
    if (pairShown()) {
      m_client->cancelPairing();
    } else if (m_confirmForget) {
      m_confirmForget = false;
    } else {
      setQuery({});
      return;
    }
    m_window->requestRebuild();
  }

  void LinkApp::setQuery(std::string query) {
    m_query = std::move(query);
    refreshSearch();
    if (m_window != nullptr) m_window->requestRebuild();
  }

  void LinkApp::refreshSearch() {
    if (m_query.empty() || !m_client) {
      m_search.reset();
      return;
    }
    m_search = searchHits(m_query, m_client->messages().threads(), m_client->calls(), m_client->files().entries(),
                          m_client->photos().items());
  }

  void LinkApp::show(Page page) {
    if (m_client) m_client->cancelPairing();
    if (m_page == page) return;
    if (m_client && m_page == Page::Mirror) m_client->stopMirror();
    if (m_client && m_viewing) {
      m_viewing.reset();
      m_client->closePhoto();
    }
    m_page = page;
    m_confirmForget = false;
    m_settingsView = SettingsView::Main;
    commandsChanged(m_commands.close());
    if (page == Page::Photos && m_client) m_client->loadPhotos();
    if (page == Page::Files && m_client) m_client->openFolder(m_client->files().path());
    if (page == Page::Mirror && m_client) m_client->startMirror();
    sync();
    updateCaption();
    m_window->requestRebuild();
  }

  void LinkApp::syncHotspotSheet() {
    const auto& ask = m_client->hotspotAsk();
    if (!ask) {
      if (!std::exchange(m_hotspotSheet, {}).empty()) m_window->dismissSheet();
      return;
    }
    // A newer ask replaces the shown one, so the text always names the device the answer goes to.
    std::string shown = ask->deviceId + '\n' + ask->ssid;
    if (shown == m_hotspotSheet) return;
    // The shown ask's sheet gives way without its onCancel: that would decline the new ask.
    const bool replacing = !m_hotspotSheet.empty();
    m_hotspotSheet = std::move(shown);
    const Presence presence = m_client->presence();
    const std::string phone = presence.phone.id == ask->deviceId ? presence.phone.name : i18n::tr("link.hotspot.your-phone");
    (m_window->*(replacing ? &AppWindow::replaceAlert : &AppWindow::presentAlert))(
        {.title = i18n::tr("link.hotspot.title", "phone", phone),
         .body = i18n::tr("link.hotspot.body", "ssid", ask->ssid),
         .buttons = {{.text = i18n::tr("link.hotspot.use"), .variant = ButtonVariant::Primary,
                      .onClick = [this] { m_client->answerHotspot(true); }},
                     {.text = i18n::tr("link.hotspot.always"), .variant = ButtonVariant::Tonal,
                      .onClick = [this] { m_client->answerHotspot(true, true); }},
                     {.text = i18n::tr("link.hotspot.not-now"), .variant = ButtonVariant::Tonal,
                      .onClick = [this] { m_client->answerHotspot(false); }}}},
        [this] { m_client->answerHotspot(false); });
  }

  void LinkApp::sync() {
    if (m_client == nullptr) return;
    if (m_page == Page::Photos) m_client->markPhotosSeen();
    const Presence::Kind kind = m_client->presence().kind;
    const bool pairing = kind == Presence::Kind::NotPaired;
    if (pairing && m_page != Page::Home && m_client->pair().kind == Pair::Kind::Idle) m_client->startPairing();
    m_window->setTitle(pairShown() ? i18n::tr("link.pair.title") : i18n::tr(kPages[static_cast<std::size_t>(m_page)].title));
    const bool live = kind == Presence::Kind::Connected || kind == Presence::Kind::Locked;
    if (m_page == Page::Files && live && m_client->files().load() == Files::Load::Idle) m_client->openFolder("/");
    const bool commands = m_page == Page::Settings && m_settingsView == SettingsView::Commands && !pairing && !pairShown() &&
                          kind != Presence::Kind::NoDaemon;
    const bool cards =
        ((m_page == Page::Messages || m_page == Page::Photos || m_page == Page::Files || m_page == Page::Mirror) && live) ||
        commands;
    const Messages& messages = m_client->messages();
    if (m_compose && !m_compose->sent.empty()) {
      if (const auto thread = messages.threadFor(m_compose->sent)) {
        m_compose.reset();
        m_thread = thread;
        m_conversationScroll = {};
      }
    }
    if (cards && !m_compose && (!m_thread || messages.thread(*m_thread) == nullptr)) {
      m_thread = messages.threads().empty() ? std::nullopt : std::optional(messages.threads().front().id);
      m_conversationScroll = {};
    }
    m_client->viewThread(cards ? m_thread : std::nullopt);
  }

  void LinkApp::updateCaption() {
    const std::size_t n = m_client ? m_client->feed().items().size() : 0;
    std::string state;
    if (m_page == Page::Waiting && n > 0 && !pairShown()) state = n == 1 ? "1 item" : std::format("{} items", n);
    m_window->setCaption({.state = state});
  }

  std::unique_ptr<Node> LinkApp::sidebar(float /*width*/) {
    const float s = m_scale = m_window->uiScale();
    auto side = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP8 * s, .padding = Tokens::componentSidebarPadding * s,
                            .flexGrow = 1.0F});
    side->addChild(ui::searchCapsule({.out = &m_searchInput, .placeholder = "Search", .height = Tokens::sizeControlM, .scale = s,
                                      .value = m_query, .clearButton = true,
                                      .onChange = [this](const std::string& query) {
                                        m_searchCaret = m_searchInput != nullptr ? m_searchInput->caret() : query.size();
                                        setQuery(query);
                                      },
                                      .onKeyEvent = [this](std::uint32_t sym, std::uint32_t) {
                                        if (sym != XKB_KEY_Escape || m_query.empty()) return false;
                                        setQuery({});
                                        return true;
                                      }}));
    // A rebuild hands the keyboard focus to the new field by this key.
    m_searchInput->inputArea()->setTabFocusKey("link-search");
    m_searchInput->selectRange(m_searchCaret, m_searchCaret); // the new field keeps the caret where the edit left it
    auto list = std::make_unique<SelectionList>(s);
    list->setGap(Tokens::spaceP4 * s);
    Node* selected = nullptr;
    const std::vector<FeedItem> none;
    const auto& items = m_client ? m_client->feed().items() : none;
    // "<n> <noun> · <newest who>" and the newest time for one kind of feed item.
    const auto summary = [&](FeedItem::Kind kind, const char* key) -> std::pair<std::string, std::string> {
      const auto count = std::ranges::count(items, kind, &FeedItem::kind);
      if (count == 0) return {};
      const auto newest = std::ranges::find(items, kind, &FeedItem::kind);
      const std::string who = kind == FeedItem::Kind::Offer ? std::string() : " · " + newest->who;
      return {i18n::tr(key, "count", count) + who, clock(newest->atMs)};
    };
    for (const PageInfo& info : kPages) {
      if (info.page == Page::Settings) continue;
      auto row = std::make_unique<StatusRow>(s);
      std::pair<std::string, std::string> line;
      switch (info.page) {
      case Page::Home: break;
      case Page::Waiting:
        if (!items.empty()) line.first = i18n::trp("link.sidebar.items", static_cast<long>(items.size()));
        else if (m_client && m_client->presence().kind == Presence::Kind::Away) line.first = i18n::tr("link.sidebar.phone-not-connected");
        break;
      case Page::Messages: {
        static const Messages kNone;
        const Messages& messages = m_client ? m_client->messages() : kNone;
        const auto newest = std::ranges::find_if(messages.threads(), [](const Thread& t) { return t.unread > 0; });
        if (newest != messages.threads().end()) {
          line = {i18n::tr("link.sidebar.unread", "count", messages.unread(), "name", newest->name), clock(newest->atMs)};
        } else if (!messages.threads().empty()) {
          line.first = i18n::tr("link.sidebar.all-read");
        }
        break;
      }
      case Page::Calls: line = summary(FeedItem::Kind::MissedCall, "link.sidebar.missed"); break;
      case Page::Activity:
        if (m_client) {
          const Activity& activity = m_client->activity();
          const std::size_t offered = activity.offeredCount();
          const auto running = std::ranges::count(activity.items(), Transfer::Running, &TransferEntry::state);
          if (offered > 0) {
            line.first = i18n::tr("link.sidebar.offered", "count", offered);
          } else if (running > 0) {
            line.first = i18n::trp("link.sidebar.transfers-running", running);
          }
        }
        break;
      case Page::Files: line.first = i18n::tr("link.sidebar.phone-storage"); break;
      case Page::Mirror: line.first = i18n::tr("link.sidebar.show-screen"); break;
      case Page::Photos: {
        if (!m_client) break;
        const std::int64_t now = nowMs();
        const auto days = m_client->photos().days(now);
        if (days.empty() || daysAgo(m_client->photos().items()[days.front().first].atS * 1000, now) != 0) break;
        const std::size_t today = days.size() > 1 ? days[1].first : m_client->photos().items().size();
        line.first = i18n::trp("link.sidebar.new-photos-today", static_cast<long>(today));
        break;
      }
      case Page::Settings: break;
      }
      bool hot = !line.second.empty() || (info.page == Page::Waiting && !items.empty());
      if (m_search) {
        const auto count = [&]() -> std::optional<std::size_t> {
          switch (info.page) {
          case Page::Messages: return m_search->threads.size();
          case Page::Calls: return m_search->calls.size();
          case Page::Photos: return m_search->photos.size();
          case Page::Files: return m_search->files.size();
          default: return std::nullopt;
          }
        }();
        line.second = count ? std::to_string(*count) : std::string();
        hot = count.value_or(0) > 0;
      }
      row->setData({.symbol = info.glyph, .name = i18n::tr(info.title), .status = line.first, .time = line.second, .unseen = hot,
                    .selected = info.page == m_page && !pairShown()});
      row->setOnClick([this, page = info.page] { show(page); });
      if (info.page == m_page && !pairShown()) selected = row.get();
      list->addChild(std::move(row));
    }
    auto settings = std::make_unique<StatusRow>(s);
    settings->setOneLine(true);
    settings->setData({.symbol = "settings", .name = i18n::tr("link.pages.settings"), .selected = m_page == Page::Settings && !pairShown()});
    settings->setOnClick([this] { show(Page::Settings); });
    if (m_page == Page::Settings && !pairShown()) selected = settings.get();
    list->addChild(std::move(settings));
    if (selected != nullptr) list->setSelected(selected, false);
    // Pair (flow.html#page=pair): only search and the PhoneCard until a phone is paired.
    if (!m_client || m_client->presence().kind != Presence::Kind::NotPaired) {
      // The list scrolls on a short window, so the PhoneCard below always shows whole.
      auto scroll = ui::scrollView({.state = &m_sideScroll, .contentScale = s, .scrollbarAutoHide = true,
                                    .contentAlign = ScrollView::ContentAlign::Start, .viewportPaddingV = 0.0F, .flexGrow = 1.0F});
      scroll->content()->addChild(std::move(list));
      side->addChild(std::move(scroll));
    } else {
      side->addChild(ui::spacer());
    }

    return side;
  }

  const PhonePlayer* LinkApp::player() const {
    if (!m_client) return nullptr;
    const Presence presence = m_client->presence();
    if (presence.kind != Presence::Kind::Connected && presence.kind != Presence::Kind::Locked) return nullptr;
    for (const Phone& phone : m_client->phones()) {
      if (phone.id == presence.phone.id && phone.player) return &*phone.player;
    }
    return nullptr;
  }

  // Waiting (pages.html B.waiting): "<n> waiting" + Clear all, then the two section cards side by side. A card is as
  // tall as its rows, at most the room below its head; its rows scroll inside it.
  std::unique_ptr<Node> LinkApp::waiting(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const float gap = Tokens::spaceP24 * s;
    const float barGap = Tokens::spaceP16 * s;
    const float inset = Tokens::spaceP4 * s;
    auto page = ui::column({.align = FlexAlign::Stretch, .gap = barGap, .width = width, .height = height});
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .minHeight = Tokens::sizeControlM * s});
    auto count = text(i18n::tr("link.waiting.count", "count", m_client->feed().items().size()), Tokens::typeFootnote, ColorRole::Label2, s);
    count->setFontWeight(FontWeight::SemiBold);
    count->setFlexGrow(1.0F);
    bar->addChild(std::move(count));
    bar->addChild(pill(i18n::tr("link.common.clear-all"), std::nullopt, false, s, [this] { m_client->clearAll(); }));
    const float barHeight = bar->measure(renderer, LayoutConstraints::available(width, height)).height;
    page->addChild(std::move(bar));

    const float columnWidth = std::floor((width - gap) / 2.0F);
    auto columns = ui::row({.align = FlexAlign::Start, .gap = gap, .flexGrow = 1.0F});
    for (const auto section : {FeedAdapter::Section::Answer, FeedAdapter::Section::Notifications}) {
      const auto i = static_cast<std::size_t>(section);
      auto column = ui::column({.align = FlexAlign::Stretch, .width = columnWidth});
      FeedAdapter& adapter = *m_feed[i];
      const std::size_t rows = adapter.itemCount();
      if (rows > 0) {
        auto head = ui::row({.align = FlexAlign::Center});
        head->setPadding(Tokens::spaceP12 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s);
        auto title = text(i18n::tr(section == FeedAdapter::Section::Answer ? "link.waiting.answer" : "link.waiting.notifications"),
                          Tokens::typeFootnote, ColorRole::Label2, s);
        title->setFontWeight(FontWeight::SemiBold);
        head->addChild(std::move(title));
        const float headHeight = head->measure(renderer, LayoutConstraints::available(columnWidth, height)).height;
        column->addChild(std::move(head));
        const float room = std::max(0.0F, height - barHeight - barGap - headHeight - 2.0F * inset);
        const float rowWidth = columnWidth - 2.0F * inset;
        float content = 0.0F;
        for (std::size_t r = 0; r < rows && content < room; ++r) content += adapter.measureItem(renderer, r, rowWidth);
        const float radius = Style::scaledRadius(Tokens::radiusR16, s);
        auto card = ui::column({.align = FlexAlign::Stretch, .padding = inset, .fill = colorSpecFromRole(ColorRole::Card),
                                .radius = radius, .clipChildren = true});
        auto rowsBox = ui::column({.align = FlexAlign::Stretch, .height = std::min(content, room)});
        rowsBox->addChild(ui::virtualListView({.out = &m_feedList[i], .state = &m_feedScroll[i], .contentScale = s, .adapter = &adapter,
                                               .flexGrow = 1.0F}));
        card->addChild(std::move(rowsBox));
        column->addChild(std::move(card));
      }
      columns->addChild(std::move(column));
    }
    page->addChild(std::move(columns));
    return page;
  }

  std::string LinkApp::pageTitle(Page page) { return i18n::tr(kPages[static_cast<std::size_t>(page)].title); }

  std::unique_ptr<Node> LinkApp::pageBody(Renderer& renderer, float width, float height) {
    m_photoGrid = nullptr;
    m_feedList = {};
    m_conversationList = nullptr;
    m_renderer = &renderer;
    const float s = m_scale = m_window->uiScale();
    if (m_client == nullptr) return emptyState("plug-connected-x", i18n::tr("link.errors.no-bus-title"), i18n::tr("link.errors.no-bus-body"), s);
    const Presence presence = m_client->presence();
    switch (presence.kind) {
    case Presence::Kind::NoDaemon:
      if (m_page == Page::Settings) return settings(presence);
      return emptyState("plug-connected-x", i18n::tr("link.phone-card.not-running"), i18n::tr("link.errors.start-service"), s);
    case Presence::Kind::NotPaired:
      return pair(renderer, width, height);
    case Presence::Kind::Away: {
      if (m_page == Page::Settings && m_settingsView == SettingsView::Commands) return commands(presence, width, height);
      if (m_page == Page::Settings) return settings(presence);
      const std::string id = presence.phone.id;
      const std::string seen = lastSeenText(presence.phone.lastSeen, nowMs() / 1000);
      const std::string_view transport = presence.phone.transport;
      const char* hint = presence.phone.lastSeen <= 0 ? "link.away.hint"
                         : transport.empty()          ? "link.away.hint-seen"
                         : transport == "bluetooth"   ? "link.away.hint-seen-bluetooth"
                                                      : "link.away.hint-seen-wifi";
      return emptyState("device-mobile", i18n::tr("link.away.title", "phone", presence.phone.name), i18n::tr(hint, "seen", seen), s,
                        ui::button({.text = i18n::tr("link.away.connect"), .variant = ButtonVariant::Primary,
                                    .onClick = [this, id] { m_client->connect(id); }}));
    }
    case Presence::Kind::Connected:
    case Presence::Kind::Locked: break;
    }
    if (m_page == Page::Messages) return messages(width, height);
    if (m_page == Page::Calls) return calls(renderer, width, height);
    if (m_page == Page::Photos) return photos(renderer, width, height);
    if (m_page == Page::Files) return files(width, height);
    if (m_page == Page::Activity) return activity(renderer, width, height);
    if (m_page == Page::Settings && m_settingsView == SettingsView::Commands) return commands(presence, width, height);
    if (m_page == Page::Settings) return settings(presence);
    if (m_page != Page::Waiting) {
      const PageInfo& info = kPages[static_cast<std::size_t>(m_page)];
      return emptyState(info.glyph, i18n::tr(info.title), i18n::tr("link.errors.later-step"), s);
    }
    if (m_client->feed().items().empty()) {
      return emptyState("bell", i18n::tr("link.waiting.empty-title"), i18n::tr("link.waiting.empty-body"), s);
    }
    return waiting(renderer, width, height);
  }

  // A card as split cards look (design-split): window card radius, stroke edge, a 40 caption that moves the window.
  std::unique_ptr<Node> LinkApp::card(std::vector<std::unique_ptr<Node>> caption, std::unique_ptr<Node> body, float width,
                                      bool windowButtons) {
    const float s = m_scale;
    const float radius = Style::scaledRadius(Tokens::componentWindowCardRadius, s);
    const float height = Tokens::componentWindowCaption * s;
    auto box = ui::column({.align = FlexAlign::Stretch, .fill = Style::windowGlassFill(),
                           .radius = radius, .border = colorSpecFromRole(ColorRole::Stroke),
                           .borderWidth = Tokens::componentSplitStroke * s, .minWidth = width, .maxWidth = width, .width = width});
    box->setClipRadii(Radii{radius, radius, radius, radius});
    auto cap = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .width = width, .height = height});
    cap->setPadding(0.0F, Tokens::spaceP8 * s, 0.0F, Tokens::spaceP16 * s);
    for (auto& part : caption) cap->addChild(std::move(part));
    if (windowButtons) cap->addChild(m_window->buildWindowButtons());
    // A drag on the caption moves the window, as the header does.
    struct Press {
      bool down = false;
      std::uint32_t serial = 0;
      float x = 0.0F;
      float y = 0.0F;
    };
    auto press = std::make_shared<Press>();
    auto area = ui::inputArea({.width = width, .height = height});
    area->setOnPress([press](const InputArea::PointerData& data) {
      if (data.button == BTN_LEFT) *press = {.down = data.pressed, .serial = data.serial, .x = data.localX, .y = data.localY};
    });
    area->setOnMotion([this, press, s](const InputArea::PointerData& data) {
      if (press->down && std::hypot(data.localX - press->x, data.localY - press->y) > Tokens::spaceP4 * s) {
        press->down = false;
        m_window->beginMove(press->serial);
      }
    });
    area->addChild(std::move(cap));
    box->addChild(std::move(area));
    box->addChild(std::move(body));
    return box;
  }

  // A page's search field (pages.html .field): card fill with a stroke, on the shared query.
  std::unique_ptr<Flex> LinkApp::pageSearch(const std::string& placeholder, float s) {
    auto field = ui::searchCapsule({.out = &m_pageSearchInput, .placeholder = placeholder, .height = Tokens::sizeControlM, .scale = s,
                                    .value = m_query, .clearButton = true,
                                    .onChange = [this](const std::string& query) {
                                      m_searchCaret = m_pageSearchInput != nullptr ? m_pageSearchInput->caret() : query.size();
                                      setQuery(query);
                                    },
                                    .onKeyEvent = [this](std::uint32_t sym, std::uint32_t) {
                                      if (sym != XKB_KEY_Escape || m_query.empty()) return false;
                                      setQuery({});
                                      return true;
                                    }});
    field->setFill(colorSpecFromRole(ColorRole::Card));
    field->setBorder(colorSpecFromRole(ColorRole::Separator), s);
    m_pageSearchInput->inputArea()->setTabFocusKey("link-page-search");
    m_pageSearchInput->selectRange(m_searchCaret, m_searchCaret);
    return field;
  }

  bool LinkApp::splitPage() const {
    if (m_client == nullptr || pairShown()) return false;
    if (m_page != Page::Messages && (m_page != Page::Calls || m_client->callLog() != CallLog::Ready)) return false;
    const Presence::Kind kind = m_client->presence().kind;
    return kind == Presence::Kind::Connected || kind == Presence::Kind::Locked;
  }

  // Messages (pages.html .split): the thread pane with "Search texts" and new message, a line, then the conversation:
  // head (avatar, name, number, call), bubbles, reply bar. The split fills the sheet edge to edge.
  std::unique_ptr<Node> LinkApp::messages(float width, float height) {
    const float s = m_scale;
    const Messages& messages = m_client->messages();
    const float line = s;
    const float listWidth = std::clamp(width * Tokens::componentLinkThreadListShare, Tokens::componentLinkThreadListMin * s,
                                       Tokens::componentLinkThreadList * s);
    auto split = ui::row({.align = FlexAlign::Stretch, .gap = Tokens::spaceP16 * s, .width = width, .height = height});

    auto pane = ui::column({.align = FlexAlign::Stretch, .width = listWidth - line, .height = height});
    pane->setPadding(Tokens::spaceP16 * s, Tokens::spaceP12 * s, 0.0F, Tokens::spaceP12 * s);
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    bar->setPadding(0.0F, Tokens::spaceP4 * s, Tokens::spaceP16 * s, Tokens::spaceP4 * s);
    auto field = pageSearch(i18n::tr("link.messages.search"), s);
    field->setFlexGrow(1.0F);
    bar->addChild(std::move(field));
    bar->addChild(iconKey("edit", i18n::tr("link.messages.new-tooltip"), false, Tokens::sizeControlM * s, s, [this] { openCompose({}); }));
    pane->addChild(std::move(bar));
    if (messages.load() == Messages::Load::Loading) {
      pane->addChild(ui::spacer());
    } else if (messages.threads().empty()) {
      auto empty = emptyState("message", i18n::tr("link.messages.empty-title"), i18n::tr("link.messages.empty-body"), s);
      empty->setFlexGrow(1.0F);
      pane->addChild(std::move(empty));
    } else {
      if (m_search) {
        auto scope = ui::row({.paddingV = Tokens::spaceP4 * s, .paddingH = Tokens::spaceP8 * s});
        scope->addChild(text(i18n::tr("link.messages.search-scope", "count", messages.threads().size()), Tokens::typeFootnote,
                             ColorRole::Label2, s));
        pane->addChild(std::move(scope));
      }
      pane->addChild(ui::virtualListView({.state = &m_threadScroll, .contentScale = s, .adapter = m_threads.get(), .flexGrow = 1.0F}));
    }
    auto edge = ui::row({.align = FlexAlign::Stretch, .width = listWidth, .height = height});
    edge->addChild(std::move(pane));
    edge->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .width = line, .height = height}));
    split->addChild(std::move(edge));

    auto conv = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    const Thread* thread = m_thread ? messages.thread(*m_thread) : nullptr;
    if (m_compose || thread != nullptr) {
      auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s});
      head->setPadding(Tokens::spaceP16 * s, Tokens::spaceP20 * s, Tokens::spaceP12 * s, Tokens::spaceP20 * s);
      if (m_compose) {
        head->addChild(text(i18n::tr("link.messages.new-title"), Tokens::typeTitle3, ColorRole::Label1, s));
      } else {
        head->addChild(initialsAvatar(thread->name, s));
        auto who = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
        who->addChild(text(thread->name, Tokens::typeTitle3, ColorRole::Label1, s));
        if (thread->address != thread->name) who->addChild(text(thread->address, Tokens::typeFootnote, ColorRole::Label2, s));
        head->addChild(std::move(who));
        const std::string number = thread->address;
        const std::string device = m_client->presence().phone.id;
        head->addChild(iconKey("phone", i18n::tr("link.common.call"), false, Tokens::sizeControlM * s, s,
                               [this, device, number] { m_client->dial(device, number); }));
      }
      conv->addChild(std::move(head));
      conv->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .height = line}));
    }
    if (m_compose) {
      conv->addChild(compose(s));
    } else if (thread == nullptr) {
      auto empty = emptyState("message", i18n::tr("link.messages.none-title"), i18n::tr("link.messages.none-body"), s);
      empty->setFlexGrow(1.0F);
      conv->addChild(std::move(empty));
    } else {
      m_conversation->setRows(messages.conversation(thread->id, nowMs()));
      auto log = ui::column({.align = FlexAlign::Stretch, .paddingV = Tokens::spaceP12 * s, .flexGrow = 1.0F});
      VirtualListView* list = nullptr;
      log->addChild(ui::virtualListView({.out = &list, .state = &m_conversationScroll, .contentScale = s,
                                         .itemGap = Tokens::spaceP4 * s, .adapter = m_conversation.get(), .flexGrow = 1.0F}));
      m_conversationList = list;
      list->scrollView().setStickToBottom(true);
      list->setAnchorBottom(true);
      conv->addChild(std::move(log));
      const std::uint64_t id = thread->id;
      const auto send = [this, id] {
        const std::string value = m_threadDrafts[id];
        if (value.empty()) return;
        m_threadDrafts.erase(id);
        m_client->sendText(id, value);
      };
      conv->addChild(replyBar(m_threadDrafts[id], i18n::tr("link.messages.placeholder"), s,
                              [this, id](const std::string& value) { m_threadDrafts[id] = value; }, send));
    }
    split->addChild(std::move(conv));
    return split;
  }

  // New message: a "To" field with contact matches under it, the text capsule at the bottom.
  std::unique_ptr<Node> LinkApp::compose(float s) {
    const Messages& messages = m_client->messages();
    auto column = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    auto to = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingV = Tokens::spaceP8 * s,
                       .paddingH = Tokens::spaceP20 * s});
    to->addChild(text(i18n::tr("link.messages.to"), Tokens::typeBody, ColorRole::Label2, s));
    to->addChild(ui::input({.value = m_compose->to, .placeholder = i18n::tr("link.messages.name-or-number"), .fontSize = Tokens::typeBody.size * s,
                            .frameVisible = false, .flexGrow = 1.0F,
                            .onChange = [this](const std::string& value) {
                              m_compose->to = value;
                              m_compose->sent.clear();
                              m_window->requestRebuild();
                            },
                            .configure = [](Input& input) {
                              if (input.inputArea() != nullptr) input.inputArea()->setTabFocusKey("link-compose-to");
                            }}));
    column->addChild(std::move(to));
    column->addChild(ui::separator({}));
    auto list = ui::column({.align = FlexAlign::Stretch, .padding = Tokens::spaceP4 * s, .flexGrow = 1.0F});
    constexpr std::size_t kMatches = 6;
    for (const Contact& contact : messages.matches(m_compose->to, kMatches)) {
      const std::string number = contact.numbers.front();
      auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingH = Tokens::spaceP12 * s,
                          .height = Tokens::sizeControlL * s});
      row->addChild(initialsAvatar(contact.name, s));
      auto name = text(contact.name, Tokens::typeBody, ColorRole::Label1, s);
      name->setFlexGrow(1.0F);
      row->addChild(std::move(name));
      row->addChild(text(number, Tokens::typeFootnote, ColorRole::Label2, s));
      auto pick = ui::inputArea({.onClick = [this, number](const InputArea::PointerData&) { openCompose(number); }});
      pick->addChild(std::move(row));
      list->addChild(std::move(pick));
    }
    if (!m_compose->sent.empty()) {
      list->addChild(ui::row({.paddingV = Tokens::spaceP8 * s, .paddingH = Tokens::spaceP12 * s},
                             text(i18n::tr("link.messages.sent-to", "to", m_compose->sent), Tokens::typeFootnote, ColorRole::Label2, s)));
    }
    column->addChild(std::move(list));
    const auto send = [this] {
      const std::string number = m_client->messages().numberFor(m_compose->to);
      if (number.empty() || m_compose->body.empty()) return;
      m_client->sendTo(number, std::exchange(m_compose->body, {}));
      m_compose->sent = number;
      m_window->requestRebuild();
    };
    column->addChild(replyBar(m_compose->body, i18n::tr("link.messages.placeholder"), s,
                              [this](const std::string& value) { m_compose->body = value; }, send));
    return column;
  }

  std::uint64_t FileAdapter::itemKey(std::size_t index) const {
    return std::hash<std::string>{}(m_client.files().entries()[at(index)].name);
  }

  float FileAdapter::measureItem(Renderer& /*renderer*/, std::size_t /*index*/, float /*width*/) {
    return Tokens::sizeControlL * m_scale;
  }

  std::unique_ptr<Node> FileAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  // File row (pages.html .tbl .tr2): glyph | name | size | modified | more, h 40, r 8, every other row in card.
  void FileAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool hovered) {
    const float s = m_scale;
    const Files& files = m_client.files();
    const FileEntry& entry = files.entries()[at(index)];
    const ColorSpec fill = hovered ? colorSpecFromRole(ColorRole::Fill1) : index % 2 == 0 ? colorSpecFromRole(ColorRole::Card) : clearColorSpec();
    auto row = fileColumns(s, fill, Tokens::sizeControlL * s);
    const ColorSpec folderInk = isResolvedLightTheme() ? fixedColorSpec(rgbaHex(Tokens::componentLinkCoreTextLight)) : core();
    row->addChild(ui::row({.align = FlexAlign::Center, .width = Tokens::componentLinkFileSymbol * s},
                          ui::glyph({.glyph = std::string(entry.folder ? "folder" : fileGlyph(entry.name)), .glyphSize = Tokens::sizeSymbol * s,
                                     .color = entry.folder ? folderInk : colorSpecFromRole(ColorRole::Label2)})));
    auto name = text(entry.name, Tokens::typeBody, ColorRole::Label1, s);
    name->setFlexGrow(1.0F);
    row->addChild(std::move(name));
    std::string size = entry.folder ? std::string("—") : byteSize(entry.size);
    ColorRole sizeRole = ColorRole::Label2;
    if (const FileSave* save = entry.folder ? nullptr : files.save(files.childPath(entry.name))) {
      switch (save->state) {
      case FileSave::State::Running:
        size = save->total > 0 ? i18n::tr("link.files.saving-percent", "percent", save->got * 100 / save->total) : i18n::tr("link.files.saving");
        break;
      case FileSave::State::Done: size = i18n::tr("link.files.in-downloads"); break;
      case FileSave::State::Failed:
        size = i18n::tr("link.files.not-saved");
        sizeRole = ColorRole::DangerText;
        break;
      }
    }
    const std::string modified = entry.mtimeS > 0 ? dayLabel(entry.mtimeS * 1000, nowMs()) + " " + clock(entry.mtimeS * 1000) : std::string();
    for (auto [value, role, column] : {std::tuple{size, sizeRole, Tokens::componentLinkFileColumn},
                                       std::tuple{modified, ColorRole::Label2, Tokens::componentLinkFileModified}}) {
      auto cell = text(value, Tokens::typeFootnote, role, s);
      cell->setTextAlign(TextAlign::End);
      row->addChild(ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End, .width = column * s}, std::move(cell)));
    }
    Button* more = nullptr;
    row->addChild(ui::button({.out = &more, .glyph = std::string("dots"), .glyphSize = Tokens::sizeSymbol * s, .controlHeight = Tokens::sizeControlM * s,
                              .variant = ButtonVariant::Ghost, .tooltip = i18n::tr("link.files.more"), .radius = Tokens::sizeControlM * s / 2.0F,
                              .width = Tokens::sizeControlL * s}));
    const std::size_t entryIndex = at(index);
    more->setOnClick([this, more, entryIndex] {
      float x = 0.0F;
      float y = 0.0F;
      Node::mapToScene(more, 0.0F, more->height(), x, y);
      m_menu(entryIndex, x, y);
    });
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(std::move(row));
  }

  void FileAdapter::onActivate(std::size_t index) { open(at(index)); }

  void FileAdapter::open(std::size_t entryIndex) {
    const Files& files = m_client.files();
    if (entryIndex >= files.entries().size()) return;
    const FileEntry& entry = files.entries()[entryIndex];
    if (entry.folder) {
      m_client.openFolder(files.childPath(entry.name));
      return;
    }
    const FileSave* save = files.save(files.childPath(entry.name));
    if (save != nullptr && save->state == FileSave::State::Done) {
      m_client.showInFolder({save->saved});
    } else {
      m_client.saveFile(entry.name);
    }
  }

  // Files (pages.html B.files): bar Back, breadcrumb, Send files here; the table Name | Size | Modified with the folder's
  // rows; a copy into the folder shows its progress under it.
  std::unique_ptr<Node> LinkApp::files(float width, float height) {
    const float s = m_scale;
    const Files& files = m_client->files();
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .minHeight = Tokens::sizeControlM * s});
    bar->setPadding(0.0F, 0.0F, Tokens::spaceP12 * s, 0.0F);
    if (files.path() != "/") {
      bar->addChild(iconKey("arrow-left", i18n::tr("link.common.back"), false, Tokens::sizeControlM * s, s,
                            [this] { m_client->openFolder(m_client->files().parent()); }));
    }
    auto crumbs = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP4 * s, .clipChildren = true, .flexGrow = 1.0F});
    const std::vector<std::string> names = files.crumbs();
    for (std::size_t i = 0; i < names.size(); ++i) {
      if (i > 0) {
        crumbs->addChild(ui::glyph({.glyph = std::string("chevron-right"), .glyphSize = Tokens::sizeSymbol * s,
                                    .color = colorSpecFromRole(ColorRole::Label2)}));
      }
      if (i + 1 == names.size()) {
        crumbs->addChild(text(names[i], Tokens::typeTitle3, ColorRole::Label1, s));
      } else {
        const Button::ButtonStateColors rest{.bg = clearColorSpec(), .border = clearColorSpec(), .label = colorSpecFromRole(ColorRole::Label2)};
        const Button::ButtonStateColors hot{.bg = clearColorSpec(), .border = clearColorSpec(), .label = colorSpecFromRole(ColorRole::Label1)};
        crumbs->addChild(ui::button({.text = names[i], .fontSize = Tokens::typeTitle3.size * s,
                                     .variant = ButtonVariant::Ghost,
                                     .customPalette = Button::ButtonPalette{.normal = rest, .hover = hot, .pressed = hot, .disabled = rest},
                                     .paddingV = 0.0F, .paddingH = 0.0F,
                                     .onClick = [this, i] { m_client->openFolder(m_client->files().crumbPath(i)); }}));
      }
    }
    bar->addChild(std::move(crumbs));
    bar->addChild(pill(i18n::tr("link.files.send-here"), std::string("upload"), true, s, [this] {
      chooseFiles([this](std::vector<std::filesystem::path> paths) { m_client->copyIntoFolder(paths); });
    }));

    std::unique_ptr<Node> body;
    if (files.load() == Files::Load::Failed) {
      body = emptyState("folder-off", i18n::tr("link.files.open-failed"), files.error(), s,
                        ui::button({.text = i18n::tr("link.common.try-again"), .variant = ButtonVariant::Secondary,
                                    .onClick = [this] { m_client->openFolder(m_client->files().path()); }}));
    } else if (files.load() == Files::Load::Ready && files.entries().empty()) {
      body = emptyState("folder", i18n::tr("link.files.empty-title"), i18n::tr("link.files.empty-body"), s);
    } else {
      auto table = ui::column({.align = FlexAlign::Stretch});
      auto head = fileColumns(s, clearColorSpec(), Tokens::sizeControlM * s);
      head->addChild(ui::row({.width = Tokens::componentLinkFileSymbol * s}));
      const auto heading = [s](const char* key) { return text(i18n::tr(key), Tokens::typeFootnote, ColorRole::Label3, s); };
      auto name = heading("link.files.name");
      name->setFlexGrow(1.0F);
      head->addChild(std::move(name));
      for (auto [key, column] : {std::pair{"link.files.size", Tokens::componentLinkFileColumn},
                                 std::pair{"link.files.modified", Tokens::componentLinkFileModified}}) {
        auto cell = heading(key);
        cell->setTextAlign(TextAlign::End);
        head->addChild(ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End, .width = column * s}, std::move(cell)));
      }
      head->addChild(ui::row({.width = Tokens::sizeControlL * s}));
      table->addChild(std::move(head));
      table->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .height = s}));
      if (!files.entries().empty()) {
        auto rows = ui::column({.align = FlexAlign::Stretch, .paddingV = Tokens::spaceP4 * s, .flexGrow = 1.0F});
        rows->addChild(ui::virtualListView({.state = &m_fileScroll, .contentScale = s, .adapter = m_files.get(), .flexGrow = 1.0F}));
        table->addChild(std::move(rows));
      }
      body = std::move(table);
    }
    body->setFlexGrow(1.0F);
    if (const FileOp& op = m_client->fileOp(); op.kind != FileOp::Kind::Idle) {
      auto column = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      column->addChild(std::move(body));
      column->addChild(ui::separator({.color = colorSpecFromRole(ColorRole::Separator), .thickness = s}));
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                          .paddingH = Tokens::spaceP12 * s, .minHeight = Tokens::sizeControlL * s});
      const bool copying = op.kind == FileOp::Kind::Copying;
      std::string line = op.error;
      if (copying) {
        line = i18n::tr("link.files.copying", "name", op.name);
        if (op.count > 1) line += i18n::tr("link.files.copying-of", "index", op.index + 1, "count", op.count);
        if (op.total > 0) line += std::format(" · {} %", op.sent * 100 / op.total);
      }
      auto words = text(line, Tokens::typeFootnote, copying ? ColorRole::Label2 : ColorRole::DangerText, s, 2);
      words->setFlexGrow(1.0F);
      bar->addChild(std::move(words));
      bar->addChild(ui::button({.text = i18n::tr(copying ? "common.actions.cancel" : "link.files.dismiss"), .variant = ButtonVariant::Ghost,
                                .onClick = [this, copying] { copying ? m_client->cancelCopy() : m_client->dismissFileOp(); }}));
      column->addChild(std::move(bar));
      body = std::move(column);
    }
    auto page = ui::column({.align = FlexAlign::Stretch, .width = width, .height = height});
    page->addChild(std::move(bar));
    page->addChild(std::move(body));
    return page;
  }

  std::optional<std::string> LinkApp::photoStep(int delta) const {
    if (!m_viewing) return std::nullopt;
    const auto& items = m_client->photos().items();
    const auto it = std::ranges::find(items, *m_viewing, &Photo::path);
    if (it == items.end()) return std::nullopt;
    const auto item = static_cast<std::size_t>(it - items.begin());
    if (m_search) {
      const auto& order = m_search->photos;
      const auto hit = std::ranges::find(order, item);
      if (hit == order.end()) return std::nullopt; // the shown photo is not a hit of the new query
      const auto pos = static_cast<std::ptrdiff_t>(hit - order.begin()) + delta;
      if (pos < 0 || pos >= static_cast<std::ptrdiff_t>(order.size())) return std::nullopt;
      return items[order[static_cast<std::size_t>(pos)]].path;
    }
    const auto pos = static_cast<std::ptrdiff_t>(item) + delta;
    if (pos < 0 || pos >= static_cast<std::ptrdiff_t>(items.size())) return std::nullopt;
    return items[static_cast<std::size_t>(pos)].path;
  }

  void LinkApp::viewPhoto(std::optional<std::string> path) {
    m_viewing = std::move(path);
    m_viewImage = {};
    const auto& items = m_client->photos().items();
    const auto it = m_viewing ? std::ranges::find(items, *m_viewing, &Photo::path) : items.end();
    const Presence::Kind kind = m_client->presence().kind;
    const bool live = kind == Presence::Kind::Connected || kind == Presence::Kind::Locked;
    // A video shows its poster: the viewer reads only photos in full, and only from a connected phone.
    if (it != items.end() && !it->videoS && live) {
      m_client->openPhoto(*m_viewing);
    } else {
      m_client->closePhoto();
    }
    if (it != items.end()) {
      // Back in the grid, the last shown photo is in view.
      const auto item = static_cast<std::size_t>(it - items.begin());
      if (!m_search) {
        m_photoScrollTo = item;
      } else if (const auto hit = std::ranges::find(m_search->photos, item); hit != m_search->photos.end()) {
        m_photoScrollTo = static_cast<std::size_t>(hit - m_search->photos.begin());
      }
    }
    if (m_window != nullptr) m_window->requestRebuild();
  }

  void LinkApp::copyPhoto() {
    const std::optional<PhotoFile>& file = m_client->photoFile();
    if (!file || file->kind != PhotoFile::Kind::Ready) return;
    std::ifstream in(file->local, std::ios::binary);
    auto bytes = std::make_shared<std::string>(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
    std::string ext = std::filesystem::path(file->phonePath).extension().string();
    std::ranges::transform(ext, ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    const std::string mime = ext == ".png" ? "image/png" : ext == ".webp" ? "image/webp" : ext == ".heic" ? "image/heic"
                           : ext == ".gif" ? "image/gif" : "image/jpeg";
    Display& display = loop().display();
    (void)display.setClipboard(display.lastInputSerial(), {mime}, [bytes](const std::string&) -> std::string { return *bytes; });
  }

  // PhotoLightbox (design-link-parity handoff.yaml): the dark stage (the Mirror's), the photo contain-fit with P24
  // around it, the toolbar capsule: Previous, Next | name over "time · size · W × H" | Copy, Save.
  std::unique_ptr<Node> LinkApp::viewer(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const auto& items = m_client->photos().items();
    const auto it = std::ranges::find(items, *m_viewing, &Photo::path);
    auto stage = ui::column({.align = FlexAlign::Center, .justify = FlexJustify::Center, .gap = Tokens::spaceP16 * s,
                             .fill = colorSpecFromRole(ColorRole::View), .flexGrow = 1.0F});
    stage->setPaletteTone(PaletteTone::Dark);
    if (it == items.end()) return stage;
    const Photo& photo = *it;
    const std::optional<PhotoFile>& file = m_client->photoFile();
    const bool failed = file && file->phonePath == photo.path && file->kind == PhotoFile::Kind::Failed;
    const bool ready = file && file->phonePath == photo.path && file->kind == PhotoFile::Kind::Ready;
    const float barH = Tokens::sizeControlXl * s;
    const float pad = Tokens::spaceP24 * s;
    const float boxW = std::max(1.0F, width - 2.0F * pad);
    const float boxH = std::max(1.0F, height - Tokens::componentWindowCaption * s - barH - 3.0F * pad);
    if (failed) {
      stage->addChild(emptyState("photo-off", i18n::tr("link.photos.load-failed-title"), i18n::tr("link.photos.load-failed-body"), s,
                                 ui::button({.text = i18n::tr("link.common.try-again"), .variant = ButtonVariant::Secondary,
                                             .onClick = [this, path = photo.path] { m_client->closePhoto(); viewPhoto(path); }})));
    } else {
      Image* image = nullptr;
      stage->addChild(ui::image({.out = &image, .fit = ImageFit::Contain, .width = boxW, .height = boxH}));
      // The grid's thumbnail scaled up until the full image decodes.
      const std::vector<std::uint8_t>* png = m_client->thumbnail(photo.path);
      if (png != nullptr) (void)image->setSourceBytes(renderer, png->data(), png->size());
      if (ready) {
        const int side = static_cast<int>(std::max(boxW, boxH));
        if (!image->setSourceFileAsync(renderer, m_viewCache, file->local, side) && png == nullptr) image->clear(renderer);
        if (image->sourceWidth() > 0) m_viewImage = {image->sourceWidth(), image->sourceHeight()};
        image->setAsyncReadyCallback([this, image] {
          const std::pair<int, int> size{image->sourceWidth(), image->sourceHeight()};
          if (size == m_viewImage || m_window == nullptr) return;
          m_viewImage = size;
          m_window->requestRebuild();
        });
      }
    }

    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::Popover),
                        .radius = barH / 2.0F, .height = barH});
    bar->setPadding(0.0F, Tokens::spaceP8 * s, 0.0F, Tokens::spaceP8 * s);
    const auto key = [&](const char* glyph, const char* tip, bool enabled, std::function<void()> click) {
      bar->addChild(ui::button({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s, .enabled = enabled,
                                .variant = ButtonVariant::Ghost, .tooltip = i18n::tr(tip), .onClick = std::move(click)}));
    };
    const auto previous = photoStep(-1);
    const auto next = photoStep(1);
    key("chevron-left", "link.player.previous", previous.has_value(), [this, previous] { viewPhoto(previous); });
    key("chevron-right", "link.player.next", next.has_value(), [this, next] { viewPhoto(next); });
    bar->addChild(ui::separator({.orientation = SeparatorOrientation::VerticalRule, .height = Tokens::sizeControlS * s}));
    auto names = ui::column({.align = FlexAlign::Start, .paddingH = Tokens::spaceP8 * s, .maxWidth = std::max(0.0F, width / 2.0F)});
    names->addChild(text(photo.name, Tokens::typeBody, ColorRole::Label1, s));
    std::string info = std::format("{} {}", dayLabel(photo.atS * 1000, nowMs()), when(photo.atS * 1000));
    if (photo.videoS) info += std::format(" · {}:{:02}", *photo.videoS / 60, *photo.videoS % 60);
    if (ready) info += " · " + byteSize(file->size);
    if (ready && m_viewImage.first > 0) info += std::format(" · {} × {}", groupedCount(static_cast<std::uint64_t>(m_viewImage.first)),
                                                            groupedCount(static_cast<std::uint64_t>(m_viewImage.second)));
    names->addChild(text(info, Tokens::typeFootnote, ColorRole::Label2, s));
    bar->addChild(std::move(names));
    bar->addChild(ui::separator({.orientation = SeparatorOrientation::VerticalRule, .height = Tokens::sizeControlS * s}));
    if (!photo.videoS) key("copy", "desk.edit.copy", ready, [this] { copyPhoto(); });
    const auto saved = m_viewSaved.find(photo.path);
    if (saved != m_viewSaved.end()) {
      bar->addChild(ui::button({.text = i18n::tr("link.common.show-in-folder"), .glyph = std::string("folder"), .variant = ButtonVariant::Tonal,
                                .onClick = [this, local = saved->second] { m_client->showInFolder({local}); }}));
    } else {
      const Presence::Kind kind = m_client->presence().kind;
      const bool live = kind == Presence::Kind::Connected || kind == Presence::Kind::Locked;
      bar->addChild(ui::button({.text = i18n::tr("link.common.save"), .glyph = std::string("download"), .enabled = live,
                                .variant = ButtonVariant::Tonal,
                                .onClick = [this, path = photo.path] {
                                  m_client->savePhoto(path, [this, path](const std::string& local) {
                                    if (local.empty()) return;
                                    m_viewSaved[path] = local;
                                    if (m_window != nullptr) m_window->requestRebuild();
                                  });
                                }}));
    }
    stage->addChild(std::move(bar));
    return stage;
  }

  // Calls (handoff.yaml): dial row (capsule "Name or number" + Primary Call), then the log grouped Today / Earlier.
  std::unique_ptr<Node> PhotoAdapter::createTile() {
    auto tile = std::make_unique<Node>();
    m_shown.erase(tile.get()); // a tile of an earlier grid may have had this address
    tile->addChild(ui::image({.fit = ImageFit::Cover}));
    // pages.html .pg .sel: an inset core ring and a core check badge at the top right.
    tile->addChild(ui::box({.fill = clearColorSpec(), .border = core(), .borderWidth = 0.0F}));
    tile->addChild(ui::box({.fill = core()}));
    tile->addChild(ui::glyph({.glyph = std::string("check"), .color = fixedColorSpec(rgbaHex(Tokens::componentLinkCoreInk))}));
    // A video's length at the bottom left (pages.html .pg .vid): caption in white on a shade plate.
    tile->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Shadow, Tokens::componentLinkVideoShade)}));
    tile->addChild(ui::label({.fontSize = Tokens::typeCaption.size * m_scale, .color = colorSpecFromRole(ColorRole::OnAccent),
                              .maxLines = 1}));
    return tile;
  }

  namespace {
    // The corner check of a tile (top right, inset 8), with an inset-8 margin around it as its hit area.
    bool inCheck(float x, float y, float width, float /*height*/, float s) {
      const float reach = (Tokens::componentLinkTileCheck + 2.0F * Tokens::spaceP8) * s;
      return x >= width - reach && y <= reach;
    }
  } // namespace

  void PhotoAdapter::onActivate(std::size_t index) {
    if (m_client.photos().selectedCount() > 0) {
      m_client.togglePhoto(at(index));
    } else {
      m_open(at(index));
    }
  }

  bool PhotoAdapter::onPointerPress(std::size_t index, float x, float y, float width, float height) {
    if (!inCheck(x, y, width, height, m_scale)) return false;
    m_client.togglePhoto(at(index));
    return true;
  }

  void PhotoAdapter::bindTile(Node& tile, std::size_t index, bool /*selected*/, bool hovered) {
    const float s = m_scale;
    const auto& kids = tile.children();
    auto* image = static_cast<Image*>(kids[0].get());
    auto* ring = static_cast<Box*>(kids[1].get());
    auto* badge = static_cast<Box*>(kids[2].get());
    auto* check = static_cast<Glyph*>(kids[3].get());
    auto* plate = static_cast<Box*>(kids[4].get());
    auto* length = static_cast<Label*>(kids[5].get());
    const float w = tile.width();
    const float h = tile.height();
    const bool picked = m_client.photos().selected(at(index));
    const float radius = Style::scaledRadius(Tokens::radiusR4, s);
    image->setPosition(0.0F, 0.0F);
    image->setSize(w, h);
    image->setRadius(radius);
    ring->setVisible(picked);
    ring->setBorder(core(), Tokens::componentLinkTileRing * s);
    ring->setPosition(0.0F, 0.0F);
    ring->setSize(w, h);
    ring->setRadius(radius);
    const float d = Tokens::componentLinkTileCheck * s;
    const float inset = Tokens::spaceP8 * s;
    // A hovered tile shows its empty check: a press there selects without opening the viewer.
    badge->setVisible(picked || hovered);
    badge->setFill(picked ? core() : colorSpecFromRole(ColorRole::Shadow, Tokens::componentLinkTileCheckShade));
    badge->setBorder(picked ? clearColorSpec() : colorSpecFromRole(ColorRole::OnAccent), Tokens::componentLinkTileCheckEdge * s);
    badge->setPosition(w - inset - d, inset);
    badge->setSize(d, d);
    badge->setRadius(d / 2.0F);
    check->setVisible(picked);
    check->setGlyphSize(Tokens::sizeSymbol * s * 0.75F);
    if (m_renderer != nullptr) check->layout(*m_renderer); // its size is known only once measured
    check->setPosition(badge->x() + (d - check->width()) / 2.0F, badge->y() + (d - check->height()) / 2.0F);
    const Photo& photo = m_client.photos().items()[at(index)];
    plate->setVisible(photo.videoS.has_value());
    length->setVisible(photo.videoS.has_value());
    if (photo.videoS) {
      length->setText(std::format("{}:{:02}", *photo.videoS / 60, *photo.videoS % 60));
      if (m_renderer != nullptr) length->measure(*m_renderer);
      const float padH = Tokens::spaceP4 * s;
      const float plateH = Tokens::typeCaption.line * s;
      plate->setPosition(inset, h - inset - plateH);
      plate->setSize(length->width() + 2.0F * padH, plateH);
      plate->setRadius(plateH / 2.0F);
      length->setPosition(inset + padH, h - inset - plateH);
    }
    if (!m_search && index + Photos::kCount / 2 >= itemCount()) m_client.loadMorePhotos(); // a search covers loaded pages
    const std::string& path = photo.path;
    std::string& shown = m_shown[&tile];
    if (shown == path || m_renderer == nullptr) return;
    const std::vector<std::uint8_t>* png = m_client.thumbnail(path);
    if (png == nullptr) {
      image->clear(*m_renderer);
      shown.clear();
      return;
    }
    if (image->setSourceBytes(*m_renderer, png->data(), png->size())) shown = path;
  }

  std::string PhotoAdapter::itemTooltip(std::size_t index) const {
    return m_client.photos().items()[at(index)].name;
  }

  std::unique_ptr<Node> PhotoAdapter::createSectionHeader() {
    auto header = ui::column({.align = FlexAlign::Start, .justify = FlexJustify::End});
    header->setPadding(Tokens::spaceP12 * m_scale, 0.0F, Tokens::spaceP8 * m_scale, Tokens::spaceP2 * m_scale);
    header->addChild(ui::label({.fontSize = Tokens::typeFootnote.size * m_scale, .fontWeight = static_cast<FontWeight>(600),
                                .color = colorSpecFromRole(ColorRole::Label3), .maxLines = 1}));
    return header;
  }

  void PhotoAdapter::bindSectionHeader(Node& header, std::size_t section, float /*width*/, float /*height*/) {
    if (section < m_days.size()) static_cast<Label*>(header.children()[0].get())->setText(m_days[section].second);
  }

  // Photos (pages.html B.photos): bar (All / Photos / Videos, the count, Take Photo, "Search by name"), the grid under
  // day heads, and the floating selection bar over the grid's foot.
  std::unique_ptr<Node> LinkApp::photos(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const Photos& photos = m_client->photos();
    auto page = ui::column({.align = FlexAlign::Stretch, .width = width, .height = height});
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .minHeight = Tokens::sizeControlM * s});
    bar->setPadding(0.0F, 0.0F, Tokens::spaceP12 * s, 0.0F);
    if (m_viewing) {
      bar->addChild(iconKey("chevron-left", i18n::tr("link.photos.back"), false, Tokens::sizeControlM * s, s,
                            [this] { viewPhoto(std::nullopt); }));
      const std::vector<std::size_t>* order = m_search ? &m_search->photos : nullptr;
      const auto& items = photos.items();
      const auto at = std::ranges::find(items, *m_viewing, &Photo::path);
      const auto index = static_cast<std::size_t>(at - items.begin());
      if (order != nullptr && std::ranges::find(*order, index) == order->end()) order = nullptr; // not a hit: place among all
      const std::size_t shown = order ? static_cast<std::size_t>(std::ranges::find(*order, index) - order->begin()) : index;
      bar->addChild(text(i18n::tr("link.photos.place", "index", groupedCount(shown + 1), "count",
                                  groupedCount(order ? order->size() : photos.total())),
                         Tokens::typeBody, ColorRole::Label2, s));
    } else {
      bar->addChild(ui::segmented({.options = std::vector<ui::SegmentedOption>{{.label = i18n::tr("link.photos.all")},
                                                                               {.label = i18n::tr("link.pages.photos")},
                                                                               {.label = i18n::tr("link.photos.videos")}},
                                   .selectedIndex = static_cast<std::size_t>(photos.media()), .scale = s, .compact = true,
                                   .surfaceRole = ColorRole::Card,
                                   .onChange = [this](std::size_t i) { m_client->setPhotoMedia(static_cast<Photos::Media>(i)); },
                                   .configure = [](Segmented& seg) { seg.setThumbRoles(ColorRole::Label1, ColorRole::View); }}));
      if (photos.total() > 0) {
        auto count = ui::row({.paddingH = Tokens::spaceP8 * s},
                             text(i18n::tr(photos.total() == 1 ? "link.photos.item" : "link.photos.items", "count", groupedCount(photos.total())),
                                  Tokens::typeBody, ColorRole::Label2, s));
        bar->addChild(std::move(count));
      }
      bar->addChild(ui::row({.flexGrow = 1.0F}));
      if (m_client->takingPhoto()) {
        bar->addChild(text(i18n::tr("link.photos.waiting"), Tokens::typeBody, ColorRole::Label2, s));
        bar->addChild(pill(i18n::tr("common.actions.cancel"), std::nullopt, false, s, [this] { m_client->cancelTakePhoto(); }));
      } else if (m_client->presence().kind == Presence::Kind::Connected) {
        bar->addChild(pill(i18n::tr("link.photos.take"), std::string("camera"), false, s, [this] { m_client->takePhoto(); }));
      }
      auto field = pageSearch(i18n::tr("link.photos.search"), s);
      const float searchWidth = Tokens::componentLinkPhotoSearch * s;
      field->setMinWidth(searchWidth);
      field->setMaxWidth(searchWidth);
      bar->addChild(std::move(field));
    }
    page->addChild(std::move(bar));
    const auto filled = [&](std::unique_ptr<Node> body) {
      body->setFlexGrow(1.0F);
      page->addChild(std::move(body));
      return std::move(page);
    };
    const auto retry = [this] {
      return ui::button({.text = i18n::tr("link.common.try-again"), .variant = ButtonVariant::Secondary,
                         .onClick = [this] { m_client->loadPhotos(); }});
    };
    const bool videos = photos.media() == Photos::Media::Videos;
    switch (photos.load()) {
    case Photos::Load::Idle:
    case Photos::Load::Loading:
      if (photos.items().empty()) {
        return filled(emptyState("photo", i18n::tr("link.photos.loading-title"), i18n::tr("link.photos.loading-body"), s));
      }
      break;
    case Photos::Load::Denied:
      return filled(emptyState("photo-off", i18n::tr("link.photos.denied-title"), i18n::tr("link.photos.denied-body"), s, retry()));
    case Photos::Load::Unsupported:
      return filled(emptyState("photo-off", i18n::tr("link.photos.unsupported-title"), i18n::tr("link.photos.unsupported-body"), s));
    case Photos::Load::Failed:
      return filled(emptyState("alert-circle", i18n::tr("link.photos.failed-title"), i18n::tr("link.photos.failed-body"), s, retry()));
    case Photos::Load::More:
    case Photos::Load::Ready:
      if (photos.items().empty()) {
        return filled(videos ? emptyState("video", i18n::tr("link.photos.no-videos-title"), i18n::tr("link.photos.no-videos-body"), s)
                             : emptyState("photo", i18n::tr("link.photos.none-title"), i18n::tr("link.photos.none-body"), s));
      }
      break;
    }
    if (m_viewing) return filled(viewer(renderer, width, height));
    m_photos->setRenderer(&renderer);
    auto days = m_search ? std::vector<std::pair<std::size_t, std::string>>{{0, std::format("{} of {} loaded", m_search->photos.size(), photos.items().size())}}
                         : photos.days(nowMs());
    std::vector<std::size_t> starts;
    starts.reserve(days.size());
    for (const auto& day : days) starts.push_back(day.first);
    m_photos->setDays(std::move(days));
    const float headerH = (Tokens::spaceP12 + Tokens::typeFootnote.line + Tokens::spaceP8) * s;
    const float gap = Tokens::spaceP4 * s;
    page->addChild(ui::virtualGridView({.out = &m_photoGrid, .state = &m_photoScroll, .contentScale = s,
                                        .minCellWidth = Tokens::componentLinkPhotoMin * s, .squareCells = true, .columnGap = gap,
                                        .rowGap = gap, .overscanRows = 2, .itemCursorShape = CursorShape::Pointer,
                                        .adapter = m_photos.get(), .flexGrow = 1.0F}));
    m_photoGrid->setSections(std::move(starts), headerH);
    if (m_photoScrollTo) m_photoGrid->scrollToIndex(*std::exchange(m_photoScrollTo, std::nullopt));

    // Selection bar (pages.html .fbar): "<n> selected", Clear, Save to Downloads; or how the last save went.
    std::string what;
    if (photos.selectedCount() > 0) {
      what = i18n::tr("link.photos.selected", "count", photos.selectedCount());
    } else if (photos.save() == Photos::Save::Saving) {
      what = i18n::tr("link.photos.saving");
    } else if (photos.save() == Photos::Save::Done) {
      what = photos.saveFailed() == 0 ? i18n::tr("link.photos.saved", "count", photos.saved())
                                      : i18n::tr("link.photos.not-saved", "count", photos.saveFailed());
    }
    if (!what.empty()) {
      const float h = Tokens::sizeControlXl * s;
      auto selection = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::Popover),
                                .radius = h / 2.0F, .border = colorSpecFromRole(ColorRole::Separator), .borderWidth = s, .height = h});
      selection->setPadding(0.0F, Tokens::spaceP8 * s, 0.0F, Tokens::spaceP16 * s);
      selection->addChild(text(what, Tokens::typeBody, ColorRole::Label1, s));
      if (photos.selectedCount() > 0) {
        selection->addChild(pill(i18n::tr("link.photos.clear"), std::string("x"), false, s, [this] { m_client->clearPhotoSelection(); }));
        if (photos.save() != Photos::Save::Saving) {
          selection->addChild(pill(i18n::tr("link.photos.save"), std::string("download"), true, s, [this] { m_client->savePhotos(); }));
        }
      }
      // Over the grid: out of the page's layout, laid out alone to learn its width.
      selection->setParticipatesInLayout(false);
      selection->layout(renderer);
      selection->setPosition((width - selection->width()) / 2.0F, height - selection->height());
      page->addChild(std::move(selection));
    }
    return page;
  }

  // Calls (pages.html B.calls): a pane with "Name or number" + Call and the log under day heads, a line, then the shown
  // call's contact: avatar, name, number, Text + Call back, its newest calls, where the audio goes.
  std::unique_ptr<Node> LinkApp::calls(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    switch (m_client->callLog()) {
    case CallLog::Off:
      return emptyState("phone-off", i18n::tr("link.calls.off-title"), i18n::tr("link.calls.off-body"), s);
    case CallLog::Denied:
      return emptyState("phone-off", i18n::tr("link.calls.denied-title"), i18n::tr("link.calls.denied-body"), s);
    case CallLog::Failed:
      return emptyState("alert-circle", i18n::tr("link.calls.failed-title"), i18n::tr("link.calls.failed-body"), s);
    case CallLog::Ready: break;
    }
    const auto& log = m_client->calls();
    const std::string device = m_client->presence().phone.id;
    const float line = s;
    const float listWidth = std::clamp(width * Tokens::componentLinkThreadListShare, Tokens::componentLinkThreadListMin * s,
                                       Tokens::componentLinkThreadList * s);
    auto split = ui::row({.align = FlexAlign::Stretch, .gap = Tokens::spaceP16 * s, .width = width, .height = height});

    auto pane = ui::column({.align = FlexAlign::Stretch, .width = listWidth - line, .height = height});
    pane->setPadding(Tokens::spaceP16 * s, Tokens::spaceP12 * s, 0.0F, Tokens::spaceP12 * s);
    const auto dial = [this, device] {
      const std::string number = m_client->messages().numberFor(m_dial);
      if (number.empty()) return;
      m_dial.clear();
      m_client->dial(device, number);
      m_window->requestRebuild();
    };
    const float h = Tokens::sizeControlM * s;
    auto field = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingH = Tokens::spaceP12 * s,
                          .fill = colorSpecFromRole(ColorRole::Card), .radius = h / 2.0F, .border = colorSpecFromRole(ColorRole::Separator),
                          .borderWidth = line, .height = h, .flexGrow = 1.0F});
    field->addChild(ui::glyph({.glyph = std::string("phone"), .glyphSize = Tokens::sizeSymbol * s, .color = colorSpecFromRole(ColorRole::Label3)}));
    field->addChild(ui::input({.value = m_dial, .placeholder = i18n::tr("link.messages.name-or-number"), .fontSize = Tokens::typeBody.size * s,
                               .frameVisible = false, .flexGrow = 1.0F,
                               .onChange = [this](const std::string& value) { m_dial = value; },
                               .onSubmit = [dial](const std::string&) { dial(); }}));
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    bar->setPadding(0.0F, Tokens::spaceP4 * s, Tokens::spaceP4 * s, Tokens::spaceP4 * s);
    bar->addChild(std::move(field));
    bar->addChild(pill(i18n::tr("link.common.call"), std::nullopt, true, s, dial));
    pane->addChild(std::move(bar));
    if (log.empty()) {
      auto empty = emptyState("phone", i18n::tr("link.calls.none-title"), i18n::tr("link.calls.none-body"), s);
      empty->setFlexGrow(1.0F);
      pane->addChild(std::move(empty));
    } else {
      pane->addChild(ui::virtualListView({.state = &m_callScroll, .contentScale = s, .adapter = m_calls.get(), .flexGrow = 1.0F}));
    }
    auto edge = ui::row({.align = FlexAlign::Stretch, .width = listWidth, .height = height});
    edge->addChild(std::move(pane));
    edge->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .width = line, .height = height}));
    split->addChild(std::move(edge));

    auto shown = std::ranges::find_if(log, [this](const CallEntry& c) { return m_call == callKey(c); });
    if (shown == log.end()) shown = log.begin();
    if (shown == log.end()) {
      split->addChild(ui::spacer());
      return split;
    }
    m_call = callKey(*shown);
    const CallEntry call = *shown;
    const std::string name = call.name.empty() ? m_client->messages().nameFor(call.number) : call.name;
    const float pad = Tokens::spaceP24 * s;
    const float detailWidth = std::max(0.0F, width - listWidth - Tokens::spaceP16 * s - 2.0F * pad);
    auto detail = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    detail->setPadding(pad, pad, 0.0F, pad);
    auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP16 * s});
    const float big = Tokens::spaceP64 * s;
    const std::string letters = initials(name);
    auto mark = text(letters.empty() ? "#" : letters, Tokens::typeTitle1, ColorRole::Label1, s);
    head->addChild(ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                            .radius = big / 2.0F, .width = big, .height = big},
                           std::move(mark)));
    auto who = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    who->addChild(text(name, Tokens::typeTitle1, ColorRole::Label1, s));
    if (name != call.number) who->addChild(text(call.number, Tokens::typeBody, ColorRole::Label2, s));
    head->addChild(std::move(who));
    const std::string number = call.number;
    head->addChild(pill(i18n::tr("link.feed.text"), std::string("message"), false, s, [this, number] {
      show(Page::Messages);
      openCompose(number);
    }));
    head->addChild(pill(i18n::tr("link.client.call-back"), std::string("phone"), true, s,
                        [this, device, number] { m_client->dial(device, number); }));
    detail->addChild(std::move(head));

    auto with = text(i18n::tr("link.calls.with", "name", name), Tokens::typeFootnote, ColorRole::Label3, s);
    with->setFontWeight(FontWeight::SemiBold);
    auto withLine = ui::row({}, std::move(with));
    withLine->setPadding(Tokens::spaceP32 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s);
    detail->addChild(std::move(withLine));
    // The contact's newest calls; the whole log is in the list.
    constexpr std::size_t kWithRows = 5;
    const float inset = Tokens::spaceP4 * s;
    auto card = ui::column({.align = FlexAlign::Stretch, .padding = inset, .fill = colorSpecFromRole(ColorRole::Card),
                            .radius = Style::scaledRadius(Tokens::radiusR16, s)});
    const std::int64_t now = nowMs();
    std::size_t rows = 0;
    for (const CallEntry& c : log) {
      if (c.number != call.number) continue;
      if (rows > 0) card->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Separator), .height = line}));
      card->addChild(callRow(c, callKind(c), {}, dayLabel(c.atMs, now) + " " + clock(c.atMs), clearColorSpec(), renderer,
                             detailWidth - 2.0F * inset, s));
      if (++rows == kWithRows) break;
    }
    detail->addChild(std::move(card));
    auto route = text(i18n::tr("link.calls.route"), Tokens::typeFootnote, ColorRole::Label3, s, 3);
    route->setMaxWidth(detailWidth);
    auto routeLine = ui::row({}, std::move(route));
    routeLine->setPadding(Tokens::spaceP12 * s, 0.0F, 0.0F, 0.0F);
    detail->addChild(std::move(routeLine));
    split->addChild(std::move(detail));
    return split;
  }

  // Activity (pages.html B.activity): a bar "Today" | Clear finished, then two columns: Running, Waiting for you and
  // Failed on the left, Done on the right. Rows that do not fit scroll inside their card.
  std::unique_ptr<Node> LinkApp::activity(Renderer& renderer, float width, float height) {
    using Section = TransferAdapter::Section;
    const float s = m_scale;
    std::array<std::size_t, 4> rows{};
    for (std::size_t i = 0; i < rows.size(); ++i) rows[i] = m_transfers[i]->itemCount();
    if (std::ranges::all_of(rows, [](std::size_t n) { return n == 0; })) {
      return emptyState("arrows-down-up", i18n::tr("link.activity.none-title"), i18n::tr("link.activity.none-body"), s);
    }
    const float gap = Tokens::spaceP24 * s;
    const float barGap = Tokens::spaceP16 * s;
    const float inset = Tokens::spaceP4 * s;
    auto page = ui::column({.align = FlexAlign::Stretch, .gap = barGap, .width = width, .height = height});
    auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .minHeight = Tokens::sizeControlM * s});
    auto today = text(i18n::tr("link.day.today"), Tokens::typeFootnote, ColorRole::Label2, s);
    today->setFontWeight(FontWeight::SemiBold);
    today->setFlexGrow(1.0F);
    bar->addChild(std::move(today));
    if (m_client->activity().finishedCount() > 0) {
      bar->addChild(pill(i18n::tr("link.common.clear-finished"), std::nullopt, false, s, [this] { m_client->clearFinished(); }));
    }
    const float barHeight = bar->measure(renderer, LayoutConstraints::available(width, height)).height;
    page->addChild(std::move(bar));
    const float columnWidth = std::floor((width - gap) / 2.0F);
    const float rowWidth = columnWidth - 2.0F * inset;
    const float columnHeight = std::max(0.0F, height - barHeight - barGap);
    const auto head = [&](const char* key) {
      auto line = ui::row({.align = FlexAlign::Center});
      line->setPadding(Tokens::spaceP12 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s, Tokens::spaceP8 * s);
      auto title = text(i18n::tr(key), Tokens::typeFootnote, ColorRole::Label2, s);
      title->setFontWeight(FontWeight::SemiBold);
      line->addChild(std::move(title));
      return line;
    };
    // One column of sections. The heads and card insets come first; the room left goes to the cards, the shortest
    // first, each up to its rows' height and at least an equal share of what remains.
    const auto column = [&](std::initializer_list<std::pair<Section, const char*>> sections) {
      auto box = ui::column({.align = FlexAlign::Stretch, .width = columnWidth, .height = columnHeight});
      float room = columnHeight;
      struct Shown {
        Section section;
        const char* key;
        float content = 0.0F;
        float height = 0.0F;
      };
      std::vector<Shown> shown;
      for (const auto& [section, key] : sections) {
        const auto i = static_cast<std::size_t>(section);
        if (rows[i] == 0) continue;
        room -= head(key)->measure(renderer, LayoutConstraints::available(columnWidth, height)).height + 2.0F * inset;
        float content = 0.0F;
        for (std::size_t r = 0; r < rows[i] && content < columnHeight; ++r) content += m_transfers[i]->measureItem(renderer, r, rowWidth);
        shown.push_back({.section = section, .key = key, .content = content});
      }
      std::vector<Shown*> byContent;
      for (Shown& card : shown) byContent.push_back(&card);
      std::ranges::sort(byContent, {}, &Shown::content);
      for (std::size_t n = 0; n < byContent.size(); ++n) {
        byContent[n]->height = std::min(byContent[n]->content, std::max(0.0F, room) / static_cast<float>(byContent.size() - n));
        room -= byContent[n]->height;
      }
      for (const Shown& card : shown) {
        const auto i = static_cast<std::size_t>(card.section);
        box->addChild(head(card.key));
        auto plate = ui::column({.align = FlexAlign::Stretch, .padding = inset, .fill = colorSpecFromRole(ColorRole::Card),
                                 .radius = Style::scaledRadius(Tokens::radiusR16, s), .clipChildren = true});
        auto rowsBox = ui::column({.align = FlexAlign::Stretch, .height = card.height});
        rowsBox->addChild(ui::virtualListView({.state = &m_transferScroll[i], .contentScale = s, .adapter = m_transfers[i].get(),
                                               .flexGrow = 1.0F}));
        plate->addChild(std::move(rowsBox));
        box->addChild(std::move(plate));
      }
      return box;
    };
    auto columns = ui::row({.align = FlexAlign::Start, .gap = gap, .flexGrow = 1.0F});
    columns->addChild(column({{Section::Running, "link.activity.running"}, {Section::Waiting, "link.activity.waiting"},
                              {Section::Failed, "link.activity.failed"}}));
    columns->addChild(column({{Section::Done, "link.activity.done"}}));
    page->addChild(std::move(columns));
    return page;
  }


} // namespace linkapp
