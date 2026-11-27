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
#include <qrencode.h>
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

    constexpr std::int32_t kAndroidPower = 26; // KeyEvent.KEYCODE_POWER
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

    // Rulebook §9.13: symbol 48 label_3, title_3, paragraph label_2, at most one button.
    std::unique_ptr<Node> emptyState(const std::string& glyph, const std::string& title, const std::string& body, float s,
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

    // ReplyCapsule (handoff.yaml): a fill_1 capsule with the field and a round Primary send button.
    std::unique_ptr<Flex> replyCapsule(const std::string& value, const std::string& placeholder, float height, float s,
                                       std::function<void(const std::string&)> change, std::function<void()> send) {
      const float key = height - 2.0F * Tokens::spaceP4 * s;
      auto capsule = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::Fill1),
                              .radius = height / 2.0F, .height = height, .flexGrow = 1.0F});
      capsule->setPadding(0.0F, Tokens::spaceP4 * s, 0.0F, Tokens::spaceP12 * s);
      capsule->addChild(ui::input({.value = value, .placeholder = placeholder, .fontSize = Tokens::typeBody.size * s,
                                   .frameVisible = false, .flexGrow = 1.0F, .onChange = std::move(change),
                                   .onSubmit = [send](const std::string&) { send(); }}));
      capsule->addChild(ui::button({.glyph = std::string("send"), .glyphSize = Tokens::sizeSymbolInline * s,
                                    .variant = ButtonVariant::Primary, .tooltip = i18n::tr("link.common.send"), .radius = key / 2.0F,
                                    .width = key, .height = key, .onClick = std::move(send)}));
      return capsule;
    }

    std::string lockedText() { return i18n::tr("link.feed.locked"); }

  } // namespace

  std::size_t FeedAdapter::itemCount() const { return m_client.feed().items().size(); }

  std::uint64_t FeedAdapter::itemKey(std::size_t index) const {
    return std::hash<std::string>{}(m_client.feed().items()[index].key);
  }

  float FeedAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(nullptr, index, width)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> FeedAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void FeedAdapter::bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool /*hovered*/) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(&renderer, index, width));
  }

  // FeedItem (handoff.yaml): avatar 32 | who headline + time footnote; body paragraph, 2 lines; actions row g 8.
  std::unique_ptr<Node> FeedAdapter::build(Renderer* renderer, std::size_t index, float width) {
    const float s = m_scale;
    const Feed& feed = m_client.feed();
    const FeedItem& item = feed.items()[index];
    const bool locked = m_client.presence().kind == Presence::Kind::Locked;
    auto block = ui::column({.align = FlexAlign::Stretch});
    const std::size_t earlier = feed.earlierStart(nowMs());
    if (index == 0 || index == earlier) {
      block->addChild(ui::row({.paddingV = Tokens::spaceP8 * s, .paddingH = (Tokens::spaceP16 + Tokens::sizeControlM + Tokens::spaceP12) * s},
                              text(index < earlier ? i18n::tr("link.feed.now") : i18n::tr("link.feed.earlier-today"), Tokens::typeFootnote, ColorRole::Label2, s)));
    } else {
      block->addChild(ui::separator({}));
    }

    auto row = ui::row({.align = FlexAlign::Start, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP12 * s,
                        .paddingH = Tokens::spaceP16 * s});
    switch (item.kind) {
    case FeedItem::Kind::Text: row->addChild(initialsAvatar(m_client.messages().nameFor(item.who), s)); break;
    case FeedItem::Kind::Offer: row->addChild(glyphAvatar("file", s)); break;
    case FeedItem::Kind::MissedCall: row->addChild(glyphAvatar("phone-x", s)); break;
    case FeedItem::Kind::Notification: row->addChild(glyphAvatar("bell", s)); break;
    case FeedItem::Kind::Photos: row->addChild(glyphAvatar("photo", s)); break;
    }

    auto body = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP2 * s, .flexGrow = 1.0F});
    const bool offer = item.kind == FeedItem::Kind::Offer;
    const std::string who = offer ? (item.files.size() == 1 ? item.files.front().first
                                                            : i18n::trp("link.common.files-count", static_cast<long>(item.files.size())))
                                  : item.kind == FeedItem::Kind::Text ? m_client.messages().nameFor(item.who)
                                                                      : item.who;
    auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    auto name = text(who, Tokens::typeHeadline, ColorRole::Label1, s);
    name->setFlexGrow(1.0F);
    head->addChild(std::move(name));
    const bool photos = item.kind == FeedItem::Kind::Photos;
    head->addChild(text(offer ? byteSize(item.total) : photos ? i18n::tr("link.day.today") : clock(item.atMs), Tokens::typeFootnote,
                        ColorRole::Label2, s));
    body->addChild(std::move(head));
    const std::string line = offer    ? i18n::tr("link.feed.from-phone")
                             : photos ? i18n::tr("link.feed.drag-hint")
                             : locked ? lockedText()
                                      : item.body;
    body->addChild(text(line, Tokens::typeParagraph, (locked && !offer) || photos ? ColorRole::Label2 : ColorRole::Label1, s, 2));
    if (photos) {
      // Mockup .strip: 72 thumbnails at gap 8, as many as fit the body width.
      const float side = Tokens::componentLinkStripThumb * s;
      const float gap = Tokens::spaceP8 * s;
      const float bodyWidth = width - (2.0F * Tokens::spaceP16 + Tokens::sizeControlM + Tokens::spaceP12) * s;
      const auto fit = static_cast<std::size_t>(std::max(1.0F, std::floor((bodyWidth + gap) / (side + gap))));
      const float radius = Style::scaledRadius(Tokens::radiusR12, s);
      auto strip = ui::row({.gap = gap, .paddingV = Tokens::spaceP4 * s});
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
        pick->addChild(ui::box({.fill = clearColorSpec(), .border = colorSpecFromRole(ColorRole::Stroke),
                                .borderWidth = Tokens::componentSplitStroke * s, .radius = radius, .width = side, .height = side}));
        strip->addChild(std::move(pick));
      }
      body->addChild(std::move(strip));
    }

    auto actions = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingV = Tokens::spaceP4 * s});
    const FeedItem copy = item;
    const auto canReply = item.kind == FeedItem::Kind::Text || std::ranges::any_of(item.actions, &FeedAction::reply);
    if (offer && item.transfer == Transfer::Offered) {
      actions->addChild(ui::button({.text = i18n::tr("link.common.decline"), .variant = ButtonVariant::Tonal,
                                    .onClick = [this, copy] { m_client.decline(copy); }}));
      actions->addChild(ui::button({.text = i18n::tr("link.common.save-to-downloads"), .variant = ButtonVariant::Primary,
                                    .onClick = [this, copy] { m_client.accept(copy); }}));
    } else if (offer) {
      const float progress = item.total > 0 ? static_cast<float>(item.bytes) / static_cast<float>(item.total) : 0.0F;
      actions->addChild(ui::progressBar({.progress = item.transfer == Transfer::Done ? 1.0F : progress,
                                         .height = Tokens::spaceP4 * s, .flexGrow = 1.0F}));
    } else if (canReply && !locked) {
      const std::string key = item.key;
      auto capsule = replyCapsule(m_drafts[key], i18n::tr("link.feed.reply"), Tokens::sizeControlM * s, s,
                                  [this, key](const std::string& value) { m_drafts[key] = value; },
                                  [this, copy, key] {
                                    const std::string value = m_drafts[key];
                                    if (value.empty()) return;
                                    m_drafts.erase(key);
                                    m_client.reply(copy, value);
                                  });
      actions->addChild(std::move(capsule));
      // Mockup .chip: a 24 high fill_1 capsule, footnote text.
      for (const std::string& reply : item.replies) {
        actions->addChild(ui::button({.text = reply, .fontSize = Tokens::typeFootnote.size * s, .controlHeight = Tokens::sizeControlS * s,
                                      .variant = ButtonVariant::Tonal,
                                      .paddingV = 0.0F, .paddingH = Tokens::spaceP8 * s, .radius = Tokens::sizeControlS * s / 2.0F,
                                      .height = Tokens::sizeControlS * s,
                                      .onClick = [this, copy, reply] { m_client.reply(copy, reply); }}));
      }
    } else {
      for (const FeedAction& action : item.actions) {
        if (action.reply) continue;
        const bool call = item.kind == FeedItem::Kind::MissedCall;
        actions->addChild(ui::button({.text = action.label, .glyph = call ? std::optional<std::string>("phone") : std::nullopt,
                                      .glyphSize = Tokens::sizeSymbolInline * s, .variant = ButtonVariant::Tonal,
                                      .onClick = [this, copy, id = action.id] { m_client.action(copy, id); }}));
        if (call && m_hooks.textTo) {
          actions->addChild(ui::button({.text = i18n::tr("link.feed.text"), .glyph = std::string("message"),
                                        .glyphSize = Tokens::sizeSymbolInline * s, .variant = ButtonVariant::Ghost,
                                        .onClick = [this, number = action.id] { m_hooks.textTo(number); }}));
        }
      }
    }
    if (!actions->children().empty()) body->addChild(std::move(actions));
    row->addChild(std::move(body));
    block->addChild(std::move(row));
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
    return Tokens::componentLinkThreadRow * m_scale;
  }

  std::unique_ptr<Node> ThreadAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  // Thread row (handoff.yaml): avatar | name + time; last line + unread badge; selected = the selection tint.
  void ThreadAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool hovered) {
    const float s = m_scale;
    const Thread& t = m_client.messages().threads()[at(index)];
    const bool selected = m_selected == t.id;
    const bool locked = m_client.presence().kind == Presence::Kind::Locked;
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    const ColorSpec fill = selected  ? colorSpecFromRole(ColorRole::Accent, SelectionList::selectionTintAlpha())
                           : hovered ? colorSpecFromRole(ColorRole::Fill1)
                                     : clearColorSpec();
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .paddingH = Tokens::spaceP12 * s, .fill = fill, .radius = Style::scaledRadius(Tokens::radiusR12, s),
                        .height = Tokens::componentLinkThreadRow * s});
    row->addChild(initialsAvatar(t.name, s));
    auto body = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    const ColorRole quiet = selected ? ColorRole::Label1 : ColorRole::Label2;
    auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    auto name = text(t.name, Tokens::typeBody, ColorRole::Label1, s);
    if (t.unread > 0) name->setFontWeight(FontWeight::SemiBold);
    name->setFlexGrow(1.0F);
    head->addChild(std::move(name));
    head->addChild(text(when(t.atMs), Tokens::typeFootnote, quiet, s));
    body->addChild(std::move(head));
    auto line = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    auto last = text(locked && t.unread > 0 ? lockedText() : t.last, Tokens::typeFootnote, quiet, s);
    last->setFlexGrow(1.0F);
    line->addChild(std::move(last));
    if (t.unread > 0) {
      const float h = Tokens::componentBadgeHeight * s;
      auto badge = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .paddingH = Tokens::componentBadgePaddingX * s,
                            .fill = colorSpecFromRole(ColorRole::Fill3), .radius = h / 2.0F, .minWidth = h, .height = h});
      auto count = text(t.unread > 99 ? std::string("99+") : std::to_string(t.unread), Tokens::typeCaption, ColorRole::Label1, s);
      count->setFontWeight(FontWeight::SemiBold);
      badge->addChild(std::move(count));
      line->addChild(std::move(badge));
    }
    body->addChild(std::move(line));
    row->addChild(std::move(body));
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

  // Bubbles (handoff.yaml): in fill_1 left, out fill_2 right, r 16, paragraph, at most 72 % wide; the run's time
  // below its last bubble; day lines as centred footnotes. An MMS image is a 240 x 160 cover inside a p4 bubble.
  std::unique_ptr<Node> ConversationAdapter::build(std::size_t index, float width, Renderer* renderer) const {
    using K = ConversationRow::Kind;
    using N = ConversationRow::Note;
    const float s = m_scale;
    const ConversationRow& row = m_rows[index];
    const float inset = Tokens::spaceP24 * s;
    if (row.kind == K::Day) {
      return ui::row({.justify = FlexJustify::Center, .paddingV = Tokens::spaceP8 * s},
                     text(row.text, Tokens::typeFootnote, ColorRole::Label2, s));
    }
    const bool in = row.kind == K::In;
    const bool hidden = in && m_client.presence().kind == Presence::Kind::Locked;
    auto block = ui::column({.align = in ? FlexAlign::Start : FlexAlign::End, .gap = Tokens::spaceP4 * s, .paddingH = inset});
    const float padH = Tokens::spaceP12 * s;
    const ColorSpec fill = colorSpecFromRole(in ? ColorRole::Fill1 : ColorRole::Fill2);
    const float radius = Style::scaledRadius(Tokens::componentLinkBubbleRadius, s);
    if (row.image != 0 && !hidden) {
      const float pad = Tokens::spaceP4 * s;
      const float inner = Style::scaledRadius(Tokens::componentLinkBubbleRadius - Tokens::spaceP4, s);
      auto bubble = ui::column({.padding = pad, .fill = fill, .radius = radius});
      Image* image = nullptr;
      bubble->addChild(ui::image({.out = &image, .fit = ImageFit::Cover, .radius = inner,
                                  .width = Tokens::componentLinkMmsImageW * s, .height = Tokens::componentLinkMmsImageH * s}));
      const std::vector<std::uint8_t>* png = renderer != nullptr ? m_client.textImage(row.image) : nullptr;
      if (png != nullptr) (void)image->setSourceBytes(*renderer, png->data(), png->size());
      block->addChild(std::move(bubble));
    } else {
      auto bubble = ui::column({.paddingV = Tokens::spaceP8 * s, .paddingH = padH, .fill = fill, .radius = radius});
      bubble->addChild(ui::label({.text = hidden ? lockedText() : row.text, .fontSize = Tokens::typeParagraph.size * s,
                                  .fontWeight = static_cast<FontWeight>(Tokens::typeParagraph.weight),
                                  .color = colorSpecFromRole(hidden ? ColorRole::Label2 : ColorRole::Label1),
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
      block->addChild(ui::row({.paddingV = Tokens::spaceP2 * s, .paddingH = padH},
                              text(meta, Tokens::typeFootnote, failed ? ColorRole::DangerText : ColorRole::Label2, s)));
    }
    return block;
  }

  std::uint64_t CallAdapter::itemKey(std::size_t index) const {
    const CallEntry& c = m_client.calls()[at(index)];
    return std::hash<std::string>{}(std::format("{}|{}", c.number, c.atMs));
  }

  float CallAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(index)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> CallAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void CallAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool /*hovered*/) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(index));
  }

  // Call row (handoff.yaml): direction symbol | name + kind · duration | time | Call; missed in danger_text.
  std::unique_ptr<Node> CallAdapter::build(std::size_t index) const {
    using K = CallEntry::Kind;
    const float s = m_scale;
    const auto& calls = m_client.calls();
    const CallEntry& c = calls[at(index)];
    const std::int64_t now = nowMs();
    const auto today = [&](std::size_t row) { return daysAgo(calls[at(row)].atMs, now) == 0; };
    const float indent = Tokens::sizeControlM * s + Tokens::spaceP12 * s;
    auto block = ui::column({.align = FlexAlign::Stretch});
    if (index == 0 || today(index - 1) != today(index)) {
      auto head = text(today(index) ? i18n::tr("link.day.today") : i18n::tr("link.calls.earlier"), Tokens::typeFootnote, ColorRole::Label2, s);
      head->setFontWeight(FontWeight::SemiBold);
      auto line = ui::row({.paddingV = Tokens::spaceP8 * s}, std::move(head));
      line->setPadding(Tokens::spaceP16 * s, 0.0F, Tokens::spaceP8 * s, indent);
      block->addChild(std::move(line));
    } else {
      auto rule = ui::column({.align = FlexAlign::Stretch});
      rule->setPadding(0.0F, 0.0F, 0.0F, indent);
      rule->addChild(ui::separator({}));
      block->addChild(std::move(rule));
    }
    const bool missed = c.kind == K::Missed;
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .minHeight = Tokens::componentLinkThreadRow * s});
    row->addChild(missed ? glyphAvatar("phone-x", s, ColorRole::DangerText)
                         : glyphAvatar(c.kind == K::Outgoing ? "arrow-up" : "arrow-down", s));
    auto body = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    auto name = text(c.name.empty() ? m_client.messages().nameFor(c.number) : c.name, Tokens::typeBody,
                     missed ? ColorRole::DangerText : ColorRole::Label1, s);
    name->setFontWeight(FontWeight::Medium);
    body->addChild(std::move(name));
    const std::string kind = missed                    ? i18n::tr("link.calls.missed")
                             : c.kind == K::Outgoing ? i18n::tr("link.calls.outgoing")
                                                     : i18n::tr("link.calls.incoming");
    body->addChild(text(missed || c.seconds == 0 ? kind : std::format("{} · {}", kind, duration(c.seconds)),
                        Tokens::typeFootnote, ColorRole::Label2, s));
    row->addChild(std::move(body));
    row->addChild(text(today(index) ? clock(c.atMs) : when(c.atMs) + " " + clock(c.atMs), Tokens::typeFootnote,
                       ColorRole::Label2, s));
    const std::string device = m_client.presence().phone.id;
    const std::string number = c.number;
    row->addChild(ui::button({.glyph = std::string("phone"), .glyphSize = Tokens::sizeSymbol * s, .variant = ButtonVariant::Ghost,
                              .tooltip = i18n::tr("link.common.call"),
                              .onClick = [this, device, number] { m_client.dial(device, number); }}));
    block->addChild(std::move(row));
    return block;
  }

  std::uint64_t TransferAdapter::itemKey(std::size_t index) const {
    const Activity::Row row = m_client.activity().row(index);
    if (const auto* const* clip = std::get_if<const ClipEntry*>(&row)) return std::hash<std::string>{}(std::format("c:{}", (*clip)->id));
    return std::hash<std::string>{}(std::get<const TransferEntry*>(row)->id);
  }

  float TransferAdapter::measureItem(Renderer& renderer, std::size_t index, float width) {
    return build(index)->measure(renderer, LayoutConstraints::available(width, 1.0e6F)).height;
  }

  std::unique_ptr<Node> TransferAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  void TransferAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool /*hovered*/) {
    auto& holder = static_cast<Flex&>(item);
    while (!holder.children().empty()) holder.removeChild(holder.children().back().get());
    holder.addChild(build(index));
  }

  std::unique_ptr<Node> TransferAdapter::build(std::size_t index) const {
    const float s = m_scale;
    const Activity& activity = m_client.activity();
    const auto atMs = [](const Activity::Row& row) { return std::visit([](const auto* e) { return e->atMs; }, row); };
    const Activity::Row current = activity.row(index);
    const std::int64_t now = nowMs();
    const float indent = Tokens::sizeControlM * s + Tokens::spaceP12 * s;
    auto block = ui::column({.align = FlexAlign::Stretch});
    const std::string day = dayLabel(atMs(current), now);
    if (index == 0 || dayLabel(atMs(activity.row(index - 1)), now) != day) {
      auto head = text(day, Tokens::typeFootnote, ColorRole::Label2, s);
      head->setFontWeight(FontWeight::SemiBold);
      auto line = ui::row({}, std::move(head));
      line->setPadding(Tokens::spaceP16 * s, 0.0F, Tokens::spaceP8 * s, indent);
      block->addChild(std::move(line));
    } else {
      auto rule = ui::column({.align = FlexAlign::Stretch});
      rule->setPadding(0.0F, 0.0F, 0.0F, indent);
      rule->addChild(ui::separator({}));
      block->addChild(std::move(rule));
    }
    if (const auto* const* clip = std::get_if<const ClipEntry*>(&current)) {
      block->addChild(buildClip(**clip));
    } else {
      block->addChild(buildTransfer(*std::get<const TransferEntry*>(current)));
    }
    return block;
  }

  // Clip row: clipboard symbol | the text's first line, "Copied on <phone> · time" | Copy puts the text back on
  // this desktop's clipboard.
  std::unique_ptr<Node> TransferAdapter::buildClip(const ClipEntry& e) const {
    const float s = m_scale;
    const float key = Tokens::sizeControlM * s;
    auto symbol = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                           .radius = Style::scaledRadius(Tokens::radiusR8, s), .width = key, .height = key});
    symbol->addChild(ui::glyph({.glyph = std::string("clipboard"), .glyphSize = Tokens::sizeSymbol * s,
                                .color = colorSpecFromRole(ColorRole::Label2)}));
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .minHeight = Tokens::componentLinkThreadRow * s});
    row->addChild(std::move(symbol));
    std::size_t cut = std::min(e.text.find('\n'), std::size_t{200});
    while (cut < e.text.size() && (static_cast<unsigned char>(e.text[cut]) & 0xC0U) == 0x80U) --cut; // a UTF-8 boundary
    const std::string first = e.text.substr(0, cut);
    const Presence& presence = m_client.presence();
    const std::string phone = presence.phone.id == e.deviceId ? presence.phone.name : i18n::tr("link.activity.the-phone");
    auto body = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP2 * s, .flexGrow = 1.0F});
    auto title = text(first, Tokens::typeBody, ColorRole::Label1, s);
    title->setFontWeight(FontWeight::Medium);
    body->addChild(std::move(title));
    body->addChild(text(i18n::tr("link.activity.copied-on", "phone", phone, "time", clock(e.atMs)), Tokens::typeFootnote, ColorRole::Label2, s));
    row->addChild(std::move(body));
    row->addChild(ui::button({.text = i18n::tr("desk.edit.copy"), .variant = ButtonVariant::Ghost, .onClick = [clip = e.text] {
                                if (TextClipboard* clipboard = Input::textClipboard(); clipboard != nullptr) {
                                  clipboard->setClipboardText(clip);
                                }
                              }}));
    return row;
  }

  // Transfer row (handoff.yaml): direction symbol | name, status line, ProgressBar | actions: offered Decline + Save,
  // running Cancel, done Open (one received file) and "Show in Folder"; a failed one says why in danger_text.
  std::unique_ptr<Node> TransferAdapter::buildTransfer(const TransferEntry& e) const {
    const float s = m_scale;
    const std::int64_t now = nowMs();
    const float key = Tokens::sizeControlM * s;
    auto symbol = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::Center, .fill = colorSpecFromRole(ColorRole::Fill2),
                           .radius = Style::scaledRadius(Tokens::radiusR8, s), .width = key, .height = key});
    symbol->addChild(ui::glyph({.glyph = std::string(e.incoming ? "arrow-down" : "arrow-up"), .glyphSize = Tokens::sizeSymbol * s,
                                .color = colorSpecFromRole(ColorRole::Label2)}));
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .minHeight = Tokens::componentLinkThreadRow * s});
    row->addChild(std::move(symbol));

    const std::string name = e.files.size() == 1 ? e.files.front().first
                             : e.files.empty()   ? i18n::tr("link.activity.desktop-files")
                                                 : i18n::trp("link.common.files-count", static_cast<long>(e.files.size()));
    std::string status;
    switch (e.state) {
    case Transfer::Offered: status = i18n::tr("link.activity.offered", "size", byteSize(e.total)); break;
    case Transfer::Running: {
      const auto pct = e.total > 0 ? e.bytes * 100 / e.total : 0;
      status = std::format("{} %", pct);
      if (const double rate = transferRate(e, now); rate > 0.0) {
        const auto left = static_cast<std::uint64_t>(static_cast<double>(e.total - std::min(e.bytes, e.total)) / rate);
        status += std::format(" · {}/s · {}", byteSize(static_cast<std::uint64_t>(rate)),
                              left < 60 ? i18n::tr("link.activity.under-a-minute") : i18n::tr("link.activity.minutes-left", "count", (left + 59) / 60));
      }
      break;
    }
    case Transfer::Done:
      if (e.incoming && !e.paths.empty()) {
        const std::string& path = e.paths.front();
        const auto slash = path.find_last_of('/');
        const std::string dir = slash == std::string::npos ? std::string() : path.substr(0, slash);
        status = i18n::tr("link.activity.saved-to", "folder", dir.substr(dir.find_last_of('/') + 1), "time", clock(e.endMs));
      } else {
        status = i18n::tr(e.incoming ? "link.activity.saved-line" : "link.activity.sent-line", "size", byteSize(e.total), "time", clock(e.endMs));
      }
      break;
    case Transfer::Failed:
      status = e.status == "cancelled"   ? i18n::tr("link.activity.cancelled")
               : e.status == "no-space"  ? i18n::tr("link.activity.failed-no-space")
               : e.status == "too-large" ? i18n::tr("link.activity.failed-too-large")
               : e.status == "busy"      ? i18n::tr("link.activity.failed-busy")
                                         : i18n::tr("link.activity.failed-broke-off");
      break;
    }
    auto body = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP2 * s, .flexGrow = 1.0F});
    auto title = text(name, Tokens::typeBody, ColorRole::Label1, s);
    title->setFontWeight(FontWeight::Medium);
    body->addChild(std::move(title));
    const bool failed = e.state == Transfer::Failed && e.status != "cancelled";
    body->addChild(text(status, Tokens::typeFootnote, failed ? ColorRole::DangerText : ColorRole::Label2, s));
    if (e.state == Transfer::Running) {
      const float progress = e.total > 0 ? static_cast<float>(e.bytes) / static_cast<float>(e.total) : 0.0F;
      body->addChild(ui::progressBar({.progress = progress, .height = Tokens::spaceP4 * s}));
    }
    row->addChild(std::move(body));

    auto actions = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    const std::string id = e.id;
    switch (e.state) {
    case Transfer::Offered:
      actions->addChild(ui::button({.text = i18n::tr("link.common.decline"), .variant = ButtonVariant::Tonal,
                                    .onClick = [this, id] { m_client.declineTransfer(id); }}));
      actions->addChild(ui::button({.text = i18n::tr("link.common.save"), .variant = ButtonVariant::Primary,
                                    .onClick = [this, id] { m_client.acceptTransfer(id); }}));
      break;
    case Transfer::Running:
      actions->addChild(ui::button({.text = i18n::tr("common.actions.cancel"), .variant = ButtonVariant::Tonal,
                                    .onClick = [this, id] { m_client.cancelTransfer(id); }}));
      break;
    case Transfer::Done:
      if (e.incoming && e.paths.size() == 1) {
        actions->addChild(ui::button({.text = i18n::tr("link.common.open"), .variant = ButtonVariant::Ghost,
                                      .onClick = [this, path = e.paths.front()] { m_open(path); }}));
      }
      if (!e.paths.empty()) {
        actions->addChild(ui::button({.text = i18n::tr("link.common.show-in-folder"), .variant = ButtonVariant::Ghost,
                                        .onClick = [this, paths = e.paths] { m_client.showInFolder(paths); }}));
      }
      break;
    case Transfer::Failed: break;
    }
    if (!actions->children().empty()) row->addChild(std::move(actions));
    return row;
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
      m_feed = std::make_unique<FeedAdapter>(*m_client, m_scale,
                                             FeedAdapter::Hooks{.textTo =
                                                                    [this](const std::string& number) {
                                                                      openCompose(number);
                                                                      show(Page::Messages);
                                                                    },
                                                                .openPhotos = [this] { show(Page::Photos); },
                                                                .dragPhoto = [this](const std::string& path) { dragPhoto(path); }});
      m_threads = std::make_unique<ThreadAdapter>(*m_client, m_scale, m_thread, m_search, [this](std::uint64_t id) { openThread(id); });
      m_conversation = std::make_unique<ConversationAdapter>(*m_client, m_scale);
      m_calls = std::make_unique<CallAdapter>(*m_client, m_scale, m_search);
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
        if (m_feedList != nullptr) m_feedList->notifyDataChanged();
        if (m_conversationList != nullptr) m_conversationList->notifyDataChanged();
      });
      m_transfers = std::make_unique<TransferAdapter>(*m_client, m_scale, [this](const std::string& path) { openLocalFile(path); });
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
    // The header's one Ghost action: "Clear all" on Waiting, "Clear finished" on Activity.
    const auto clear = [this](bool finished) {
      return ui::button({.text = i18n::tr(finished ? "link.common.clear-finished" : "link.common.clear-all"), .variant = ButtonVariant::Ghost,
                         .onClick = [this, finished] { finished ? m_client->clearFinished() : m_client->clearAll(); }});
    };
    window.setHeaderAccessoryBuilder([this, clear]() -> std::unique_ptr<Node> {
      if (m_client == nullptr || pairShown()) return ui::node();
      if (m_page == Page::Waiting && !m_client->feed().items().empty()) {
        return ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End}, clear(false));
      }
      if (m_page == Page::Activity && m_client->activity().finishedCount() > 0) {
        return ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End}, clear(true));
      }
      return ui::node();
    });
    window.setHeaderAccessoryWidths([clear](Renderer& renderer) {
      const auto width = [&](bool finished) {
        return clear(finished)->measure(renderer, LayoutConstraints::available(1.0e6F, 1.0e6F)).width;
      };
      const float widest = std::max(width(false), width(true));
      return AppWindow::HeaderAccessoryWidths{.floor = widest, .keep = widest};
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

  void LinkApp::chooseAndSend(const std::string& deviceId) {
    if (m_window == nullptr || bus() == nullptr) return;
    if (m_chooser == nullptr) m_chooser = std::make_unique<desk::FileChooser>(bus());
    m_window->exportHandle([this, deviceId](const std::string& parent) {
      m_chooser->openFiles(parent, i18n::tr("link.common.send-files-title"), [this, deviceId](std::vector<std::filesystem::path> paths) {
        if (!paths.empty()) m_client->sendFiles(deviceId, paths);
      });
    });
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
           m_client->presence().kind != Presence::Kind::NotPaired;
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
    if (pairing && m_client->pair().kind == Pair::Kind::Idle) m_client->startPairing();
    m_window->setTitle(pairing || pairShown() ? i18n::tr("link.pair.title") : i18n::tr(kPages[static_cast<std::size_t>(m_page)].title));
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

  std::string LinkApp::pageTitle(Page page) { return i18n::tr(kPages[static_cast<std::size_t>(page)].title); }

  std::unique_ptr<Node> LinkApp::pageBody(Renderer& renderer, float width, float height) {
    m_photoGrid = nullptr;
    m_feedList = nullptr;
    m_conversationList = nullptr;
    m_mirrorImage = nullptr;
    m_mirrorStat = nullptr;
    m_mirrorArea = nullptr;
    m_renderer = &renderer;
    const float s = m_scale = m_window->uiScale();
    if (m_client == nullptr) return emptyState("plug-connected-x", i18n::tr("link.errors.no-bus-title"), i18n::tr("link.errors.no-bus-body"), s);
    const Presence presence = m_client->presence();
    if (presence.kind != Presence::Kind::NoDaemon && pairShown()) return pair(renderer, s);
    switch (presence.kind) {
    case Presence::Kind::NoDaemon:
      if (m_page == Page::Settings) return settings(presence);
      return emptyState("plug-connected-x", i18n::tr("link.phone-card.not-running"), i18n::tr("link.errors.start-service"), s);
    case Presence::Kind::NotPaired:
      return pair(renderer, s);
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
    if (m_page == Page::Calls) return calls();
    if (m_page == Page::Photos) return photos(renderer, width, height);
    if (m_page == Page::Files) return files(width, height);
    if (m_page == Page::Mirror) return mirror(width, height);
    if (m_page == Page::Activity) return activity();
    if (m_page == Page::Settings && m_settingsView == SettingsView::Commands) return commands(presence, width, height);
    if (m_page == Page::Settings) return settings(presence);
    if (m_page != Page::Waiting) {
      const PageInfo& info = kPages[static_cast<std::size_t>(m_page)];
      return emptyState(info.glyph, i18n::tr(info.title), i18n::tr("link.errors.later-step"), s);
    }
    if (m_client->feed().items().empty()) {
      return emptyState("bell", i18n::tr("link.waiting.empty-title"), i18n::tr("link.waiting.empty-body"), s);
    }
    auto view = ui::virtualListView({.out = &m_feedList, .state = &m_feedScroll, .contentScale = s, .adapter = m_feed.get(),
                                     .flexGrow = 1.0F});
    return view;
  }

  // A card as split cards look (design-split): window card radius, stroke edge, a 40 caption that moves the window.
  std::unique_ptr<Node> LinkApp::card(std::vector<std::unique_ptr<Node>> caption, std::unique_ptr<Node> body, float width,
                                      bool windowButtons) {
    const float s = m_scale;
    const float radius = Style::scaledRadius(Tokens::componentWindowCardRadius, s);
    const float height = Tokens::componentWindowCaption * s;
    auto box = ui::column({.align = FlexAlign::Stretch, .fill = colorSpecFromRole(panelMaterialRole(), Style::windowGlassTint()),
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

  // Messages (handoff.yaml): thread list card 320 | conversation card with the reply capsule.
  std::unique_ptr<Node> LinkApp::messages(float width, float height) {
    const float s = m_scale;
    const Messages& messages = m_client->messages();
    auto cards = ui::row({.align = FlexAlign::Stretch, .gap = Tokens::componentWindowCardGap * s, .width = width, .height = height});
    const float listWidth = Tokens::componentLinkThreadList * s;

    std::vector<std::unique_ptr<Node>> listCaption;
    listCaption.push_back(text(i18n::tr("link.pages.messages"), Tokens::typeHeadline, ColorRole::Label1, s));
    listCaption.back()->setFlexGrow(1.0F);
    listCaption.push_back(ui::button({.glyph = std::string("plus"), .glyphSize = Tokens::sizeSymbol * s, .variant = ButtonVariant::Ghost,
                                      .tooltip = i18n::tr("link.messages.new-tooltip"), .onClick = [this] { openCompose({}); }}));
    std::unique_ptr<Node> list;
    if (messages.load() == Messages::Load::Loading) {
      list = ui::spacer();
    } else if (messages.threads().empty()) {
      list = emptyState("message", i18n::tr("link.messages.empty-title"), i18n::tr("link.messages.empty-body"), s);
    } else {
      auto padded = ui::column({.align = FlexAlign::Stretch, .padding = Tokens::spaceP4 * s, .flexGrow = 1.0F});
      if (m_search) {
        auto scope = ui::row({.paddingV = Tokens::spaceP4 * s, .paddingH = Tokens::spaceP12 * s});
        scope->addChild(text(i18n::tr("link.messages.search-scope", "count", messages.threads().size()), Tokens::typeFootnote,
                             ColorRole::Label2, s));
        padded->addChild(std::move(scope));
      }
      padded->addChild(ui::virtualListView({.state = &m_threadScroll, .contentScale = s, .adapter = m_threads.get(),
                                            .flexGrow = 1.0F}));
      list = std::move(padded);
    }
    list->setFlexGrow(1.0F);
    cards->addChild(card(std::move(listCaption), std::move(list), listWidth, false));

    const float convWidth = std::max(0.0F, width - listWidth - Tokens::componentWindowCardGap * s);
    const Thread* thread = m_thread ? messages.thread(*m_thread) : nullptr;
    std::vector<std::unique_ptr<Node>> head;
    head.push_back(text(thread ? thread->name : m_compose ? i18n::tr("link.messages.new-title") : std::string(), Tokens::typeHeadline,
                        ColorRole::Label1, s));
    head.back()->setFlexGrow(1.0F);
    std::unique_ptr<Node> body;
    if (m_compose) {
      body = compose(s);
    } else if (thread == nullptr) {
      body = emptyState("message", i18n::tr("link.messages.none-title"), i18n::tr("link.messages.none-body"), s);
    } else {
      const std::string number = thread->address;
      const std::string device = m_client->presence().phone.id;
      head.push_back(ui::button({.glyph = std::string("phone"), .glyphSize = Tokens::sizeSymbol * s, .variant = ButtonVariant::Ghost,
                                 .tooltip = i18n::tr("link.common.call"), .onClick = [this, device, number] { m_client->dial(device, number); }}));
      m_conversation->setRows(messages.conversation(thread->id, nowMs()));
      auto column = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      VirtualListView* log = nullptr;
      column->addChild(ui::virtualListView({.out = &log, .state = &m_conversationScroll, .contentScale = s,
                                            .itemGap = Tokens::spaceP4 * s, .adapter = m_conversation.get(), .flexGrow = 1.0F}));
      m_conversationList = log;
      log->scrollView().setStickToBottom(true);
      log->setAnchorBottom(true);
      const std::uint64_t id = thread->id;
      const auto send = [this, id] {
        const std::string value = m_threadDrafts[id];
        if (value.empty()) return;
        m_threadDrafts.erase(id);
        m_client->sendText(id, value);
      };
      auto reply = ui::row({.align = FlexAlign::Center, .padding = Tokens::spaceP8 * s});
      reply->addChild(replyCapsule(m_threadDrafts[id], i18n::tr("link.messages.placeholder"), Tokens::sizeControlL * s, s,
                                   [this, id](const std::string& value) { m_threadDrafts[id] = value; }, send));
      column->addChild(std::move(reply));
      body = std::move(column);
    }
    body->setFlexGrow(1.0F);
    cards->addChild(card(std::move(head), std::move(body), convWidth, true));
    return cards;
  }

  // New message: a "To" field with contact matches under it, the text capsule at the bottom.
  std::unique_ptr<Node> LinkApp::compose(float s) {
    const Messages& messages = m_client->messages();
    auto column = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    auto to = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingV = Tokens::spaceP8 * s,
                       .paddingH = Tokens::spaceP16 * s});
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
    auto reply = ui::row({.align = FlexAlign::Center, .padding = Tokens::spaceP8 * s});
    reply->addChild(replyCapsule(m_compose->body, i18n::tr("link.messages.placeholder"), Tokens::sizeControlL * s, s,
                                 [this](const std::string& value) { m_compose->body = value; }, send));
    column->addChild(std::move(reply));
    return column;
  }

  std::uint64_t FileAdapter::itemKey(std::size_t index) const {
    return std::hash<std::string>{}(m_client.files().entries()[at(index)].name);
  }

  float FileAdapter::measureItem(Renderer& /*renderer*/, std::size_t /*index*/, float /*width*/) {
    return Tokens::sizeControlL * m_scale;
  }

  std::unique_ptr<Node> FileAdapter::createItem() { return ui::column({.align = FlexAlign::Stretch}); }

  // File row (flow.html .tr2): symbol 32 | name | size 112 | modified 112, h 40, r 8, fill_1 on hover.
  void FileAdapter::bindItem(Renderer& /*renderer*/, Node& item, std::size_t index, float /*width*/, bool hovered) {
    const float s = m_scale;
    const Files& files = m_client.files();
    const FileEntry& entry = files.entries()[at(index)];
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s,
                        .fill = hovered ? colorSpecFromRole(ColorRole::Fill1) : clearColorSpec(),
                        .radius = Style::scaledRadius(Tokens::radiusR8, s), .height = Tokens::sizeControlL * s});
    auto symbol = ui::row({.align = FlexAlign::Center, .width = Tokens::componentLinkFileSymbol * s});
    symbol->setPadding(0.0F, 0.0F, 0.0F, Tokens::spaceP8 * s);
    symbol->addChild(ui::glyph({.glyph = std::string(entry.folder ? "folder" : "file"), .glyphSize = Tokens::sizeSymbol * s,
                                .color = colorSpecFromRole(ColorRole::Label2)}));
    row->addChild(std::move(symbol));
    auto name = text(entry.name, Tokens::typeBody, ColorRole::Label1, s);
    name->setFlexGrow(1.0F);
    row->addChild(std::move(name));
    std::string size = entry.folder ? std::string() : byteSize(entry.size);
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
    for (auto [value, role] : {std::pair{size, sizeRole}, std::pair{entry.mtimeS > 0 ? when(entry.mtimeS * 1000) : std::string(),
                                                                       ColorRole::Label2}}) {
      auto cell = text(value, Tokens::typeFootnote, role, s);
      cell->setTextAlign(TextAlign::End);
      auto box = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End, .width = Tokens::componentLinkFileColumn * s});
      box->setPadding(0.0F, Tokens::spaceP12 * s, 0.0F, 0.0F);
      box->addChild(std::move(cell));
      row->addChild(std::move(box));
    }
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

  // Files (flow.html#page=files): caption Back + breadcrumb; header Name | Size | Modified; the folder's rows.
  std::unique_ptr<Node> LinkApp::files(float width, float height) {
    const float s = m_scale;
    const Files& files = m_client->files();
    std::vector<std::unique_ptr<Node>> caption;
    caption.push_back(ui::button({.glyph = std::string("chevron-left"), .enabled = files.path() != "/", .variant = ButtonVariant::Ghost,
                                  .tooltip = i18n::tr("link.common.back"), .onClick = [this] { m_client->openFolder(m_client->files().parent()); }}));
    auto crumbs = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP4 * s, .clipChildren = true, .flexGrow = 1.0F});
    const std::vector<std::string> names = files.crumbs();
    for (std::size_t i = 0; i < names.size(); ++i) {
      if (i > 0) {
        crumbs->addChild(ui::glyph({.glyph = std::string("chevron-right"), .glyphSize = Tokens::sizeSymbol * s * 0.75F,
                                    .color = colorSpecFromRole(ColorRole::Label3)}));
      }
      if (i + 1 == names.size()) {
        crumbs->addChild(text(names[i], Tokens::typeHeadline, ColorRole::Label1, s));
      } else {
        const Button::ButtonStateColors rest{.bg = clearColorSpec(), .border = clearColorSpec(), .label = colorSpecFromRole(ColorRole::Label2)};
        const Button::ButtonStateColors hot{.bg = clearColorSpec(), .border = clearColorSpec(), .label = colorSpecFromRole(ColorRole::Label1)};
        auto crumb = ui::button({.text = names[i], .fontSize = Tokens::typeHeadline.size * s, .variant = ButtonVariant::Ghost,
                                 .customPalette = Button::ButtonPalette{.normal = rest, .hover = hot, .pressed = hot, .disabled = rest},
                                 .paddingV = 0.0F, .paddingH = 0.0F,
                                 .onClick = [this, i] { m_client->openFolder(m_client->files().crumbPath(i)); }});
        crumbs->addChild(std::move(crumb));
      }
    }
    caption.push_back(std::move(crumbs));
    caption.push_back(ui::button({.text = i18n::tr("link.phone-card.send-files"), .glyph = std::string("arrow-up"),
                                  .variant = ButtonVariant::Tonal,
                                  .onClick = [this] { chooseAndSend(m_client->presence().phone.id); }}));

    std::unique_ptr<Node> body;
    if (files.load() == Files::Load::Failed) {
      body = emptyState("folder-off", i18n::tr("link.files.open-failed"), files.error(), s,
                        ui::button({.text = i18n::tr("link.common.try-again"), .variant = ButtonVariant::Secondary,
                                    .onClick = [this] { m_client->openFolder(m_client->files().path()); }}));
    } else if (files.load() == Files::Load::Ready && files.entries().empty()) {
      body = emptyState("folder", i18n::tr("link.files.empty-title"), i18n::tr("link.files.empty-body"), s);
    } else {
      auto page = ui::column({.align = FlexAlign::Stretch});
      page->setPadding(Tokens::spaceP8 * s, Tokens::spaceP24 * s, 0.0F, Tokens::spaceP24 * s);
      auto head = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .height = Tokens::sizeControlM * s});
      head->addChild(ui::row({.width = Tokens::componentLinkFileSymbol * s}));
      for (const char* label : {"link.files.name", "link.files.size", "link.files.modified"}) {
        auto cell = ui::label({.text = i18n::tr(label), .fontSize = Tokens::typeFootnote.size * s,
                               .fontWeight = static_cast<FontWeight>(500), .color = colorSpecFromRole(ColorRole::Label2)});
        if (std::string_view(label) == "link.files.name") {
          cell->setFlexGrow(1.0F);
          head->addChild(std::move(cell));
          continue;
        }
        cell->setTextAlign(TextAlign::End);
        auto box = ui::row({.align = FlexAlign::Center, .justify = FlexJustify::End, .width = Tokens::componentLinkFileColumn * s});
        box->setPadding(0.0F, Tokens::spaceP12 * s, 0.0F, 0.0F);
        box->addChild(std::move(cell));
        head->addChild(std::move(box));
      }
      page->addChild(std::move(head));
      page->addChild(ui::separator({.color = colorSpecFromRole(ColorRole::Separator), .thickness = s}));
      if (!files.entries().empty()) {
        page->addChild(ui::virtualListView({.state = &m_fileScroll, .contentScale = s, .adapter = m_files.get(), .flexGrow = 1.0F}));
      }
      body = std::move(page);
    }
    body->setFlexGrow(1.0F);
    if (const FileOp& op = m_client->fileOp(); op.kind != FileOp::Kind::Idle) {
      auto column = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      column->addChild(std::move(body));
      column->addChild(ui::separator({.color = colorSpecFromRole(ColorRole::Separator), .thickness = s}));
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                          .paddingH = Tokens::spaceP24 * s, .minHeight = Tokens::sizeControlL * s});
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
    auto cards = ui::row({.align = FlexAlign::Stretch, .width = width, .height = height});
    cards->addChild(card(std::move(caption), std::move(body), width, true));
    return cards;
  }

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
    // Rulebook §9.7 thumbnail grid: a 2 px label_1 ring at a 2 px gap and a 20 px accent check badge at inset 8.
    tile->addChild(ui::box({.fill = clearColorSpec(), .border = colorSpecFromRole(ColorRole::Label1), .borderWidth = 2.0F}));
    tile->addChild(ui::box({.fill = colorSpecFromRole(ColorRole::Accent)}));
    tile->addChild(ui::glyph({.glyph = std::string("check"), .color = colorSpecFromRole(ColorRole::OnAccent)}));
    // A video's length at inset 8, bottom left (flow.html .pgrid i.vid): caption 600 in white over the picture.
    tile->addChild(ui::label({.fontSize = Tokens::typeCaption.size * m_scale, .fontWeight = static_cast<FontWeight>(600),
                              .color = colorSpecFromRole(ColorRole::OnAccent), .maxLines = 1}));
    return tile;
  }

  namespace {
    // The corner check of a tile (rulebook §9.7: 20 px at inset 8), with an inset-8 margin around it as its hit area.
    bool inCheck(float x, float y, float width, float height, float s) {
      const float reach = (Tokens::componentLinkTileCheck + 2.0F * Tokens::spaceP8) * s;
      return x >= width - reach && y >= height - reach;
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
    auto* length = static_cast<Label*>(kids[4].get());
    const float w = tile.width();
    const float h = tile.height();
    const bool picked = m_client.photos().selected(at(index));
    const float radius = Style::scaledRadius(Tokens::radiusR12, s);
    const float inset = picked ? 4.0F * s : 0.0F; // the ring and its gap stay inside the cell
    image->setPosition(inset, inset);
    image->setSize(w - 2.0F * inset, h - 2.0F * inset);
    image->setRadius(std::max(0.0F, radius - inset));
    ring->setBorder(colorSpecFromRole(picked ? ColorRole::Label1 : ColorRole::Stroke), (picked ? 2.0F : 0.5F) * s);
    ring->setPosition(0.0F, 0.0F);
    ring->setSize(w, h);
    ring->setRadius(radius);
    const float d = Tokens::componentLinkTileCheck * s;
    // A hovered tile shows its empty check: a press there selects without opening the viewer.
    badge->setVisible(picked || hovered);
    badge->setFill(picked ? colorSpecFromRole(ColorRole::Accent) : colorSpecFromRole(ColorRole::Shadow, Tokens::componentLinkTileCheckShade));
    badge->setBorder(picked ? clearColorSpec() : colorSpecFromRole(ColorRole::OnAccent), Tokens::componentLinkTileCheckEdge * s);
    badge->setPosition(w - Tokens::spaceP8 * s - d, h - Tokens::spaceP8 * s - d);
    badge->setSize(d, d);
    badge->setRadius(d / 2.0F);
    check->setVisible(picked);
    check->setGlyphSize(Tokens::sizeSymbol * s * 0.75F);
    if (m_renderer != nullptr) check->layout(*m_renderer); // its size is known only once measured
    check->setPosition(badge->x() + (d - check->width()) / 2.0F, badge->y() + (d - check->height()) / 2.0F);
    const Photo& photo = m_client.photos().items()[at(index)];
    length->setVisible(photo.videoS.has_value());
    if (photo.videoS) {
      length->setText(std::format("{}:{:02}", *photo.videoS / 60, *photo.videoS % 60));
      length->setShadow(colorSpecFromRole(ColorRole::Shadow), 0.0F, s);
      length->setPosition(Tokens::spaceP8 * s + inset, h - Tokens::spaceP8 * s - inset - Tokens::typeCaption.line * s);
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
    header->setPadding(Tokens::spaceP16 * m_scale, 0.0F, Tokens::spaceP8 * m_scale, 0.0F);
    header->addChild(ui::label({.fontSize = Tokens::typeFootnote.size * m_scale, .fontWeight = static_cast<FontWeight>(600),
                                .color = colorSpecFromRole(ColorRole::Label2), .maxLines = 1}));
    return header;
  }

  void PhotoAdapter::bindSectionHeader(Node& header, std::size_t section, float /*width*/, float /*height*/) {
    if (section < m_days.size()) static_cast<Label*>(header.children()[0].get())->setText(m_days[section].second);
  }

  // Photos (flow.html#page=photos): caption title, total and the All / Photos / Videos segment; the grid under day
  // headers; the FloatingCapsuleBar over the grid, 24 px above the card's foot.
  std::unique_ptr<Node> LinkApp::photos(Renderer& renderer, float width, float height) {
    const float s = m_scale;
    const Photos& photos = m_client->photos();
    std::vector<std::unique_ptr<Node>> caption;
    caption.push_back(text(i18n::tr("link.pages.photos"), Tokens::typeHeadline, ColorRole::Label1, s));
    if (photos.total() > 0) caption.push_back(text(groupedCount(photos.total()), Tokens::typeFootnote, ColorRole::Label2, s));
    caption.push_back(ui::row({.flexGrow = 1.0F}));
    if (m_viewing) {
      // The lightbox caption: Back, the photo's place in the grid order.
      caption.clear();
      caption.push_back(ui::button({.glyph = std::string("chevron-left"), .glyphSize = Tokens::sizeSymbol * s,
                                    .variant = ButtonVariant::Ghost, .tooltip = i18n::tr("link.photos.back"),
                                    .onClick = [this] { viewPhoto(std::nullopt); }}));
      const std::vector<std::size_t>* order = m_search ? &m_search->photos : nullptr;
      const auto& items = photos.items();
      const auto at = std::ranges::find(items, *m_viewing, &Photo::path);
      const auto index = static_cast<std::size_t>(at - items.begin());
      if (order != nullptr && std::ranges::find(*order, index) == order->end()) order = nullptr; // not a hit: place among all
      const std::size_t shown = order ? static_cast<std::size_t>(std::ranges::find(*order, index) - order->begin()) : index;
      auto place = text(i18n::tr("link.photos.place", "index", groupedCount(shown + 1), "count", groupedCount(order ? order->size() : photos.total())),
                        Tokens::typeFootnote, ColorRole::Label2, s);
      caption.push_back(std::move(place));
      caption.push_back(ui::row({.flexGrow = 1.0F}));
    } else {
      const bool taking = m_client->takingPhoto();
      const bool live = m_client->presence().kind == Presence::Kind::Connected;
      caption.push_back(ui::button({.text = i18n::tr(taking ? "link.photos.waiting" : "link.photos.take"),
                                    .glyph = std::string("camera"), .enabled = live && !taking,
                                    .variant = ButtonVariant::Tonal, .onClick = [this] { m_client->takePhoto(); }}));
      if (taking) {
        caption.push_back(ui::button({.text = i18n::tr("common.actions.cancel"), .variant = ButtonVariant::Ghost,
                                      .onClick = [this] { m_client->cancelTakePhoto(); }}));
      }
    }
    if (!m_viewing) caption.push_back(ui::segmented({.options = std::vector<ui::SegmentedOption>{{.label = i18n::tr("link.photos.all")}, {.label = i18n::tr("link.pages.photos")}, {.label = i18n::tr("link.photos.videos")}},
                                     .selectedIndex = static_cast<std::size_t>(photos.media()), .scale = s, .compact = true,
                                     .onChange = [this](std::size_t i) {
                                       m_client->setPhotoMedia(static_cast<Photos::Media>(i));
                                     }}));
    const auto cards = [&](std::unique_ptr<Node> body) {
      body->setFlexGrow(1.0F);
      auto row = ui::row({.align = FlexAlign::Stretch, .width = width, .height = height});
      row->addChild(card(std::move(caption), std::move(body), width, true));
      return row;
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
        return cards(emptyState("photo", i18n::tr("link.photos.loading-title"), i18n::tr("link.photos.loading-body"), s));
      }
      break;
    case Photos::Load::Denied:
      return cards(emptyState("photo-off", i18n::tr("link.photos.denied-title"), i18n::tr("link.photos.denied-body"), s,
                              retry()));
    case Photos::Load::Unsupported:
      return cards(emptyState("photo-off", i18n::tr("link.photos.unsupported-title"),
                              i18n::tr("link.photos.unsupported-body"), s));
    case Photos::Load::Failed:
      return cards(emptyState("alert-circle", i18n::tr("link.photos.failed-title"), i18n::tr("link.photos.failed-body"), s, retry()));
    case Photos::Load::More:
    case Photos::Load::Ready:
      if (photos.items().empty()) {
        return cards(videos ? emptyState("video", i18n::tr("link.photos.no-videos-title"), i18n::tr("link.photos.no-videos-body"), s)
                            : emptyState("photo", i18n::tr("link.photos.none-title"), i18n::tr("link.photos.none-body"), s));
      }
      break;
    }
    if (m_viewing) return cards(viewer(renderer, width, height));
    m_photos->setRenderer(&renderer);
    auto days = m_search ? std::vector<std::pair<std::size_t, std::string>>{{0, std::format("{} of {} loaded", m_search->photos.size(), photos.items().size())}}
                         : photos.days(nowMs());
    std::vector<std::size_t> starts;
    starts.reserve(days.size());
    for (const auto& day : days) starts.push_back(day.first);
    m_photos->setDays(std::move(days));
    auto page = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    page->setPadding(0.0F, Tokens::spaceP24 * s, Tokens::spaceP24 * s, Tokens::spaceP24 * s);
    const float headerH = (Tokens::spaceP16 + Tokens::typeFootnote.line + Tokens::spaceP8) * s;
    page->addChild(ui::virtualGridView({.out = &m_photoGrid, .state = &m_photoScroll, .contentScale = s,
                                        .minCellWidth = Tokens::componentLinkPhotoMin * s, .squareCells = true,
                                        .columnGap = Tokens::spaceP8 * s, .rowGap = Tokens::spaceP8 * s, .overscanRows = 2,
                                        .itemCursorShape = CursorShape::Pointer, .adapter = m_photos.get(), .flexGrow = 1.0F}));
    m_photoGrid->setSections(std::move(starts), headerH);
    if (m_photoScrollTo) m_photoGrid->scrollToIndex(*std::exchange(m_photoScrollTo, std::nullopt));

    // FloatingCapsuleBar (handoff.yaml): "n selected · Save · Clear", or how the last save went.
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
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .fill = colorSpecFromRole(ColorRole::Popover),
                          .radius = Tokens::sizeControlL * s / 2.0F, .height = Tokens::sizeControlL * s});
      bar->setPadding(0.0F, Tokens::spaceP8 * s, 0.0F, Tokens::spaceP16 * s);
      bar->addChild(text(what, Tokens::typeBody, ColorRole::Label1, s));
      if (photos.selectedCount() > 0) {
        const bool saving = photos.save() == Photos::Save::Saving;
        bar->addChild(ui::button({.text = i18n::tr("link.common.save"), .glyph = std::string("download"), .enabled = !saving,
                                  .variant = ButtonVariant::Ghost, .onClick = [this] { m_client->savePhotos(); }}));
        bar->addChild(ui::button({.text = i18n::tr("link.photos.clear"), .glyph = std::string("x"), .variant = ButtonVariant::Ghost,
                                  .onClick = [this] { m_client->clearPhotoSelection(); }}));
      }
      // Over the grid: out of the page's layout, laid out alone to learn its width.
      bar->setParticipatesInLayout(false);
      bar->layout(renderer);
      const float pageH = height - Tokens::componentWindowCaption * s;
      bar->setPosition((width - bar->width()) / 2.0F, pageH - Tokens::spaceP24 * s - bar->height());
      page->addChild(std::move(bar));
    }
    return cards(std::move(page));
  }

  std::unique_ptr<Node> LinkApp::calls() {
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
    auto page = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    page->setPadding(0.0F, Tokens::spaceP24 * s, Tokens::spaceP24 * s, Tokens::spaceP24 * s);
    const std::string device = m_client->presence().phone.id;
    const auto dial = [this, device] {
      const std::string number = m_client->messages().numberFor(m_dial);
      if (number.empty()) return;
      m_dial.clear();
      m_client->dial(device, number);
      m_window->requestRebuild();
    };
    const float h = Tokens::sizeControlM * s;
    auto field = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingH = Tokens::spaceP8 * s,
                          .fill = colorSpecFromRole(ColorRole::Fill1), .radius = h / 2.0F, .height = h, .flexGrow = 1.0F});
    field->addChild(ui::glyph({.glyph = std::string("phone"), .glyphSize = Tokens::sizeSymbol * s,
                               .color = colorSpecFromRole(ColorRole::Label2)}));
    field->addChild(ui::input({.value = m_dial, .placeholder = i18n::tr("link.messages.name-or-number"), .fontSize = Tokens::typeBody.size * s,
                               .frameVisible = false, .flexGrow = 1.0F,
                               .onChange = [this](const std::string& value) { m_dial = value; },
                               .onSubmit = [dial](const std::string&) { dial(); }}));
    auto row = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingV = Tokens::spaceP8 * s});
    row->addChild(std::move(field));
    row->addChild(ui::button({.text = i18n::tr("link.common.call"), .variant = ButtonVariant::Primary, .onClick = dial}));
    page->addChild(std::move(row));
    if (m_client->calls().empty()) {
      page->addChild(emptyState("phone", i18n::tr("link.calls.none-title"), i18n::tr("link.calls.none-body"), s));
    } else {
      page->addChild(ui::virtualListView({.state = &m_callScroll, .contentScale = s, .adapter = m_calls.get(), .flexGrow = 1.0F}));
    }
    return page;
  }

  // Activity (handoff.yaml): transfers since the app started; Link1 keeps no list.
  std::unique_ptr<Node> LinkApp::activity() {
    const float s = m_scale;
    if (m_client->activity().rowCount() == 0) {
      return emptyState("arrows-down-up", i18n::tr("link.activity.none-title"), i18n::tr("link.activity.none-body"), s);
    }
    auto page = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
    page->setPadding(0.0F, Tokens::spaceP24 * s, Tokens::spaceP24 * s, Tokens::spaceP24 * s);
    page->addChild(ui::virtualListView({.state = &m_transferScroll, .contentScale = s, .adapter = m_transfers.get(),
                                        .flexGrow = 1.0F}));
    return page;
  }

  // Settings (handoff.yaml): max width 640; grouped cards (fill_1 r 12 p 4) of rows with a Toggle; Ghost destructive
  // "Forget <phone>…" that asks in place.
  void LinkApp::commandsChanged(bool changed) {
    if (auto send = m_commands.takeSend()) {
      m_client->setCommands(send->second, [this, serial = send->first](std::optional<std::string> error) {
        if (m_commands.sent(serial, std::move(error))) m_window->requestRebuild();
      });
      changed = true;
    }
    if (changed) m_window->requestRebuild();
  }

  void LinkApp::commandGrip(std::size_t row, const InputArea::PointerData& data, bool motion) {
    CommandDrag& d = m_commandDrag;
    if (row >= d.rows.size()) return;
    if (!motion && data.pressed) {
      if (data.button != BTN_LEFT || row >= m_commands.list().size()) return;
      d.kind = CommandDrag::Kind::Pressed;
      d.from = d.target = row;
      d.moving = m_commands.list()[row];
      d.startY = data.sceneY;
      d.tops.assign(d.rows.size(), 0.0F);
      d.shift.assign(d.rows.size(), 0.0F);
      d.springs.assign(d.rows.size(), 0);
      for (std::size_t j = 0; j < d.rows.size(); ++j) {
        float x = 0.0F;
        Node::absolutePosition(d.rows[j], x, d.tops[j]);
      }
      return;
    }
    if (!motion) {
      if (d.kind == CommandDrag::Kind::Idle) return;
      const bool dragged = d.kind == CommandDrag::Kind::Dragging;
      for (std::size_t j = 0; j < d.rows.size(); ++j) {
        if (j < d.springs.size() && d.springs[j] != 0) m_window->animations().cancel(d.springs[j]);
        d.rows[j]->setTranslation(0.0F, 0.0F);
        d.rows[j]->setZIndex(0);
        if (j == d.from) d.rows[j]->setFill(m_commandDrag.restFill);
      }
      d.kind = CommandDrag::Kind::Idle;
      if (dragged) commandsChanged(m_commands.moveIf(d.from, d.target, d.moving));
      return;
    }
    if (d.kind == CommandDrag::Kind::Idle) return;
    const float dy = data.sceneY - d.startY;
    if (d.kind == CommandDrag::Kind::Pressed && std::abs(dy) < Tokens::spaceP4 * m_scale) return;
    Flex* moving = d.rows[d.from];
    if (d.kind == CommandDrag::Kind::Pressed) {
      d.restFill = moving->fill();
      moving->setFill(colorSpecFromRole(ColorRole::View));
      moving->setRadius(Style::scaledRadius(Tokens::componentListRowRadius, m_scale));
    }
    d.kind = CommandDrag::Kind::Dragging;
    const std::size_t n = d.rows.size();
    const float y = std::clamp(dy, d.tops.front() - d.tops[d.from], d.tops.back() - d.tops[d.from]);
    moving->setZIndex(1);
    moving->setTranslation(0.0F, y);
    const auto middle = [&](std::size_t j) { return d.tops[j] + d.rows[j]->height() / 2.0F; };
    const float center = d.tops[d.from] + y + moving->height() / 2.0F;
    std::size_t target = d.from;
    while (target + 1 < n && center > middle(target + 1)) ++target;
    while (target > 0 && center < middle(target - 1)) --target;
    if (target == d.target) return;
    d.target = target;
    float gap = 0.0F; // the rule between two neighbouring rows
    for (std::size_t j = 0; j + 1 < n; ++j) {
      const float between = d.tops[j + 1] - d.tops[j] - d.rows[j]->height();
      if (between > 0.0F && (gap == 0.0F || between < gap)) gap = between;
    }
    const float pitch = moving->height() + gap;
    for (std::size_t j = 0; j < n; ++j) {
      if (j == d.from) continue;
      const float want = d.from < j && j <= target ? -pitch : target <= j && j < d.from ? pitch : 0.0F;
      if (want == d.shift[j]) continue;
      d.shift[j] = want;
      Node* node = d.rows[j];
      d.springs[j] = m_window->animations().drive(d.springs[j], node->translationY(), want, Style::spring(Tokens::springMove),
                                                  [node](float v) { node->setTranslation(0.0F, v); }, {}, node, 0.0F, Unit::Px, true);
    }
  }

  // Settings › Commands (design-link-commands concept A): the grant row, then one row per command in the phone's order;
  // a row opens in place into Name and Command. Only a commit sends SetCommands (CommandsEditor).
  std::unique_ptr<Node> LinkApp::commands(const Presence& presence, float width, float height) {
    const float s = m_scale;
    const CommandsEditor& ed = m_commands;
    const std::vector<Command>& list = ed.list();
    for (std::uint32_t id : m_commandDrag.springs)
      if (id != 0) m_window->animations().cancel(id);
    m_commandDrag = {};
    m_commandDrag.rows.assign(list.size(), nullptr);
    const std::string id = presence.phone.id;
    const std::string& phone = presence.phone.name;
    const bool away = presence.kind == Presence::Kind::Away;
    const float inset = Tokens::componentListRowInset * s;
    const auto group = [&] {
      return ui::column({.align = FlexAlign::Stretch, .padding = Tokens::componentListCardPadding * s,
                         .fill = colorSpecFromRole(ColorRole::Fill1),
                         .radius = Style::scaledRadius(Tokens::componentListCardRadius, s)});
    };
    bool firstRow = true;
    const auto rule = [&](Flex& card) {
      if (std::exchange(firstRow, false)) return;
      auto line = ui::column({.align = FlexAlign::Stretch});
      line->setPadding(0.0F, inset, 0.0F, inset);
      line->addChild(ui::separator({}));
      card.addChild(std::move(line));
    };
    const auto mono = [&](std::string value, ColorRole role) {
      auto label = text(std::move(value), Tokens::typeMono, role, s);
      label->setFontFamily(std::string(Tokens::fontMono));
      return label;
    };
    const auto note = [&](std::string value, ColorRole role) { return text(std::move(value), Tokens::typeFootnote, role, s, 3); };

    std::vector<std::unique_ptr<Node>> caption;
    caption.push_back(ui::button({.glyph = std::string("chevron-left"), .variant = ButtonVariant::Ghost,
                                  .tooltip = i18n::tr("link.commands.back"), .onClick = [this] {
                                    commandsChanged(m_commands.close());
                                    m_settingsView = SettingsView::Main;
                                    sync();
                                    m_window->requestRebuild();
                                  }}));
    caption.push_back(text(i18n::tr("link.commands.title"), Tokens::typeHeadline, ColorRole::Label1, s));
    caption.back()->setFlexGrow(1.0F);
    if (!list.empty()) {
      caption.push_back(ui::button({.text = i18n::tr("link.commands.add"), .glyph = std::string("plus"),
                                    .enabled = ed.canAdd(), .variant = ButtonVariant::Tonal,
                                    .onClick = [this] { commandsChanged(m_commands.add()); }}));
    }

    auto page = ui::column({.align = FlexAlign::Stretch, .fillWidth = true});
    page->setPadding(Tokens::spaceP16 * s, 0.0F, Tokens::spaceP24 * s, 0.0F);

    auto grant = group();
    {
      auto r = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .paddingH = inset, .minHeight = Tokens::sizeControlL * s});
      r->addChild(ui::glyph({.glyph = std::string("device-mobile"), .glyphSize = Tokens::sizeSymbol * s,
                             .color = colorSpecFromRole(ColorRole::Label1)}));
      auto words = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      words->addChild(text(i18n::tr("link.commands.may-run", "phone", phone), Tokens::typeBody, ColorRole::Label1, s, 2));
      const bool granted = m_client->phoneSettings().commands;
      words->addChild(note(i18n::tr(!granted ? "link.commands.grant-off" : away ? "link.commands.grant-away" : "link.commands.grant-on"),
                           ColorRole::Label2));
      r->addChild(std::move(words));
      r->addChild(ui::toggle({.checked = granted, .scale = s,
                              .onChange = [this, id](bool on) { m_client->setGrant(id, "commands", on); }}));
      grant->addChild(std::move(r));
    }
    page->addChild(std::move(grant));

    auto head = ui::row({.align = FlexAlign::Center});
    head->setPadding(Tokens::spaceP16 * s, inset, Tokens::spaceP8 * s, inset);
    auto headTitle = text(i18n::tr("link.commands.buttons-on", "phone", phone), Tokens::typeFootnote, ColorRole::Label2, s);
    headTitle->setFontWeight(FontWeight::SemiBold);
    headTitle->setFlexGrow(1.0F);
    head->addChild(std::move(headTitle));
    if (list.size() >= CommandsEditor::kMax - 8) {
      auto count = text(i18n::tr("link.commands.count", "count", list.size(), "max", CommandsEditor::kMax), Tokens::typeFootnote, ColorRole::Label2, s);
      count->setFontWeight(FontWeight::SemiBold);
      head->addChild(std::move(count));
    }
    page->addChild(std::move(head));

    const bool editing = ed.mode() == CommandsEditor::Mode::Editing;
    const auto editor = [&] {
      auto box = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP8 * s, .padding = Tokens::spaceP12 * s,
                             .fill = colorSpecFromRole(ColorRole::Fill2),
                             .radius = Style::scaledRadius(Tokens::componentListRowRadius, s)});
      const auto field = [&](const char* label, std::unique_ptr<Node> input, const char* error) {
        auto line = ui::row({.align = FlexAlign::Start, .gap = Tokens::spaceP12 * s});
        auto name = text(i18n::tr(label), Tokens::typeFootnote, ColorRole::Label2, s);
        name->setFontWeight(FontWeight::SemiBold);
        auto cell = ui::column({.align = FlexAlign::Start, .width = Tokens::componentLinkFieldLabel * s});
        cell->setPadding(Tokens::spaceP8 * s, 0.0F, 0.0F, 0.0F);
        cell->addChild(std::move(name));
        line->addChild(std::move(cell));
        auto right = ui::column({.align = FlexAlign::Stretch, .gap = Tokens::spaceP4 * s, .flexGrow = 1.0F});
        right->addChild(std::move(input));
        if (error != nullptr) right->addChild(note(i18n::tr(error), ColorRole::DangerText));
        line->addChild(std::move(right));
        return line;
      };
      const auto invalid = ed.invalid();
      box->addChild(field("link.commands.name",
                          ui::input({.value = ed.draft().name,
                                     .fontSize = Tokens::typeBody.size * s, .invalid = invalid == CommandsEditor::Invalid::Name,
                                     .surfaceRole = ColorRole::View, .flexGrow = 1.0F,
                                     .onChange = [this](const std::string& v) {
                                       const auto before = m_commands.invalid();
                                       m_commands.editName(v);
                                       if (m_commands.invalid() != before) m_window->requestRebuild();
                                     },
                                     .onSubmit = [this](const std::string&) { commandsChanged(m_commands.close()); }}),
                          invalid == CommandsEditor::Invalid::Name
                              ? (ed.draft().name.size() > CommandsEditor::kMaxName ? "link.commands.name-long" : "link.commands.name-hint")
                              : nullptr));
      box->addChild(field("link.commands.command",
                          ui::input({.value = ed.draft().line,
                                     .fontSize = Tokens::typeMono.size * s, .invalid = invalid == CommandsEditor::Invalid::Line,
                                     .surfaceRole = ColorRole::View,
                                     .height = Tokens::componentLinkCommandField * s, .flexGrow = 1.0F,
                                     .onChange = [this](const std::string& v) {
                                       const auto before = m_commands.invalid();
                                       m_commands.editLine(v);
                                       if (m_commands.invalid() != before) m_window->requestRebuild();
                                     },
                                     .onSubmit = [this](const std::string&) { commandsChanged(m_commands.close()); },
                                     .configure = [](Input& input) {
                                       input.setMultiline(true);
                                       input.setSubmitOnEnter(true);
                                       input.setFontFamily(std::string(Tokens::fontMono));
                                     }}),
                          invalid == CommandsEditor::Invalid::Line ? "link.commands.line-missing" : nullptr));
      auto bar = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
      Button* remove = nullptr;
      bar->addChild(ui::button({.out = &remove, .text = i18n::tr("link.commands.remove"), .variant = ButtonVariant::Ghost,
                                .onClick = [this] { commandsChanged(m_commands.remove()); }}));
      auto palette = Button::defaultPalette(ButtonVariant::Ghost);
      for (auto* state : {&palette.normal, &palette.hover, &palette.pressed}) state->label = colorSpecFromRole(ColorRole::DangerText);
      remove->setCustomPalette(palette);
      const bool full = invalid == CommandsEditor::Invalid::Full;
      const bool failed = ed.sync() == CommandsEditor::Sync::Failed;
      auto status = note(full     ? i18n::tr("link.commands.full")
                         : failed ? i18n::tr("link.commands.not-saved-text", "error", ed.error())
                         : away   ? i18n::tr("link.commands.waiting", "phone", phone)
                                  : i18n::tr("link.commands.enter-saves"),
                         full || failed ? ColorRole::DangerText : ColorRole::Label2);
      status->setFlexGrow(1.0F);
      bar->addChild(std::move(status));
      if (failed) {
        bar->addChild(ui::button({.text = i18n::tr("link.commands.try-again"), .variant = ButtonVariant::Tonal,
                                  .onClick = [this] { commandsChanged(m_commands.retry()); }}));
      }
      bar->addChild(ui::button({.text = i18n::tr("link.commands.done"), .variant = ButtonVariant::Tonal,
                                .onClick = [this] { commandsChanged(m_commands.close()); }}));
      box->addChild(std::move(bar));
      return box;
    };

    auto rows = group();
    for (std::size_t i = 0; i <= list.size(); ++i) {
      if (ed.mode() == CommandsEditor::Mode::Removed && ed.index() == i) {
        rule(*rows);
        auto r = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s, .paddingV = Tokens::spaceP8 * s,
                          .paddingH = inset, .minHeight = Tokens::sizeControlL * s});
        auto gone = text(i18n::tr("link.commands.removed", "name", ed.draft().name), Tokens::typeBody, ColorRole::Label2, s);
        gone->setFlexGrow(1.0F);
        r->addChild(std::move(gone));
        r->addChild(ui::button({.text = i18n::tr("link.commands.undo"), .enabled = ed.canAdd(), .variant = ButtonVariant::Tonal,
                                .onClick = [this] { commandsChanged(m_commands.undo()); }}));
        rows->addChild(std::move(r));
      }
      if (editing && ed.index() == i) {
        auto open = editor();
        if (i < list.size()) m_commandDrag.rows[i] = open.get();
        rows->addChild(std::move(open));
        continue;
      }
      if (i == list.size()) break;
      rule(*rows);
      auto line = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP4 * s, .paddingV = Tokens::spaceP8 * s,
                           .paddingH = Tokens::spaceP4 * s, .minHeight = Tokens::sizeControlL * s,
                           .onClick = [this, i] { commandsChanged(m_commands.open(i)); }});
      auto grip = ui::inputArea({.cursorShape = CursorShape::Grab,
                                 .width = Tokens::sizeControlS * s, .height = Tokens::sizeControlS * s, .zIndex = 2,
                                 .onMotion = [this, i](const InputArea::PointerData& p) { commandGrip(i, p, true); },
                                 .onPress = [this, i](const InputArea::PointerData& p) { commandGrip(i, p, false); }});
      grip->addChild(ui::glyph({.glyph = std::string("grip-vertical"), .glyphSize = Tokens::sizeSymbolInline * s,
                                .color = colorSpecFromRole(ColorRole::Label3)}));
      line->addChild(std::move(grip));
      auto words = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      auto name = text(list[i].name, Tokens::typeBody, ColorRole::Label1, s);
      name->setFontWeight(FontWeight::Medium);
      words->addChild(std::move(name));
      words->addChild(mono(list[i].line, ColorRole::Label2));
      line->addChild(std::move(words));
      m_commandDrag.rows[i] = line.get();
      rows->addChild(std::move(line));
    }
    if (list.empty() && !editing) {
      rows->addChild(ui::button({.text = i18n::tr("link.commands.add"), .glyph = std::string("plus"), .variant = ButtonVariant::Ghost,
                                 .onClick = [this] { commandsChanged(m_commands.add()); }}));
    }
    page->addChild(std::move(rows));

    std::string foot = list.empty() ? i18n::tr("link.commands.empty-foot", "phone", phone)
                       : !ed.canAdd() ? i18n::tr("link.commands.full")
                                      : i18n::tr("link.commands.order-foot");
    auto footer = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    footer->setPadding(Tokens::spaceP8 * s, inset, 0.0F, inset);
    const bool failedIdle = ed.sync() == CommandsEditor::Sync::Failed && !editing;
    auto footText = note(failedIdle ? i18n::tr("link.commands.not-saved-changes", "error", ed.error()) : foot,
                         failedIdle ? ColorRole::DangerText : ColorRole::Label2);
    footText->setFlexGrow(1.0F);
    footer->addChild(std::move(footText));
    if (failedIdle) {
      footer->addChild(ui::button({.text = i18n::tr("link.commands.try-again"), .variant = ButtonVariant::Tonal,
                                   .onClick = [this] { commandsChanged(m_commands.retry()); }}));
    }
    page->addChild(std::move(footer));

    auto scroll = ui::scrollView({.state = &m_commandsScroll, .contentScale = s, .viewportPaddingH = Tokens::spaceP24 * s,
                                  .maxContentWidth = Tokens::componentLinkSettingsWidth * s,
                                  .contentAlign = ScrollView::ContentAlign::Start, .flexGrow = 1.0F});
    scroll->content()->addChild(std::move(page));
    std::unique_ptr<Node> body = std::move(scroll);
    body->setFlexGrow(1.0F);
    auto cards = ui::row({.align = FlexAlign::Stretch, .width = width, .height = height});
    cards->addChild(card(std::move(caption), std::move(body), width, true));
    return cards;
  }

  std::unique_ptr<Node> LinkApp::settings(const Presence& presence) {
    const float s = m_scale;
    const PhoneSettings& phone = m_client->phoneSettings();
    const DesktopSettings& desk = m_client->desktopSettings();
    const std::string id = presence.phone.id;
    const auto header = [&](const std::string& title) {
      auto label = text(title, Tokens::typeFootnote, ColorRole::Label2, s);
      label->setFontWeight(FontWeight::SemiBold);
      auto line = ui::row({}, std::move(label));
      line->setPadding(Tokens::spaceP16 * s, 0.0F, Tokens::spaceP8 * s, Tokens::componentListRowInset * s);
      return line;
    };
    const auto group = [&] {
      return ui::column({.align = FlexAlign::Stretch, .padding = Tokens::componentListCardPadding * s,
                         .fill = colorSpecFromRole(ColorRole::Fill1),
                         .radius = Style::scaledRadius(Tokens::componentListCardRadius, s)});
    };
    const float inset = Tokens::componentListRowInset * s;
    std::set<const Flex*> started; // cards that hold a row: the next row gets a rule above it
    const auto row = [&](Flex& card, const char* glyph, const std::string& title, const std::string& hint,
                         std::unique_ptr<Node> trail, std::function<void()> click = nullptr) {
      if (!started.insert(&card).second) {
        auto rule = ui::column({.align = FlexAlign::Stretch});
        rule->setPadding(0.0F, inset, 0.0F, inset + Tokens::sizeSymbol * s + Tokens::spaceP12 * s);
        rule->addChild(ui::separator({}));
        card.addChild(std::move(rule));
      }
      auto r = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP12 * s, .paddingV = Tokens::spaceP8 * s,
                        .paddingH = inset, .minHeight = Tokens::sizeControlL * s});
      r->addChild(ui::glyph({.glyph = std::string(glyph), .glyphSize = Tokens::sizeSymbol * s,
                             .color = colorSpecFromRole(ColorRole::Label1)}));
      auto words = ui::column({.align = FlexAlign::Stretch, .flexGrow = 1.0F});
      words->addChild(text(title, Tokens::typeBody, ColorRole::Label1, s, 2));
      if (!hint.empty()) words->addChild(text(hint, Tokens::typeFootnote, ColorRole::Label2, s, 2));
      r->addChild(std::move(words));
      if (trail) r->addChild(std::move(trail));
      if (click) {
        r->setCover(ui::inputArea({.onClick = [click = std::move(click)](const InputArea::PointerData&) { click(); }}));
      }
      card.addChild(std::move(r));
    };
    const auto toggle = [&](bool on, std::function<void(bool)> change) {
      return ui::toggle({.checked = on, .scale = s, .onChange = std::move(change)});
    };

    auto page = ui::column({.align = FlexAlign::Stretch, .fillWidth = true});
    page->setPadding(0.0F, 0.0F, Tokens::spaceP24 * s, 0.0F);
    // The app's own settings: they need no Link service.
    const auto finish = [&] {
      auto scroll = ui::scrollView({.state = &m_settingsScroll, .contentScale = s, .viewportPaddingH = Tokens::spaceP24 * s,
                                    .maxContentWidth = Tokens::componentLinkSettingsWidth * s,
                                    .contentAlign = ScrollView::ContentAlign::Start, .flexGrow = 1.0F});
      scroll->content()->addChild(std::move(page));
      return scroll;
    };
    if (presence.kind == Presence::Kind::NoDaemon) {
      page->addChild(header(i18n::tr("link.settings.phone")));
      auto card = group();
      row(*card, "plug-connected-x", i18n::tr("link.phone-card.not-running"), i18n::tr("link.settings.start-service"), nullptr);
      page->addChild(std::move(card));
      return finish();
    }
    page->addChild(header(presence.phone.name));
    const auto grant = [&](Flex& card, const char* glyph, const char* title, const char* hint, bool held,
                           const char* feature) {
      row(card, glyph, i18n::tr(title), *hint != 0 ? i18n::tr(hint) : std::string(), 
          toggle(held, [this, id, feature](bool on) { m_client->setGrant(id, feature, on); }));
    };
    auto phoneCard = group();
    if (phone.notificationsOff) {
      row(*phoneCard, "alert-triangle", i18n::tr("link.settings.notif-off-title"),
          i18n::tr("link.settings.notif-off-hint"), nullptr);
    }
    grant(*phoneCard, "bell", "link.settings.notifications", "link.settings.notifications-hint", phone.notifications,
          "notifications");
    grant(*phoneCard, "phone", "link.settings.calls", "link.settings.calls-hint", phone.calls, "calls");
    grant(*phoneCard, "clipboard", "link.settings.clipboard", "link.settings.clipboard-hint", phone.clipboard, "clipboard");
    grant(*phoneCard, "file", "link.settings.files", "link.settings.files-hint", phone.files, "files");
    row(*phoneCard, "arrow-down", i18n::tr("link.settings.auto-accept"), i18n::tr("link.settings.auto-accept-hint"),
        toggle(phone.autoAccept, [this, id](bool on) { m_client->setAutoAccept(id, on); }));
    grant(*phoneCard, "device-mobile", "link.settings.screen", "", phone.screen, "screen");
    grant(*phoneCard, "access-point", "link.settings.hotspot", "link.settings.hotspot-hint",
          phone.nearby, "nearby");
    page->addChild(std::move(phoneCard));

    page->addChild(header(i18n::tr("link.settings.control")));
    auto controlCard = group();
    if (phone.inputBlocked) {
      row(*controlCard, "alert-triangle", i18n::tr("link.settings.input-blocked-title"),
          i18n::tr("link.settings.input-blocked-hint"), nullptr);
    }
    grant(*controlCard, "player-play", "link.settings.media", "link.settings.media-hint",
          phone.media, "media");
    grant(*controlCard, "keyboard", "link.settings.input", "", phone.input, "input");
    grant(*controlCard, "adjustments", "link.settings.system", "link.settings.system-hint", phone.system,
          "system");
    grant(*controlCard, "bell-ringing", "link.settings.ring", "link.settings.ring-hint", phone.ring, "ring");
    page->addChild(std::move(controlCard));

    page->addChild(header(i18n::tr("link.settings.this-computer")));
    auto deskCard = group();
    row(*deskCard, "lock", i18n::tr("link.settings.locked-notifications"), i18n::tr("link.settings.locked-notifications-hint"),
        toggle(desk.notificationsWhileLocked, [this](bool on) { m_client->setNotificationsWhileLocked(on); }));
    row(*deskCard, "device-laptop", i18n::tr("link.settings.nearby"),
        desk.nearbyName.empty() ? i18n::tr("link.settings.quick-share") : i18n::tr("link.settings.nearby-as", "name", desk.nearbyName),
        toggle(desk.nearbyVisible, [this](bool on) { m_client->setNearbyVisible(on); }));
    row(*deskCard, "share", i18n::tr("link.settings.localsend"), i18n::tr("link.settings.localsend-hint"),
        toggle(desk.localSendVisible, [this](bool on) { m_client->setLocalSendVisible(on); }));
    const std::size_t commandCount = desk.commands.size();
    row(*deskCard, "terminal-2", i18n::tr("link.settings.commands"),
        i18n::trp("link.settings.commands-count", static_cast<long>(commandCount)),
        ui::glyph({.glyph = std::string("chevron-right"), .glyphSize = Tokens::sizeSymbolInline * s,
                   .color = colorSpecFromRole(ColorRole::Label2)}),
        [this] {
          m_settingsView = SettingsView::Commands;
          sync();
          m_window->requestRebuild();
        });
    page->addChild(std::move(deskCard));

    auto forget = ui::row({.align = FlexAlign::Center, .gap = Tokens::spaceP8 * s});
    forget->setPadding(Tokens::spaceP24 * s, 0.0F, 0.0F, 0.0F);
    if (m_confirmForget) {
      auto ask = text(i18n::tr("link.settings.forget-ask", "phone", presence.phone.name), Tokens::typeBody,
                      ColorRole::Label1, s, 2);
      ask->setFlexGrow(1.0F);
      forget->addChild(std::move(ask));
      forget->addChild(ui::button({.text = i18n::tr("common.actions.cancel"), .variant = ButtonVariant::Tonal, .onClick = [this] {
                                     m_confirmForget = false;
                                     m_window->requestRebuild();
                                   }}));
      forget->addChild(ui::button({.text = i18n::tr("link.settings.forget"), .variant = ButtonVariant::Destructive, .onClick = [this, id] {
                                     m_confirmForget = false;
                                     m_client->unpair(id);
                                   }}));
    } else {
      Button* button = nullptr;
      forget->addChild(ui::button({.out = &button, .text = i18n::tr("link.settings.forget-phone", "phone", presence.phone.name),
                                   .variant = ButtonVariant::Ghost, .onClick = [this] {
                                     m_confirmForget = true;
                                     m_window->requestRebuild();
                                   }}));
      auto palette = Button::defaultPalette(ButtonVariant::Ghost);
      for (auto* state : {&palette.normal, &palette.hover, &palette.pressed}) state->label = colorSpecFromRole(ColorRole::DangerText);
      button->setCustomPalette(palette);
    }
    page->addChild(std::move(forget));
    return finish();
  }

} // namespace linkapp
