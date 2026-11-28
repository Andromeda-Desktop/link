#pragma once

#include "link_model.h"
#include "mirror_decoder.h"
#include "core/latest_request.h"

#include <cstdint>
#include <functional>
#include <list>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <filesystem>
#include <string>
#include <vector>

namespace linkapp::bus {
  class Link;
  class Variant;
}

namespace linkapp {

  // org.andromeda.Link1 for the app: phones and the Waiting feed, kept from properties and signals. Every call is
  // async and replies land on the app loop; a reply older than one already applied is dropped.
  class LinkClient {
  public:
    // showInFolder: the desktop shows files in its file manager (DeskApp::showInFolder).
    explicit LinkClient(std::function<void(const std::vector<std::string>&)> showInFolder);
    ~LinkClient();
    LinkClient(const LinkClient&) = delete;
    LinkClient& operator=(const LinkClient&) = delete;

    void setOnChanged(std::function<void()> changed) { m_changed = std::move(changed); }
    // A pairing this app started finished; gets the new phone's id.
    void setOnPaired(std::function<void(const std::string&)> paired) { m_paired = std::move(paired); }

    [[nodiscard]] Presence presence() const { return presenceFrom(m_daemon, m_phones, m_selected); }
    [[nodiscard]] const std::vector<Phone>& phones() const { return m_phones; }
    // Shows this phone from now on, also after a restart; an id that is not paired keeps the default choice.
    void select(const std::string& deviceId);
    [[nodiscard]] const Pair& pair() const { return m_pair; }
    // Opens a pairing window (StartPairing) when this app has none open; the daemon closes it after 120 s.
    void startPairing();
    // Closes this app's pairing window; a late StartPairing reply is then dropped.
    void cancelPairing();
    [[nodiscard]] const Feed& feed() const { return m_feed; }
    // The texts of the phone presence() shows.
    [[nodiscard]] const Messages& messages() const { return m_messages; }
    // The call log of the phone presence() shows, newest first, and whether the phone gives it.
    [[nodiscard]] const std::vector<CallEntry>& calls() const { return m_calls; }
    [[nodiscard]] CallLog callLog() const { return m_callLog; }
    [[nodiscard]] const Activity& activity() const { return m_activity; }
    [[nodiscard]] const Photos& photos() const { return m_photos; }
    [[nodiscard]] const Files& files() const { return m_files; }
    [[nodiscard]] const Mirror& mirror() const { return m_mirror; }
    // OpenMirrorWith on the phone presence() shows, offering mirrorCodecs(); the phone's user allows it there.
    void startMirror();
    void stopMirror();
    // MirrorInput without args: back, home, recents, screen-off.
    void mirrorAction(const std::string& action);
    // A key press and release (Android key code).
    void mirrorKey(std::int32_t code);
    void mirrorText(const std::string& text);
    void mirrorKeyEvent(std::int32_t code, std::int32_t meta, bool down);
    // x, y in 1/10000 of the video; dx, dy the wheel's turn in 1/100 of a step, positive right and down.
    void mirrorWheel(std::int32_t x, std::int32_t y, std::int32_t dx, std::int32_t dy);
    // The phone's own panel off while mirroring goes on; stopMirror turns it back on.
    void setMirrorScreenOff(bool off);
    [[nodiscard]] bool mirrorScreenOff() const { return m_mirrorScreenOff && m_mirror.kind == Mirror::Kind::Showing; }
    // The mirrored screen's largest corner radius in 1/10000 of the video width; 0 until the phone says.
    [[nodiscard]] std::uint16_t mirrorCorner() const { return m_mirrorCorner; }
    // touch-down, touch-move or touch-up at x, y in 1/10000 of the video.
    void mirrorTouch(const std::string& action, std::int32_t x, std::int32_t y);
    void setMirrorTarget(int width, int height);
    [[nodiscard]] int mirrorFps() const { return m_decoder ? m_decoder->fps() : 0; }
    void setOnMirrorFrame(std::function<void(MirrorDecoder::Frame)> frame) { m_mirrorFrame = std::move(frame); }
    // The UI could not import a dmabuf frame: the decoder sends RGBA instead.
    void refuseMirrorDmabuf() {
      if (m_decoder) m_decoder->refuseDmabuf();
    }
    // Lists `path` on the phone presence() shows ("/" for its roots).
    void openFolder(const std::string& path);
    // Deletes a file or folder on the phone, then lists the open folder again.
    void deleteFile(const std::string& phonePath);
    // Copies regular files into the open phone folder; ignored while a copy runs.
    void copyIntoFolder(const std::vector<std::filesystem::path>& paths);
    void cancelCopy();
    void dismissFileOp();
    [[nodiscard]] const FileOp& fileOp() const { return m_fileOp; }
    // Copies the file `name` of the open folder into the downloads folder with ReadFile, 1 MiB at a time.
    void saveFile(const std::string& name);
    // The first PhotosPage of the phone presence() shows; ignored while a first page loads.
    void loadPhotos();
    // The next PhotosPage when the last answer left more on the phone; ignored while a page loads.
    void loadMorePhotos();
    // Shows only photos, only videos, or both, from the first page.
    void setPhotoMedia(Photos::Media media);
    void togglePhoto(std::size_t index);
    void clearPhotoSelection();
    // SavePhoto for each selected photo into the downloads folder.
    void savePhotos();
    // Saves one listed photo into folder; done gets the saved path, empty on failure.
    void savePhotoTo(const std::string& path, const std::string& folder, std::function<void(const std::string&)> done);
    // The PNG thumbnail of a listed photo, or null; the first ask fetches it and calls the thumbnail callback.
    [[nodiscard]] const std::vector<std::uint8_t>* thumbnail(const std::string& path);
    // The PNG of an MMS image part of the shown phone, or null; fetched and cached as thumbnail() does.
    [[nodiscard]] const std::vector<std::uint8_t>* textImage(std::uint64_t part);
    void setOnThumbnail(std::function<void()> ready) { m_thumbnailReady = std::move(ready); }
    // Marks every loaded photo as seen, so Waiting drops "N new photos" until a newer one loads.
    void markPhotosSeen();
    [[nodiscard]] const PhoneSettings& phoneSettings() const { return m_phoneSettings; }
    [[nodiscard]] const DesktopSettings& desktopSettings() const { return m_desktopSettings; }
    void setGrant(const std::string& deviceId, const std::string& feature, bool granted);
    // Replaces Link1 Commands; `done` gets the error text, or nothing on success.
    void setCommands(const std::vector<Command>& commands, std::function<void(std::optional<std::string>)> done);
    void setAutoAccept(const std::string& deviceId, bool on);
    [[nodiscard]] const std::optional<HotspotAsk>& hotspotAsk() const { return m_hotspotAsk; }
    // Answers the open ask; `always` also grants "nearby", so the next ask joins without one.
    void answerHotspot(bool allow, bool always = false);
    void setNotificationsWhileLocked(bool on);
    void setNearbyVisible(bool on);
    void setLocalSendVisible(bool on);
    void unpair(const std::string& deviceId);
    void viewThread(std::optional<std::uint64_t> thread);
    // Sends `body` to `thread`'s address; it shows as sending until Texts holds it, failed on an error.
    void sendText(std::uint64_t thread, const std::string& body);
    void retryText(std::uint64_t serial);

    void reply(const FeedItem& item, const std::string& text);
    void action(const FeedItem& item, const std::string& actionId);
    void dismiss(const FeedItem& item);
    void accept(const FeedItem& offer);
    void decline(const FeedItem& offer);
    void clearAll();
    void acceptTransfer(const std::string& id);
    void declineTransfer(const std::string& id);
    void cancelTransfer(const std::string& id);
    // Sends a failed outgoing transfer's files again (Activity::retryable); the new transfer replaces it.
    void retryTransfer(const std::string& id);
    void clearFinished();
    // Opens the file manager on these files (org.freedesktop.FileManager1.ShowItems).
    void showInFolder(const std::vector<std::string>& paths);
    void connect(const std::string& deviceId);
    // Ends the phone's session and refuses it until connect(); it stays paired.
    void disconnect(const std::string& deviceId);
    void ring(const std::string& deviceId);
    // The call on a connected phone, or null.
    [[nodiscard]] const PhoneCall* call(const std::string& deviceId) const;
    // CallAction: "mute" silences the ringer until the call ends; "decline" ends a ringing or active call.
    void callAction(const std::string& deviceId, const std::string& action);
    enum class PhoneAudio : std::uint8_t { Mic, Sound };
    // Starts or stops the phone's microphone as the default source, or its sound on the default sink.
    void setPhoneAudio(const std::string& deviceId, PhoneAudio audio, bool on);
    // Starts the phone's front camera as a webcam (link-session serves it as a camera node), or stops it.
    void setPhoneCamera(const std::string& deviceId, bool on);
    void switchPhoneCamera(const std::string& deviceId);
    void dial(const std::string& deviceId, const std::string& number);
    void setDnd(const std::string& deviceId, bool on);
    // The window's last system command until it succeeds.
    struct SystemCommand {
      enum class Phase : std::uint8_t { Pending, Failed };
      Phase phase = Phase::Pending;
      std::string device;
      std::string command;
      std::uint32_t value = 0;
      std::string error; // the daemon's or the phone's reason, while Failed
      std::uint64_t serial = 0;
    };
    // set-ringer, set-dnd, set-wifi, set-bluetooth or set-torch; a success also sets the phone's cached value.
    void sendSystemCommand(const std::string& deviceId, const std::string& command, std::uint32_t value);
    [[nodiscard]] const std::optional<SystemCommand>& systemCommand() const { return m_command; }
    // Reads the full photo into the cache for the viewer; a new path drops the last file and any read in flight.
    void openPhoto(const std::string& phonePath);
    void closePhoto();
    [[nodiscard]] const std::optional<PhotoFile>& photoFile() const { return m_photoFile; }
    // SavePhoto into the downloads folder; done gets the saved path, empty on failure.
    void savePhoto(const std::string& phonePath, std::function<void(const std::string&)> done);
    // TakePhoto on the shown phone; the Photos page reloads with the new photo. Cancel only stops waiting here.
    void takePhoto();
    void cancelTakePhoto();
    [[nodiscard]] bool takingPhoto() const { return m_takingPhoto.has_value(); }
    // PhonePlayerCommand: play-pause, next, previous, seek (value ms).
    void playerCommand(const std::string& deviceId, const std::string& command, std::uint64_t value = 0);
    void setMediaVolume(const std::string& deviceId, int percent);
    // Sends text to the phone's clipboard; a lone http(s) URL goes as a link.
    void shareText(const std::string& deviceId, const std::string& text);
    // Texts any number from the phone presence() shows; its thread shows once the phone lists the text.
    void sendTo(const std::string& number, const std::string& body);
    void sendFiles(const std::string& deviceId, const std::vector<std::filesystem::path>& paths);

  private:
    void openMirror(const std::string& device, std::uint64_t session, bool withCodecs);
    struct Download;
    void startDownload(const std::shared_ptr<Download>& download, std::function<bool(Download&)> open);
    void readChunk(const std::shared_ptr<Download>& download);
    void refresh();
    void refreshNearby();
    void send(std::uint64_t serial);
    void changed();

    std::function<void(const std::vector<std::string>&)> m_showInFolder;
    std::unique_ptr<bus::Link> m_proxy;
    std::shared_ptr<bool> m_alive = std::make_shared<bool>(true);
    std::function<void()> m_changed;
    struct Upload;
    std::shared_ptr<Upload> m_upload; // the running copy; Cancel drops it, so its late replies find it gone
    FileOp m_fileOp;
    void writeChunk(const std::shared_ptr<Upload>& upload);
    void finishUpload(const std::shared_ptr<Upload>& upload, std::optional<std::string> error);
    // One MirrorInput call (Link1.xml MirrorInput) while the mirror shows.
    void mirrorInput(const std::string& action, std::map<std::string, bus::Variant> args);
    std::function<void(const std::string&)> m_paired;
    LatestRequest m_pairRequest; // StartPairing calls; a reply applies only to the latest
    std::uint64_t m_sent = 0;    // serial of the newest refresh asked
    std::uint64_t m_applied = 0; // serial of the newest refresh applied
    bool m_daemon = false;
    bool m_pairing = false; // Link1 Pairing: a window is open, ours or another client's
    Pair m_pair;
    std::vector<Phone> m_phones;
    std::optional<SystemCommand> m_command;
    std::uint64_t m_nextCommandId = 0;
    std::optional<PhotoFile> m_photoFile;
    std::uint64_t m_photoOpens = 0;   // the viewer's read; a newer open makes older reads stale
    std::uint64_t m_nextTakeId = 0;
    std::optional<std::uint64_t> m_takingPhoto; // the TakePhoto this app waits for
    std::string m_selected;
    Feed m_feed;
    Messages m_messages;
    Activity m_activity;
    Photos m_photos;
    Files m_files;
    Mirror m_mirror;
    std::uint16_t m_mirrorCorner = 0;
    bool m_mirrorScreenOff = false; // this session sent screen-off last
    std::string m_mirrorDevice;
    std::unique_ptr<MirrorDecoder> m_decoder;
    std::function<void(MirrorDecoder::Frame)> m_mirrorFrame;
    void askPhotos(const Photos::Ask& ask);
    void clearThumbs();
    struct Thumb {
      std::vector<std::uint8_t> png; // empty while in flight
      std::list<std::string>::iterator use;
    };
    static constexpr std::size_t kThumbBytes = std::size_t{48} << 20;
    // Fetches the PNG for key with ask() once and caches it; null until it comes.
    template <typename Ask> const std::vector<std::uint8_t>* cachedImage(const std::string& key, Ask ask);
    std::map<std::string, Thumb> m_thumbs; // key (photo:<device>:<path>, mms:<device>:<part>) -> PNG, at most kThumbBytes; the least recently drawn go first
    std::list<std::string> m_thumbUse;     // keys, most recently drawn first
    std::size_t m_thumbBytes = 0;
    LatestRequest m_thumbEpoch; // clearThumbs() supersedes the replies still in flight
    LatestRequest::Ticket m_thumbTicket = m_thumbEpoch.begin();
    std::function<void()> m_thumbnailReady;
    std::int64_t m_photosSeenS = 0; // the newest photo of m_photos.device() the user saw
    PhoneSettings m_phoneSettings;
    std::optional<HotspotAsk> m_hotspotAsk;
    DesktopSettings m_desktopSettings;
    std::uint64_t m_nearbySent = 0;    // serials of the Quick Share GetAll, as m_sent / m_applied
    std::uint64_t m_nearbyApplied = 0;
    std::vector<CallEntry> m_calls;
    std::map<std::string, PhoneCall> m_phoneCalls; // by device id; only connected phones
    CallLog m_callLog = CallLog::Ready;
    std::set<std::string> m_dismissed; // missed calls the user cleared; Calls keeps reporting them
  };

} // namespace linkapp
