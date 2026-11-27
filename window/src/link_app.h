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
#include <vector>

class Label;
class ProgressBar;
class Surface;
namespace linkapp {

  enum class Page : std::uint8_t { Home, Waiting, Messages, Calls, Photos, Files, Activity, Mirror, Settings };

  // The Waiting feed as a virtual list: one row per item, built the same way to measure and to show.
  class FeedAdapter final : public VirtualListAdapter {
  public:
    struct Hooks {
      std::function<void(const std::string&)> textTo; // a missed call's "Text": opens a text to the number
      std::function<void()> openPhotos;               // a strip thumbnail clicked
      std::function<void(const std::string&)> dragPhoto; // a strip thumbnail dragged: phone path
    };
    FeedAdapter(LinkClient& client, const float& scale, Hooks hooks) : m_client(client), m_scale(scale), m_hooks(std::move(hooks)) {}
    [[nodiscard]] std::size_t itemCount() const override;
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;

  private:
    // renderer is null while measuring: the strip then loads no thumbnails.
    [[nodiscard]] std::unique_ptr<Node> build(Renderer* renderer, std::size_t index, float width);

    LinkClient& m_client;
    const float& m_scale;
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

  // Calls: the phone's call log, newest first, grouped Today / Earlier.
  class CallAdapter final : public VirtualListAdapter {
  public:
    CallAdapter(LinkClient& client, const float& scale, const std::optional<SearchHits>& search)
        : m_client(client), m_scale(scale), m_search(search) {}
    [[nodiscard]] std::size_t itemCount() const override { return m_search ? m_search->calls.size() : m_client.calls().size(); }
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;

  private:
    // A shown row to its model index: the search hit, or the row itself while not searching.
    [[nodiscard]] std::size_t at(std::size_t row) const { return m_search ? m_search->calls[row] : row; }
    [[nodiscard]] std::unique_ptr<Node> build(std::size_t index) const;

    LinkClient& m_client;
    const float& m_scale;
    const std::optional<SearchHits>& m_search; // set while the sidebar search has a query
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

  // Activity: transfers and phone clips since the app started, newest first, with actions per state.
  class TransferAdapter final : public VirtualListAdapter {
  public:
    TransferAdapter(LinkClient& client, const float& scale, std::function<void(const std::string&)> open)
        : m_client(client), m_scale(scale), m_open(std::move(open)) {}
    [[nodiscard]] std::size_t itemCount() const override { return m_client.activity().rowCount(); }
    [[nodiscard]] std::uint64_t itemKey(std::size_t index) const override;
    [[nodiscard]] bool itemInteractive(std::size_t /*index*/) const override { return true; }
    [[nodiscard]] float measureItem(Renderer& renderer, std::size_t index, float width) override;
    [[nodiscard]] std::unique_ptr<Node> createItem() override;
    void bindItem(Renderer& renderer, Node& item, std::size_t index, float width, bool hovered) override;

  private:
    [[nodiscard]] std::unique_ptr<Node> build(std::size_t index) const;
    [[nodiscard]] std::unique_ptr<Node> buildTransfer(const TransferEntry& e) const;
    [[nodiscard]] std::unique_ptr<Node> buildClip(const ClipEntry& e) const;

    LinkClient& m_client;
    const float& m_scale;
    std::function<void(const std::string&)> m_open; // opens a local file with its default app
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
    [[nodiscard]] static std::string pageTitle(Page page);
    [[nodiscard]] std::unique_ptr<Node> home(Renderer& renderer, float width, float height);
    [[nodiscard]] std::unique_ptr<Node> homeIdentity(const Presence& presence, float s);
    // The running cards; each card's star goes into `anchors`.
    [[nodiscard]] std::unique_ptr<Node> homeLive(const Presence& presence, float width, float s, std::vector<SkyAnchor>& anchors);
    [[nodiscard]] std::unique_ptr<Flex> homeSheet(Renderer& renderer, const Presence& presence, float width, float height);
    // The column under the sky and the sheet: the sky's own colour.
    [[nodiscard]] ColorSpec skyFill() const;
    [[nodiscard]] std::unique_ptr<Node> bandFront(float width, float height);
    [[nodiscard]] std::unique_ptr<Flex> sheetPlane(float width, float height, bool grab);
    [[nodiscard]] SkyLife skyLife() const;
    [[nodiscard]] SkyLook skyLook(SkyMode mode) const;
    void stopStream(const std::string& deviceId, std::string_view glyph);
    // The media card's seek ticks each second while it shows a playing player.
    void syncLiveTick();
    void tickLive();
    // Pair a Phone: the QR and code of this app's pairing window.
    [[nodiscard]] std::unique_ptr<Node> pair(Renderer& renderer, float s);
    [[nodiscard]] std::unique_ptr<Node> messages(float width, float height);
    [[nodiscard]] std::unique_ptr<Node> compose(float s);
    [[nodiscard]] std::unique_ptr<Node> calls();
    [[nodiscard]] std::unique_ptr<Node> activity();
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
    // Esc: cancels that pairing, else closes the Forget question.
    [[nodiscard]] bool dismissable() const;
    void dismiss();
    // The conversation card shows a thread or a new message, never both: these two are the only ways to change it.
    void openThread(std::optional<std::uint64_t> id);
    // A new message to `to` (name or number); an existing thread with that number opens instead.
    void openCompose(const std::string& to);

    std::unique_ptr<LinkClient> m_client;
    std::unique_ptr<FeedAdapter> m_feed;
    ScrollViewState m_feedScroll;
    ScrollViewState m_sideScroll;
    std::unique_ptr<ThreadAdapter> m_threads;
    std::unique_ptr<ConversationAdapter> m_conversation;
    ScrollViewState m_threadScroll;
    ScrollViewState m_conversationScroll;
    std::optional<std::uint64_t> m_thread; // the selected thread
    std::string m_query;                   // the sidebar search field
    Input* m_searchInput = nullptr;
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
    VirtualListView* m_feedList = nullptr;  // the Waiting list of the last build, null off Waiting
    VirtualListView* m_conversationList = nullptr; // the open conversation of the last build, null off Messages
    VirtualGridView* m_photoGrid = nullptr; // the grid of the last build, null off the Photos page
    std::unique_ptr<TransferAdapter> m_transfers;
    ScrollViewState m_transferScroll;
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
