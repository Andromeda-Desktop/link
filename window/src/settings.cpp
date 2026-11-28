// The Settings page and its Commands view.
#include "link_app.h"
#include "link_ui.h"

#include "app/app_window.h"
#include "i18n/i18n.h"
#include "ui/builders.h"
#include "ui/controls/flex.h"
#include "ui/controls/input.h"
#include "ui/style.h"

#include <algorithm>
#include <format>

namespace linkapp {

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
