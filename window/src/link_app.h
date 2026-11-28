#pragma once

#include "app/desk_app.h"
#include "app/file_chooser.h"
#include "core/shown_timer.h"
#include "core/timer_manager.h"
#include "render/core/async_texture_cache.h"
#include "render/core/texture_manager.h"
#include "link_client.h"
#include "sky.h"
#include "ui/controls/scroll_view.h"
#include "ui/controls/virtual_grid_view.h"
#include "ui/controls/virtual_list_view.h"

#include <map>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <array>
#include <vector>

class Label;
class ProgressBar;
class Surface;
namespace linkapp {

  enum class Page : std::uint8_t { Home, Waiting, Messages, Calls, Photos, Files, Activity, Mirror, Settings };

  // One card of the Waiting page as a virtual list: one row per item of its section, built the same way to measure
  // and to show.
  class FeedAdapter final : public VirtualListAdapter {
  public:
    // Needs an answer: texts, offered files, missed calls, new photos. Notifications: the phone's notifications.
    enum class Section : std::uint8_t { Answer, Notifications };
    struct Hooks {
      std::function<void(const std::string&)> textTo; // a missed call's "Text": opens a text to the number
      std::function<void()> openPhotos;               // a strip thumbnail clicked
      std::function<void(const std::string&)> dragPhoto; // a strip thumbnail dragged: phone path
    };
    FeedAdapter(LinkClient& client, const float& scale, Section section, Hooks hooks)
        : m_client(client), m_scale(scale), m_section(section), m_hooks(std::move(hooks)) {}
    [[nodiscard]] static Section sectionOf(const FeedItem& item) {
      return item.kind == FeedItem::Kind::Notification ? Section::Notifications : Section::Answer;
    }
    [[nodiscard]] std::size_t itemCount() const override;
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;

  private:
    // renderer is null while measuring: the strip then loads no thumbnails.
    [[nodiscard]] std::unique_ptr<Node> build(Renderer* renderer, std::size_t index, float width);

    [[nodiscard]] const FeedItem& item(std::size_t index) const;

    LinkClient& m_client;
    const float& m_scale;
    Section m_section;
    mutable std::vector<std::size_t> m_index; // feed index of each row, refreshed by itemCount
    std::map<std::string, std::string> m_drafts; // reply text per item key, kept across row recycling
    Hooks m_hooks;
    struct Press {
      float x = 0.0F; // scene position
      float y = 0.0F;
    };
    std::optional<Press> m_press; // a press on a strip thumbnail, until it drags
  };

  // Messages: the thread list, newest first; a click opens the thread.
  class ThreadAdapter final : public VirtualListAdapter {
  public:
    ThreadAdapter(LinkClient& client, const float& scale, const std::optional<std::uint64_t>& selected,
                  const std::optional<SearchHits>& search, std::function<void(std::uint64_t)> open)
        : m_client(client), m_scale(scale), m_selected(selected), m_search(search), m_open(std::move(open)) {}
    [[nodiscard]] std::size_t itemCount() const override;
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] std::uint64_t itemRevision(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;
    void onActivate(std::size_t index) override;

  private:
    // A shown row to its model index: the search hit, or the row itself while not searching.
    [[nodiscard]] std::size_t at(std::size_t row) const { return m_search ? m_search->threads[row] : row; }
    LinkClient& m_client;
    const float& m_scale;
    const std::optional<std::uint64_t>& m_selected;
    const std::optional<SearchHits>& m_search; // set while the sidebar search has a query
    std::function<void(std::uint64_t)> m_open;
  };

  // Messages: one conversation as day lines and bubbles, oldest first, anchored at the bottom.
  class ConversationAdapter final : public VirtualListAdapter {
  public:
    ConversationAdapter(LinkClient& client, const float& scale) : m_client(client), m_scale(scale) {}
    void setRows(std::vector<ConversationRow> rows) { m_rows = std::move(rows); }
    [[nodiscard]] std::size_t itemCount() const override { return m_rows.size(); }
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override { return m_rows[index].key; }
    [[nodiscard]] std::uint64_t itemRevision(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t index) const override;
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;
    void onActivate(std::size_t index) override;

  private:
    // renderer null while measuring: an image bubble has a fixed size.
    [[nodiscard]] std::unique_ptr<Node> build(std::size_t index, float width, Renderer* renderer) const;

    LinkClient& m_client;
    const float& m_scale;
    std::vector<ConversationRow> m_rows;
  };

  // Calls: the phone's call log, newest first, under day heads; a click shows the call's contact beside the list.
  class CallAdapter final : public VirtualListAdapter {
  public:
    CallAdapter(LinkClient& client, const float& scale, const std::optional<std::uint64_t>& selected,
                const std::optional<SearchHits>& search, std::function<void(std::uint64_t)> open)
        : m_client(client), m_scale(scale), m_selected(selected), m_search(search), m_open(std::move(open)) {}
    [[nodiscard]] std::size_t itemCount() const override { return m_search ? m_search->calls.size() : m_client.calls().size(); }
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] std::uint64_t itemRevision(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;
    void onActivate(std::size_t index) override;

  private:
    // A shown row to its model index: the search hit, or the row itself while not searching.
    [[nodiscard]] std::size_t at(std::size_t row) const { return m_search ? m_search->calls[row] : row; }
    // The row's day head, or empty when the row above has the same day.
    [[nodiscard]] std::string dayHead(std::size_t index) const;
    [[nodiscard]] std::unique_ptr<Node> build(Renderer& renderer, std::size_t index, float width, bool hovered) const;

    LinkClient& m_client;
    const float& m_scale;
    const std::optional<std::uint64_t>& m_selected; // the shown call's key
    const std::optional<SearchHits>& m_search;      // set while the sidebar search has a query
    std::function<void(std::uint64_t)> m_open;
  };

  // Photos: the phone's photos and videos page by page under day headers; a tile near the end asks for the next page.
  class PhotoAdapter final : public VirtualGridAdapter {
  public:
    PhotoAdapter(LinkClient& client, const float& scale, const std::optional<SearchHits>& search,
                 std::function<void(std::size_t item)> open)
        : m_client(client), m_scale(scale), m_search(search), m_open(std::move(open)) {}
    // The renderer of the window; thumbnails decode on bind.
    void setRenderer(Renderer* renderer) { m_renderer = renderer; }
    [[nodiscard]] std::size_t itemCount() const override {
      return m_search ? m_search->photos.size() : m_client.photos().items().size();
    }
    [[nodiscard]] std::unique_ptr<Node> createTile() override;
    void bindTile(Node& tile, std::size_t index, bool selected, bool hovered) override;
    // A click opens the viewer; with a selection it toggles, as a press on the corner check always does.
    void onActivate(std::size_t index) override;
    bool onPointerPress(std::size_t index, float x, float y, float width, float height) override;
    PointerReleaseResult onPointerRelease(std::optional<std::size_t> /*index*/) override { return {.rebind = true}; }
    [[nodiscard]] std::string itemTooltip(std::size_t index) const override;
    [[nodiscard]] std::unique_ptr<Node> createSectionHeader() override;
    void bindSectionHeader(Node& header, std::size_t section, float width, float height) override;
    void setDays(std::vector<std::pair<std::size_t, std::string>> days) { m_days = std::move(days); }

  private:
    // A shown row to its model index: the search hit, or the row itself while not searching.
    [[nodiscard]] std::size_t at(std::size_t row) const { return m_search ? m_search->photos[row] : row; }
    LinkClient& m_client;
    const float& m_scale;
    const std::optional<SearchHits>& m_search; // set while the sidebar search has a query
    std::function<void(std::size_t item)> m_open;
    Renderer* m_renderer = nullptr;
    std::vector<std::pair<std::size_t, std::string>> m_days;
    std::map<const Node*, std::string> m_shown; // tile -> path of the thumbnail it shows
  };

  // Files: one phone folder, folders first; a click opens a folder, saves a file, or shows a saved one.
  class FileAdapter final : public VirtualListAdapter {
  public:
    using Menu = std::function<void(std::size_t index, float sceneX, float sceneY)>;
    FileAdapter(LinkClient& client, const float& scale, const std::optional<SearchHits>& search, Menu menu)
        : m_client(client), m_scale(scale), m_search(search), m_menu(std::move(menu)) {}
    [[nodiscard]] std::size_t itemCount() const override {
      return m_search ? m_search->files.size() : m_client.files().entries().size();
    }
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;
    void onActivate(std::size_t index) override;
    [[nodiscard]] bool hasItemMenu() const override { return true; }
    void onItemMenu(std::size_t index, float sceneX, float sceneY) override { m_menu(at(index), sceneX, sceneY); }
    // Opens a folder, saves a file, or shows a saved one; entry indexes files().entries().
    void open(std::size_t entry);

  private:
    // A shown row to its model index: the search hit, or the row itself while not searching.
    [[nodiscard]] std::size_t at(std::size_t row) const { return m_search ? m_search->files[row] : row; }
    LinkClient& m_client;
    const float& m_scale;
    const std::optional<SearchHits>& m_search; // set while the sidebar search has a query
    Menu m_menu;
  };

  // A phone stream this desktop can run: the Home live card and the Activity Running card list it while live.
  struct StreamKind {
    const char* glyph;
    const char* name; // i18n keys
    const char* on;
    std::string Phone::* state;
  };
  inline constexpr std::array kStreams{StreamKind{"camera", "link.home.webcam", "link.home.webcam-on", &Phone::camera},
                                       StreamKind{"microphone", "link.home.mic", "link.home.mic-on", &Phone::mic},
                                       StreamKind{"volume", "link.home.sound", "link.home.sound-on", &Phone::sound}};
  [[nodiscard]] inline bool streamLive(const std::string& state) { return state == "on" || state == "asking"; }

  // A transfer's name: its one file, or the count of its files.
  [[nodiscard]] std::string transferName(const TransferEntry& e);

  // One card of the Activity page as a virtual list: transfers and phone clips since the app started, newest first,
  // and the phone's live streams in Running.
  class TransferAdapter final : public VirtualListAdapter {
  public:
    enum class Section : std::uint8_t { Running, Waiting, Failed, Done };
    struct Hooks {
      std::function<void(const std::string&)> open;                      // opens a local file with its default app
      std::function<void(const std::string&, const std::string&)> stop; // stops a phone stream: device, glyph
    };
    TransferAdapter(LinkClient& client, const float& scale, Section section, Hooks hooks)
        : m_client(client), m_scale(scale), m_section(section), m_hooks(std::move(hooks)) {}
    // A cancelled transfer is Done: it needs nothing from the user.
    [[nodiscard]] static Section sectionOf(const TransferEntry& e);
    [[nodiscard]] std::size_t itemCount() const override;
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;
    void onActivate(std::size_t index) override;

  private:
    using Item = std::variant<const TransferEntry*, const ClipEntry*, const StreamKind*>;
    [[nodiscard]] std::unique_ptr<Node> build(std::size_t index, bool hovered) const;
    // A Done row's click: open its one file, show its files in the folder, or copy its clip. Null for other rows.
    [[nodiscard]] std::function<void()> activation(std::size_t index) const;

    LinkClient& m_client;
    const float& m_scale;
    Section m_section;
    Hooks m_hooks;
    mutable std::vector<Item> m_rows; // refreshed by itemCount
    mutable Phone m_phone;            // the shown phone, for its stream rows
  };

  class LinkApp final : public DeskApp {
  public:
    static constexpr const char* kAppId = "dev.andromeda.Link";
    LinkApp();
    ~LinkApp() override;

  protected:
    AppWindow& createWindow() override;
    // "page" with a page name ("messages") shows that page.
    void action(const std::string& name, const std::vector<std::string>& parameter, const std::string& activationToken) override;

  private:
    void show(Page page);
    void updateCaption();
    [[nodiscard]] std::unique_ptr<Node> sidebar(float width);
    // The lead player of the shown phone while it is connected; null otherwise.
    [[nodiscard]] const PhonePlayer* player() const;
    // The column right of the sidebar: Home's sky and sheet, or a page's sky band over its sheet.
    [[nodiscard]] std::unique_ptr<Node> content(Renderer& renderer, float width, float height);
    // The shown page's own content, inside the sheet.
    [[nodiscard]] std::unique_ptr<Node> pageBody(Renderer& renderer, float width, float height);
    std::unique_ptr<Node> waiting(Renderer& renderer, float width, float height);
    [[nodiscard]] static std::string pageTitle(Page page);
    [[nodiscard]] std::unique_ptr<Node> home(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Node> homeIdentity(const Presence& presence, float s);
    // The running cards; each card's star goes into `anchors`.
    [[nodiscard]] std::unique_ptr<Node> homeLive(const Presence& presence, float width, float s, std::vector<SkyAnchor>& anchors);
    [[nodiscard]] std::unique_ptr<Flex> homeSheet(Renderer& renderer, const Presence& presence, float width, float height);
    // The column under the sky and the sheet: the sky's own colour.
    [[nodiscard]] ColorSpec skyFill() const;
    [[nodiscard]] std::unique_ptr<Node> skyColumn(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Flex> phoneLine(std::unique_ptr<Node> stat);
    void phoneMenu(float sceneX, float sceneY);
    [[nodiscard]] std::unique_ptr<Node> callBar(Renderer& renderer, float width, float top);
    // The sky with the call bar on `front`: every sky build goes through here.
    [[nodiscard]] std::unique_ptr<Node> skyWithCall(Renderer& renderer, const SkyLook& look, float width, float height, std::unique_ptr<Node> front,
                                                    std::vector<SkyAnchor> anchors);
    [[nodiscard]] std::unique_ptr<Node> bandFront(float width, float height, std::vector<SkyAnchor>& anchors);
    [[nodiscard]] std::unique_ptr<Flex> sheetPlane(float width, float height, bool grab);
    [[nodiscard]] SkyLife skyLife() const;
    [[nodiscard]] SkyLook skyLook(SkyMode mode) const;
    void stopStream(const std::string& deviceId, std::string_view glyph);
    // The media card's seek ticks each second while it shows a playing player.
    void syncLiveTick();
    void tickLive();
    // Pair a Phone: the QR and code of this app's pairing window.
    [[nodiscard]] std::unique_ptr<Node> pair(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Node> messages(float width, float height);
    [[nodiscard]] std::unique_ptr<Node> compose(float s);
    [[nodiscard]] std::unique_ptr<Node> calls(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Node> activity(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Node> photos(Renderer& renderer, float width, float height);
    // PhotoLightbox (design-link-parity handoff.yaml) over the Photos card: the photo on the dark stage, the toolbar.
    [[nodiscard]] std::unique_ptr<Node> viewer(Renderer& renderer, float width, float height);
    void viewPhoto(std::optional<std::string> path);
    // The photo `delta` steps from the shown one in the grid's order (search hits while searching); none at the ends.
    [[nodiscard]] std::optional<std::string> photoStep(int delta) const;
    void copyPhoto();
    [[nodiscard]] std::unique_ptr<Node> files(float width, float height);
    [[nodiscard]] std::unique_ptr<Node> mirror(float width, float height);
    // Shows a decoded frame: a dmabuf frame is imported without a copy, an RGBA frame is uploaded into the one
    // mirror texture (same size: in place); then repaints the video.
    void showMirrorFrame(MirrorDecoder::Frame frame);
    // Frees the shown mirror frame (all = also the texture) once the GPU no longer samples it.
    void retireMirrorFrame(bool all);
    [[nodiscard]] std::string mirrorStat() const;
    [[nodiscard]] std::unique_ptr<Node> settings(const Presence& presence);
    [[nodiscard]] std::unique_ptr<Node> commands(const Presence& presence, float width, float height);
    void commandsChanged(bool changed);
    void commandGrip(std::size_t row, const InputArea::PointerData& data, bool motion);
    [[nodiscard]] std::unique_ptr<Node> card(std::vector<std::unique_ptr<Node>> caption, std::unique_ptr<Node> body,
                                             float width, bool windowButtons);
    // Cards mode and the viewed thread follow the page and presence.
    void sync();
    // Shows the hotspot sheet while the client holds an ask, and closes it when the ask ends.
    void syncHotspotSheet();
    // The file chooser; done gets the picked paths, never an empty list.
    void chooseFiles(std::function<void(std::vector<std::filesystem::path>)> done);
    void chooseAndSend(const std::string& deviceId);
    void openLocalFile(const std::string& path);
    // Drags a phone photo out: saved into the cache, offered as a file URI once there.
    void dragPhoto(const std::string& path);
    // Shows this phone; the open page reloads its data from it.
    void choosePhone(const std::string& deviceId);
    void mirrorKeyInput(const InputArea::KeyData& data);
    void releaseMirrorKeys();
    void confirmLock();
    void fileMenu(std::size_t index, float sceneX, float sceneY);
    void setQuery(std::string query);
    void refreshSearch();
    void confirmDelete(const FileEntry& entry);
    void registerDropTarget();
    void dropped(const std::string& uris);
    // Where a drop over the window goes: None while nothing is dragged over it.
    enum class DropCue : std::uint8_t { None, Send, CopyIn };
    [[nodiscard]] DropCue dropCueHere() const;
    DropCue m_dropCue = DropCue::None;
    Surface* m_dropSurface = nullptr;
    // The pair view covers the page while this app pairs another phone (Pair Another Phone…).
    [[nodiscard]] bool pairShown() const;
    // The page body fills the sheet edge to edge (pages.html .split): Messages and Calls while a phone is there.
    [[nodiscard]] bool splitPage() const;
    [[nodiscard]] std::unique_ptr<Flex> pageSearch(const std::string& placeholder, float s);
    // Esc: cancels that pairing, else closes the Forget question.
    [[nodiscard]] bool dismissable() const;
    void dismiss();
    // The conversation card shows a thread or a new message, never both: these two are the only ways to change it.
    void openThread(std::optional<std::uint64_t> id);
    // A new message to `to` (name or number); an existing thread with that number opens instead.
    void openCompose(const std::string& to);

    std::unique_ptr<LinkClient> m_client;
    std::array<std::unique_ptr<FeedAdapter>, 2> m_feed; // by FeedAdapter::Section
    std::array<ScrollViewState, 2> m_feedScroll;
    ScrollViewState m_sideScroll;
    std::unique_ptr<ThreadAdapter> m_threads;
    std::unique_ptr<ConversationAdapter> m_conversation;
    ScrollViewState m_threadScroll;
    ScrollViewState m_conversationScroll;
    std::optional<std::uint64_t> m_thread; // the selected thread
    std::string m_query;                   // the sidebar search field and the Messages "Search texts" field
    Input* m_searchInput = nullptr;
    Input* m_pageSearchInput = nullptr; // the page's own field on m_query (Messages, Photos)
    std::optional<SearchHits> m_search;    // the query's hits; set exactly while m_query is not empty
    struct Compose {
      std::string to;   // name or number as typed
      std::string body;
      std::string sent; // the number last texted, until the phone lists its thread
    };
    std::optional<Compose> m_compose; // set only while m_thread is empty
    std::map<std::uint64_t, std::string> m_threadDrafts;
    std::unique_ptr<CallAdapter> m_calls;
    ScrollViewState m_callScroll;
    std::unique_ptr<FileAdapter> m_files;
    ScrollViewState m_fileScroll;
    std::unique_ptr<PhotoAdapter> m_photos;
    ScrollViewState m_photoScroll;
    std::array<VirtualListView*, 2> m_feedList{}; // the Waiting cards of the last build, null off Waiting
    VirtualListView* m_conversationList = nullptr; // the open conversation of the last build, null off Messages
    VirtualGridView* m_photoGrid = nullptr; // the grid of the last build, null off the Photos page
    std::array<std::unique_ptr<TransferAdapter>, 4> m_transfers; // by TransferAdapter::Section
    std::array<ScrollViewState, 4> m_transferScroll;
    ScrollViewState m_settingsScroll;
    bool m_confirmForget = false; // Settings: the Forget row asks in place
    enum class SettingsView : std::uint8_t { Main, Commands };
    SettingsView m_settingsView = SettingsView::Main;
    CommandsEditor m_commands;
    ScrollViewState m_commandsScroll;
    // Commands row drag: Pressed on a grip, Dragging past spaceP4; the release moves the row to `target`.
    // rows holds this build's row nodes; a rebuild waits while the button is down, so they live through a drag.
    struct CommandDrag {
      enum class Kind : std::uint8_t { Idle, Pressed, Dragging };
      Kind kind = Kind::Idle;
      std::size_t from = 0;
      Command moving; // the dragged row's command at the press
      std::size_t target = 0;
      float startY = 0.0F;
      std::vector<Flex*> rows;
      std::vector<float> tops;           // scene top of each row at the press
      std::vector<float> shift;          // each other row's translation target
      std::vector<std::uint32_t> springs; // AnimationManager ids per row
      ColorSpec restFill;                 // the dragged row's fill before the drag
    };
    CommandDrag m_commandDrag;
    std::string m_hotspotSheet;   // device id + ssid of the ask the sheet shows; empty while none shows
    std::string m_dial; // the "Name or number" field
    std::optional<std::uint64_t> m_call; // the call whose contact Calls shows (CallAdapter key); the newest when unset
    AppWindow* m_window = nullptr;
    std::unique_ptr<Sky> m_sky;
    ScrollViewState m_homeScroll;
    struct LiveView {
      ProgressBar* seek = nullptr;
      Label* position = nullptr;
    };
    LiveView m_liveView;              // of Home's last build
    ShownTimer m_liveTick;            // 1 s, only while Home shows a playing player
    std::optional<std::string> m_viewing; // phone path of the photo the viewer shows
    std::optional<std::size_t> m_photoScrollTo; // grid row the next Photos build brings into view
    std::size_t m_searchCaret = 0;    // the search field's caret, for the field the next build makes
    std::map<std::string, std::string> m_viewSaved; // phone path to its saved copy, from the viewer's Save
    std::pair<int, int> m_viewImage;  // the shown photo's decoded size, 0 until it decodes
    AsyncTextureCache m_viewCache;    // the viewer's full-size decode, off the UI thread
    std::unique_ptr<desk::FileChooser> m_chooser;
    Page m_page = Page::Home;
    float m_scale = 1.0F;
    Renderer* m_renderer = nullptr;   // the window's, from the last content build
    TextureHandle m_mirrorTexture;    // owned here; the video Image shows it without owning it
    std::shared_ptr<void> m_mirrorHold; // non-null when m_mirrorTexture wraps a decoder dmabuf
    struct RetiredFrame {
      std::shared_ptr<void> hold;
      int fence = -1; // signals once no submitted paint samples the frame; -1: already done
    };
    std::vector<RetiredFrame> m_mirrorRetired;
    Image* m_mirrorImage = nullptr;   // of the last build, null off the Mirror page
    Label* m_mirrorStat = nullptr;
    bool m_touching = false;          // a primary press on the video is down
    InputArea* m_mirrorArea = nullptr; // the video of the last build, null off a showing mirror
    std::map<std::int32_t, std::int32_t> m_mirrorHeld; // Android key code to meta of keys down on the phone
  };

} // namespace linkapp
