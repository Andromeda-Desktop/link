// Model-based test: random event sequences against the feed, presence and transfer rules, with the invariants checked
// after every event.
#include "link_model.h"
#include "i18n/i18n.h"

#include <algorithm>
#include <array>
#include <cstdio>
#include <ctime>
#include <format>
#include <ranges>
#include <tuple>
#include <cstdlib>
#include <map>
#include <optional>
#include <random>
#include <set>

using namespace linkapp;

namespace {

  int failures = 0;

  void expect(bool ok, const char* what, unsigned seed, int step) {
    if (ok) return;
    std::fprintf(stderr, "FAIL seed %u step %d: %s\n", seed, step, what);
    ++failures;
  }

  void feedInvariants(const Feed& feed, unsigned seed, int step) {
    const auto& items = feed.items();
    expect(items.size() <= Feed::kMax, "at most kMax items", seed, step);
    std::vector<std::string_view> keys;
    keys.reserve(items.size());
    for (std::size_t i = 0; i < items.size(); ++i) {
      keys.push_back(items[i].key);
      if (i > 0) expect(items[i - 1].atMs >= items[i].atMs, "newest first", seed, step);
    }
    std::ranges::sort(keys);
    expect(std::ranges::adjacent_find(keys) == keys.end(), "one item per key", seed, step);
    const std::int64_t now = items.empty() ? 0 : items.front().atMs;
    const std::size_t split = feed.earlierStart(now);
    for (std::size_t i = 0; i < items.size(); ++i) {
      expect((i < split) == (now - items[i].atMs < Feed::kNowMs), "Now/Earlier split", seed, step);
    }
  }

  // A snapshot gives back every item but offers, in order and field for field; a cut one gives nothing.
  void snapshotInvariants(const Feed& feed, unsigned seed, int step) {
    const std::vector<Phone> phones{{.id = "p", .name = "Pé \"8\"", .connected = true, .battery = 7, .lastSeen = 9, .locked = 1}};
    const std::string text = encodeSnapshot(phones, feed, {"c:p:1"});
    const auto back = decodeSnapshot(text);
    expect(back.has_value(), "snapshot decodes", seed, step);
    if (!back) return;
    std::vector<const FeedItem*> kept;
    for (const FeedItem& item : feed.items()) {
      if (item.kind != FeedItem::Kind::Offer) kept.push_back(&item);
    }
    bool same = back->items.size() == kept.size() && back->dismissed == std::set<std::string>{"c:p:1"} &&
                back->phones.size() == 1 && back->phones[0].name == phones[0].name && back->phones[0].locked == 1 &&
                back->phones[0].battery == 7 && back->phones[0].connected;
    for (std::size_t i = 0; same && i < kept.size(); ++i) {
      const FeedItem& a = *kept[i];
      const FeedItem& b = back->items[i];
      same = a.kind == b.kind && a.key == b.key && a.deviceId == b.deviceId && a.app == b.app && a.who == b.who && a.body == b.body &&
             a.atMs == b.atMs && a.actions.size() == b.actions.size();
    }
    expect(same, "snapshot round trip keeps every non-offer item", seed, step);
    expect(!decodeSnapshot(text.substr(0, text.size() / 2)), "a cut snapshot decodes to nothing", seed, step);
  }

  void runFeed(unsigned seed) {
    std::mt19937 rng(seed);
    Feed feed;
    std::uniform_int_distribution<int> op(0, 9), key(0, 300), age(0, 3 * 3600 * 1000);
    const std::int64_t base = 1'800'000'000'000;
    for (int step = 0; step < 4000; ++step) {
      const std::string k = "n:" + std::to_string(key(rng));
      const int o = op(rng);
      if (o < 5) {
        FeedItem item;
        item.key = k;
        item.kind = (key(rng) % 2) ? FeedItem::Kind::Offer : FeedItem::Kind::Notification;
        if (item.kind == FeedItem::Kind::Notification) item.app = "app" + k;
        item.atMs = base - age(rng);
        feed.upsert(item);
        expect(feed.find(k).has_value() || feed.items().size() == Feed::kMax, "upserted item present", seed, step);
      } else if (o < 7) {
        const bool had = feed.find(k).has_value();
        expect(feed.remove(k) == had, "remove reports presence", seed, step);
        expect(!feed.find(k), "removed item gone", seed, step);
      } else {
        const auto to = static_cast<Transfer>(key(rng) % 4);
        const auto at = feed.find(k);
        const auto before = at ? std::optional(feed.items()[*at]) : std::nullopt;
        const bool moved = feed.moveTransfer(k, to, 5, 10);
        const bool allowed = before && before->kind == FeedItem::Kind::Offer && transferMayMove(before->transfer, to);
        expect(moved == allowed, "transfer moves only by the rules", seed, step);
        if (before && !moved) expect(feed.items()[*at].transfer == before->transfer, "rejected move changes nothing", seed, step);
      }
      feedInvariants(feed, seed, step);
      if (step % 97 == 0) snapshotInvariants(feed, seed, step);
    }
  }

  void presence() {
    using K = Presence::Kind;
    const Phone away{.id = "a", .name = "A"};
    const Phone live{.id = "b", .name = "B", .connected = true, .locked = 0};
    Phone locked = live;
    locked.locked = 1;
    // Every combination: the result names a phone exactly when one is paired.
    for (int daemon = 0; daemon < 2; ++daemon) {
      for (const auto& phones : {std::vector<Phone>{}, {away}, {away, live}, {away, locked}})
      for (const std::string selected : {"", "a", "b", "gone"}) {
        const Presence p = presenceFrom(daemon, phones, selected);
        const bool named = !p.phone.id.empty();
        const bool paired = std::ranges::find(phones, selected, &Phone::id) != phones.end();
        expect(named == (daemon && !phones.empty()), "phone named only when shown", 0, 0);
        if (named && paired) expect(p.phone.id == selected, "a paired selected phone is the one shown", 0, 0);
        if (!daemon) expect(p.kind == K::NoDaemon, "no daemon wins", 0, 0);
        if (named) {
          expect(p.phone.connected == (p.kind == K::Connected || p.kind == K::Locked), "connected kinds", 0, 0);
          expect((p.kind == K::Locked) == (p.phone.locked == 1), "locked only when the phone says so", 0, 0);
        }
      }
    }
  }

  // Random Texts snapshots, receipts, views and sends against a reference unread set and the conversation rules.
  void runMessages(unsigned seed) {
    using K = ConversationRow::Kind;
    std::mt19937 rng(seed);
    std::uniform_int_distribution<int> op(0, 7), threadOf(1, 6), pick(0, 39);
    const std::int64_t now = 1'763'600'000'000; // 2025-11-20 UTC
    std::vector<Text> universe;
    for (std::uint64_t id = 1; id <= 40; ++id) {
      const auto thread = static_cast<std::uint64_t>(threadOf(rng));
      std::vector<std::uint64_t> images;
      for (int k = pick(rng) % 4 == 0 ? 1 + pick(rng) % 2 : 0; k > 0; --k) images.push_back(id * 10 + static_cast<std::uint64_t>(k));
      universe.push_back({.id = id, .thread = thread, .address = std::format("+49 151 000{}", thread),
                          .body = !images.empty() && pick(rng) % 2 == 0 ? std::string() : std::format("t{}", id),
                          .atMs = now - static_cast<std::int64_t>(pick(rng)) * 7 * 3600 * 1000,
                          .status = static_cast<Text::Status>(pick(rng) % 5), .images = std::move(images)});
    }
    Messages m;
    std::vector<Text> shown;
    std::set<std::uint64_t> unread; // reference: received, not yet seen in a viewed thread
    std::optional<std::uint64_t> viewing;
    std::uint64_t textId = 1000;
    expect(m.load() == Messages::Load::Loading, "Loading before the first Texts", seed, 0);
    for (int step = 1; step <= 400; ++step) {
      switch (op(rng)) {
      case 0:
      case 1: { // a new snapshot: a random subset, plus the sent copies of what is sending
        shown.clear();
        for (const Text& t : universe) {
          if (pick(rng) % 2 == 0) shown.push_back(t);
        }
        for (const Thread& t : m.threads()) {
          for (const ConversationRow& row : m.conversation(t.id, now)) {
            if (row.kind == K::Sending && pick(rng) % 2 == 0) {
              shown.push_back({.id = ++textId, .thread = t.id, .address = t.address, .body = row.text, .atMs = now, .status = Text::Status::Sent});
            }
          }
        }
        m.setTexts("phone", shown);
        for (const Text& t : shown) {
          for (const Thread& th : m.threads()) {
            for (const ConversationRow& row : m.conversation(th.id, now)) {
              if (t.mine() && th.id == t.thread && row.kind == K::Sending) {
                expect(row.text != t.body, "a sent text Texts holds leaves the outgoing list", seed, step);
              }
            }
          }
        }
        break;
      }
      case 2: {
        const std::uint64_t id = static_cast<std::uint64_t>(pick(rng)) + 1;
        m.received(id);
        unread.insert(id);
        break;
      }
      case 3:
        viewing = pick(rng) % 4 == 0 ? std::nullopt : std::optional<std::uint64_t>(threadOf(rng));
        m.view(viewing);
        break;
      case 4:
        m.setContacts({{.name = "Maya", .numbers = {"0151 0001"}}, {.name = "Dad", .numbers = {"+491510002", "0151 0003"}}});
        break;
      case 5: {
        const auto thread = static_cast<std::uint64_t>(threadOf(rng));
        const std::uint64_t serial = m.send(thread, std::format("s{}", step), now);
        expect((serial != 0) == (m.thread(thread) != nullptr), "send needs a known thread", seed, step);
        break;
      }
      default: {
        const auto serial = static_cast<std::uint64_t>(pick(rng) % 8) + 1;
        const Outgoing* out = m.outgoing(serial);
        const auto before = out ? std::optional(out->state) : std::nullopt;
        const bool fail = pick(rng) % 2 == 0;
        const bool moved = fail ? m.sendFailed(serial) : m.retry(serial);
        const auto want = fail ? Outgoing::State::Sending : Outgoing::State::Failed;
        expect(moved == (before == want), "Sending -> Failed and Failed -> Sending only", seed, step);
        break;
      }
      }
      if (viewing) {
        for (const Text& t : shown) {
          if (t.thread == *viewing) unread.erase(t.id);
        }
      }

      const auto& threads = m.threads();
      std::set<std::uint64_t> ids;
      for (std::size_t i = 0; i < threads.size(); ++i) {
        const Thread& th = threads[i];
        expect(ids.insert(th.id).second, "one row per thread", seed, step);
        if (i > 0) expect(threads[i - 1].atMs >= th.atMs, "threads newest first", seed, step);
        int want = 0;
        const Text* newest = nullptr;
        for (const Text& t : shown) {
          if (t.thread != th.id) continue;
          if (!t.mine() && unread.contains(t.id)) ++want;
          if (newest == nullptr || std::tie(t.atMs, t.id) > std::tie(newest->atMs, newest->id)) newest = &t;
        }
        expect(th.unread == want, "unread = received texts not yet seen in a viewed thread", seed, step);
        expect(newest != nullptr && th.last == (newest->body.empty() && !newest->images.empty() ? "Photo" : newest->body),
               "the row shows the newest text, Photo for an MMS of images only", seed, step);
        const std::string expectName = numberKey(th.address) == numberKey("0151 0001") ? "Maya" : "";
        if (!expectName.empty() && th.name != th.address) expect(th.name == expectName, "contact name by number", seed, step);
        if (viewing && th.id == *viewing) expect(th.unread == 0, "the viewed thread has no unread", seed, step);

        const auto rows = m.conversation(th.id, now);
        std::size_t bubbles = 0;
        for (std::size_t r = 0; r < rows.size(); ++r) {
          if (rows[r].kind == K::Day) {
            expect(r == 0 || rows[r - 1].runEnd, "the bubble before a day line ends its run", seed, step);
            expect(r + 1 < rows.size() && rows[r + 1].kind != K::Day, "a day line has bubbles", seed, step);
            continue;
          }
          ++bubbles;
          if (r > 0) expect(rows[r - 1].atMs <= rows[r].atMs, "oldest first", seed, step);
        }
        std::size_t want2 = 0;
        for (const Text& t : shown) {
          if (t.thread == th.id) want2 += t.images.size() + (t.body.empty() && !t.images.empty() ? 0 : 1);
        }
        for (std::uint64_t serial = 1; serial <= 400; ++serial) {
          if (const Outgoing* out = m.outgoing(serial); out && out->thread == th.id) ++want2;
        }
        expect(bubbles == want2, "one bubble per image, per text's words and per outgoing text", seed, step);
        using N = ConversationRow::Note;
        const auto lastMine = std::ranges::find_if(rows.rbegin(), rows.rend(), [](const ConversationRow& row) {
          return row.kind == K::Out || row.kind == K::Sending || row.kind == K::Failed;
        });
        for (const ConversationRow& row : rows) {
          if (row.kind != K::In && row.kind != K::Out) continue;
          const auto text = std::ranges::find_if(shown, [&](const Text& t) {
            return t.thread == th.id && (t.id == row.key || std::ranges::contains(t.images, row.image) && row.image != 0);
          });
          expect(text != shown.end(), "a bubble comes from a text of its thread", seed, step);
          const bool newest = lastMine != rows.rend() && &*lastMine == &row;
          const N want3 = text->status == Text::Status::Failed    ? N::NotSent
                          : text->status == Text::Status::Sending ? N::Sending
                          : newest && row.image == 0 && text->status == Text::Status::Delivered ? N::Delivered
                                                                                                  : N::None;
          expect(row.note == want3, "Delivered only on the newest of mine; Sending and Not sent as the phone says", seed, step);
        }
        expect(rows.empty() || rows.back().runEnd, "the last bubble ends its run", seed, step);
      }
      if (!shown.empty() || m.load() == Messages::Load::Ready) {
        std::set<std::uint64_t> want(std::from_range, shown | std::views::transform(&Text::thread));
        expect(ids == want, "a thread per thread id in Texts", seed, step);
      }
    }
  }

  // Random Transfer* signals: one entry per id, newest first, only allowed moves, declined offers gone.
  void runActivity(unsigned seed) {
    std::mt19937 rng(seed);
    std::uniform_int_distribution<int> op(0, 8), idOf(0, 11), statusOf(0, 6);
    const std::array<const char*, 7> statuses{"done", "failed", "declined", "no-space", "too-large", "busy", "cancelled"};
    Activity a;
    std::uint64_t clipId = 0;
    std::int64_t now = 1'000'000;
    for (int step = 1; step <= 300; ++step) {
      now += 250;
      const std::string id = std::format("t{}", idOf(rng));
      const auto before = std::ranges::find(a.items(), id, &TransferEntry::id);
      const std::optional<Transfer> was = before == a.items().end() ? std::nullopt : std::optional(before->state);
      const auto after = [&] {
        const auto it = std::ranges::find(a.items(), id, &TransferEntry::id);
        return it == a.items().end() ? std::nullopt : std::optional(it->state);
      };
      switch (op(rng)) {
      case 0:
        a.offered(id, "p1", {{"a.pdf", 1000}, {"b.jpg", 500}}, now);
        expect(after().has_value(), "an offer shows", seed, step);
        if (was) expect(after() == was, "a repeated offer changes nothing", seed, step);
        break;
      case 1:
      case 2: {
        const bool moved = a.progress(id, 300, 1500, now);
        expect(moved == (!was || transferMayMove(*was, Transfer::Running)), "progress only where Running is allowed", seed, step);
        if (moved) expect(after() == Transfer::Running, "progress means Running", seed, step);
        break;
      }
      case 3:
      case 4: {
        const std::string status = statuses[static_cast<std::size_t>(statusOf(rng))];
        const bool moved = a.finished(id, status, {}, now);
        if (status == "declined") {
          expect(moved == was.has_value() && !after(), "a declined transfer leaves", seed, step);
        } else {
          const Transfer to = status == "done" ? Transfer::Done : Transfer::Failed;
          expect(moved == (!was || transferMayMove(*was, to)), "finish only by an allowed move", seed, step);
          if (moved) expect(after() == to, "finish sets the end state", seed, step);
        }
        break;
      }
      case 7: {
        const bool incoming = before != a.items().end() && before->incoming;
        const std::uint64_t bytes = before == a.items().end() ? 0 : before->bytes;
        a.sent(id, "p1", {{"s.pdf", 800}}, {"/home/u/s.pdf"}, now);
        const auto it = std::ranges::find(a.items(), id, &TransferEntry::id);
        expect(it != a.items().end(), "a sent transfer shows", seed, step);
        expect(after() == (was ? *was : Transfer::Running), "a SendFiles reply never moves a state", seed, step);
        if (it->state != Transfer::Done) expect(it->bytes == bytes, "a SendFiles reply keeps the progress", seed, step);
        expect(incoming ? it->sources.empty() : it->sources.size() == 1 && it->files.size() == 1,
               "only an outgoing transfer takes the names and sources", seed, step);
        break;
      }
      case 8: {
        const bool gone = a.forget(id);
        expect(gone == (was == Transfer::Failed), "forget removes only a failed transfer", seed, step);
        expect(gone ? !after() : after() == was, "forget leaves any other state", seed, step);
        break;
      }
      case 5:
        a.clearFinished();
        expect(a.finishedCount() == 0, "Clear finished keeps only open transfers", seed, step);
        break;
      default: {
        const std::size_t rows = a.rowCount();
        a.clipped({.id = ++clipId, .deviceId = "p1", .text = "clip", .atMs = now});
        const Activity::Row top = a.row(0);
        expect(std::holds_alternative<const ClipEntry*>(top) && std::get<const ClipEntry*>(top)->id == clipId,
               "a new clip is the newest row", seed, step);
        expect(a.rowCount() == rows + 1 || a.rowCount() == rows, "a clip adds one row or replaces the oldest", seed, step);
        break;
      }
      }
      std::set<const void*> seen;
      std::size_t clips = 0;
      for (std::size_t i = 0; i < a.rowCount(); ++i) {
        const auto at = [&](std::size_t k) { return std::visit([](const auto* e) { return e->atMs; }, a.row(k)); };
        std::visit([&](const auto* e) { expect(seen.insert(e).second, "each entry is one row", seed, step); }, a.row(i));
        clips += std::holds_alternative<const ClipEntry*>(a.row(i)) ? 1 : 0;
        if (i > 0) expect(at(i - 1) >= at(i), "rows newest first", seed, step);
      }
      expect(a.rowCount() - clips == a.items().size(), "every transfer is a row", seed, step);
      expect(clips <= Activity::kClips, "at most kClips clips", seed, step);
      std::set<std::string> ids;
      for (std::size_t i = 0; i < a.items().size(); ++i) {
        const TransferEntry& e = a.items()[i];
        expect(ids.insert(e.id).second, "one entry per transfer", seed, step);
        if (i > 0) expect(a.items()[i - 1].atMs >= e.atMs, "newest first", seed, step);
        if (e.state == Transfer::Done) expect(e.bytes == e.total, "a done transfer is complete", seed, step);
      }
    }
  }

  // Phone A and phone B both have a thread 7: a text that failed on A must not show or be retried in B's thread 7.
  void phoneSwitch() {
    using K = ConversationRow::Kind;
    Messages m;
    m.setTexts("A", {{.id = 1, .thread = 7, .address = "+4915112340001", .body = "hi", .atMs = 1}});
    const std::uint64_t serial = m.send(7, "for Maya", 2);
    expect(serial != 0 && m.sendFailed(serial), "a text to A's thread 7 fails", 0, 0);
    m.setTexts("B", {{.id = 1, .thread = 7, .address = "+4917099990000", .body = "yo", .atMs = 1}});
    expect(!m.retry(serial), "a text that failed on phone A cannot be retried through phone B", 0, 0);
    expect(std::ranges::none_of(m.conversation(7, 3), [](const ConversationRow& r) { return r.kind == K::Failed || r.kind == K::Sending; }),
           "phone B's thread 7 shows no text sent on phone A", 0, 0);
  }

  // The user switches phones while phone A's first page is in flight: B gets its own load, A's late page is dropped.
  void photosFollowThePhone() {
    Photos p;
    const auto a = p.startLoad("A");
    const auto b = p.startLoad("B");
    expect(a && b, "a new phone starts its own load while the old one's is in flight", 0, 0);
    expect(!p.loaded(a->serial, 1, {{.path = "/a/1", .name = "a", .atS = 1}}), "phone A's late page is dropped", 0, 0);
    expect(b && p.loaded(b->serial, 1, {{.path = "/b/1", .name = "b", .atS = 1}}) && p.items().size() == 1 &&
               p.items()[0].path == "/b/1",
           "phone B's page fills the grid", 0, 0);
  }

  void dialing() {
    Messages m;
    m.setContacts({{.name = "Maya Okafor", .numbers = {"+49 151 1234-0001"}}, {.name = "Mum", .numbers = {}},
                   {.name = "Mumbai Office", .numbers = {"(022) 555 0100"}}});
    expect(m.numberFor("+49 (151) 2345 6789") == "+4915123456789", "a typed number keeps digits and +", 0, 0);
    expect(m.numberFor("maya") == "+4915112340001", "a name prefix dials the contact's number", 0, 0);
    expect(m.numberFor("Mum") == "0225550100", "a contact without numbers is skipped", 0, 0);
    expect(m.numberFor("Zed").empty(), "an unknown name dials nothing", 0, 0);
    const auto mu = m.matches("mU", 5);
    expect(mu.size() == 1 && mu[0].name == "Mumbai Office", "matches: name prefix, any case, with a number", 0, 0);
    expect(m.matches("", 5).empty() && m.matches("m", 1).size() == 1, "matches: none for no query, at most limit", 0, 0);
    m.setTexts("phone", {{.id = 1, .thread = 7, .address = "+4915112340001", .body = "x", .atMs = 1}});
    expect(m.threadFor("0151 1234 0001") == 7 && !m.threadFor("0170 1"), "threadFor matches the number, not its format", 0, 0);
    expect(duration(41) == "41 s" && duration(768) == "13 min" && duration(3723) == "1 h 2 min" && duration(7190) == "2 h", "durations", 0, 0);
  }

  void days() {
    const std::int64_t now = 1'763'640'000'000; // Thu 2025-11-20 12:00 UTC
    const std::int64_t day = 86'400'000;
    expect(dayLabel(now, now) == "Today", "Today", 0, 0);
    expect(dayLabel(now - day, now) == "Yesterday", "Yesterday", 0, 0);
    expect(dayLabel(now - 3 * day, now) == "Monday", "weekday within a week", 0, 0);
    expect(dayLabel(now - 3 * day, now, true) == "Mon" && dayLabel(now - day, now, true) == "Yesterday" &&
               daysAgo(now - 3 * day, now) == 3,
           "short weekday from its own key; daysAgo counts local days", 0, 0);
    expect(dayLabel(now - 9 * day, now) == "11 Nov", "date this year", 0, 0);
    expect(dayLabel(now - 365 * day, now) == "20 Nov 2024", "date with year", 0, 0);
  }

  // Random photo lists and marks: "N new photos" counts exactly the photos of today after the mark, shows the newest.
  void runNewPhotos(unsigned seed) {
    std::mt19937 rng(seed);
    const std::int64_t now = 1'763'640'000'000; // Thu 2025-11-20 12:00 UTC
    const std::int64_t midnightS = now / 1000 - 12 * 3600;
    for (int step = 0; step < 300; ++step) {
      std::vector<Photo> list;
      for (auto i = static_cast<unsigned>(rng() % 15); i > 0; --i) {
        list.push_back({.path = std::format("/p/{}", list.size()), .atS = midnightS - 7200 + static_cast<std::int64_t>(rng() % 50'000)});
      }
      std::ranges::stable_sort(list, std::greater{}, &Photo::atS);
      const std::int64_t seen = midnightS - 3600 + static_cast<std::int64_t>(rng() % 50'000);
      std::vector<std::string> fresh;
      for (const Photo& photo : list) {
        if (photo.atS > seen && photo.atS >= midnightS) fresh.push_back(photo.path);
      }
      const auto item = newPhotosItem("p1", list, seen, now);
      expect(item.has_value() == !fresh.empty(), "new photos: an item exactly when a photo of today is after the mark", seed, step);
      if (!item) continue;
      const std::string who = fresh.size() == 1 ? "1 new photo" : std::format("{} new photos", fresh.size());
      fresh.resize(std::min(fresh.size(), kStripPhotos));
      expect(item->who == who && item->photos == fresh, "new photos: the count and the newest strip photos", seed, step);
      Feed feed;
      feed.upsert(*item);
      const auto back = decodeSnapshot(encodeSnapshot({}, feed, {}));
      expect(back && back->items.empty(), "new photos stay out of the snapshot", seed, step);
    }
  }

  // Every key the mirror sends down finds its release by keysym alone (no stuck key on the phone), and a chord with
  // Ctrl, Alt or Super never types text.
  void runMirrorKeys() {
    int step = 0;
    for (std::uint32_t sym = 0x20; sym <= 0xFFFF; ++sym, ++step) {
      for (unsigned m = 0; m < 16; ++m) {
        const KeyMods mods{.shift = (m & 1) != 0, .ctrl = (m & 2) != 0, .alt = (m & 4) != 0, .super = (m & 8) != 0};
        const std::uint32_t utf32 = sym < 0x7F ? sym : 0;
        const MirrorKey key = mirrorKeyFor(sym, utf32, mods);
        if (key.kind == MirrorKey::Kind::Code) expect(androidKeyCode(sym) == key.code, "a release finds its key", 0, step);
        if (mods.ctrl || mods.alt || mods.super) expect(key.kind != MirrorKey::Kind::Text, "a chord types no text", 0, step);
        if (key.kind == MirrorKey::Kind::Text) expect(!key.text.empty(), "text keys carry text", 0, step);
      }
    }
    expect(mirrorKeyFor(0x41, 0x41, {.shift = true}).text == "A", "Shift+a types A", 0, 0);
    expect(mirrorKeyFor(0x61, 0x61, {.ctrl = true}).code == androidKeyCode(0x41), "Ctrl+a and a Shift release match", 0, 0);
    expect(mirrorKeyFor(0, 0xE9, {}).text == "\xC3\xA9", "e-acute is UTF-8", 0, 0);
  }

  void runPair(unsigned seed) {
    using K = Pair::Kind;
    using E = PairEvent::Kind;
    const std::set<std::pair<K, K>> edges{{K::Idle, K::Starting},  {K::Failed, K::Starting}, {K::Starting, K::Showing},
                                          {K::Starting, K::Failed}, {K::Showing, K::Failed},  {K::Showing, K::Idle},
                                          {K::Starting, K::Idle}, {K::Failed, K::Idle}};
    std::mt19937 rng(seed);
    Pair pair;
    for (int step = 0; step < 2000; ++step) {
      PairEvent event{.kind = static_cast<E>(rng() % 6)};
      if (event.kind == E::Started) event = {.kind = E::Started, .code = rng() % 4 ? std::to_string(rng() % 1000000) : "", .uri = "andromeda-link://pair"};
      if (event.kind == E::Failed) event = {.kind = E::Failed, .error = "No network"};
      const Pair before = pair;
      const bool moved = pairStep(pair, event);
      if (moved) {
        expect(edges.contains({before.kind, pair.kind}), "only allowed moves", seed, step);
      } else {
        expect(pair.kind == before.kind && pair.code == before.code && pair.error == before.error,
               "a rejected event changes nothing", seed, step);
      }
      expect((pair.kind == K::Showing) == !pair.code.empty(), "a code shows exactly while Showing", seed, step);
      expect(pair.kind == K::Failed || pair.error.empty(), "an error only while Failed", seed, step);
      expect(pair.kind == K::Showing || pair.uri.empty(), "a URI only while Showing", seed, step);
    }
  }

  // Random loads, failures, selections and save batches against the Photos rules.
  void runPhotos(unsigned seed) {
    using L = Photos::Load;
    using M = Photos::Media;
    std::mt19937 rng(seed);
    const std::int64_t now = 1'763'640'000'000;
    // The phone's media store, newest first; every third item a video.
    std::vector<Photo> phone;
    for (int i = 0; i < 150; ++i) {
      phone.push_back({.path = std::format("/p/{}", i), .name = "x", .atS = now / 1000 - i * 4000,
                       .videoS = i % 3 == 0 ? std::optional<std::uint32_t>(i) : std::nullopt});
    }
    const auto of = [&](M media) {
      std::vector<Photo> out;
      for (const Photo& p : phone) {
        if (media == M::All || (media == M::Videos) == p.videoS.has_value()) out.push_back(p);
      }
      return out;
    };
    Photos photos;
    std::vector<Photos::Ask> asks; // in flight, answered in any order
    std::size_t batch = 0;
    for (int step = 0; step < 4000; ++step) {
      switch (rng() % 10) {
      case 0:
        if (const auto ask = photos.startLoad("phone")) asks.push_back(*ask);
        break;
      case 1:
        if (const auto ask = photos.startMore()) asks.push_back(*ask);
        break;
      case 2:
      case 3: {
        if (asks.empty()) break;
        const std::size_t at = rng() % asks.size();
        const Photos::Ask ask = asks[at];
        asks.erase(asks.begin() + static_cast<std::ptrdiff_t>(at));
        const std::vector<Photo> all = of(ask.media);
        std::vector<Photo> page;
        for (std::size_t i = ask.offset; i < all.size() && i < ask.offset + Photos::kCount; ++i) page.push_back(all[i]);
        std::ranges::shuffle(page, rng);
        const auto before = photos.items().size();
        const bool stale = rng() % 4 == 0;
        const bool took = photos.loaded(stale ? ask.serial + 1000 : ask.serial, static_cast<std::uint32_t>(all.size()), std::move(page));
        if (stale) expect(!took && photos.items().size() == before, "an answer to another ask changes nothing", seed, step);
        break;
      }
      case 4: {
        if (asks.empty()) break;
        const Photos::Ask ask = asks.front();
        asks.erase(asks.begin());
        const L before = photos.load();
        const bool took = photos.failed(ask.serial, static_cast<L>(4 + rng() % 3));
        if (took && before == L::More) expect(photos.load() == L::Ready, "a failed next page keeps the photos", seed, step);
        break;
      }
      case 5: (void)photos.setMedia(static_cast<M>(rng() % 3)); break;
      case 6: photos.toggle(rng() % 160); break;
      case 7: photos.clearSelection(); break;
      case 8: {
        const bool saving = photos.save() == Photos::Save::Saving;
        const std::size_t want = photos.selectedCount();
        const auto paths = photos.startSave();
        expect(paths.size() == (saving ? 0 : want), "a save takes the whole selection unless one runs", seed, step);
        if (!paths.empty()) batch = paths.size();
        break;
      }
      case 9: photos.saveAnswered(rng() % 2); break;
      }
      const auto& items = photos.items();
      const std::vector<Photo> want = of(photos.media());
      bool prefix = items.size() <= want.size();
      std::size_t picked = 0;
      for (std::size_t i = 0; i < items.size(); ++i) {
        picked += photos.selected(i) ? 1 : 0;
        if (i > 0) expect(items[i - 1].atS >= items[i].atS, "newest first", seed, step);
        prefix = prefix && items[i].path == want[i].path && items[i].videoS == want[i].videoS;
      }
      expect(prefix, "the photos are the newest items of the shown media, without gaps or repeats", seed, step);
      expect(picked == photos.selectedCount(), "the selection is a subset of the photos", seed, step);
      const L load = photos.load();
      if (load != L::Ready && load != L::Loading && load != L::More) {
        expect(items.empty() && picked == 0, "no photos unless loaded", seed, step);
      }
      expect(items.size() <= photos.total() || items.empty(), "never more photos than the phone has", seed, step);
      const std::size_t answered = photos.saved() + photos.saveFailed();
      if (photos.save() == Photos::Save::Saving) expect(answered < batch, "saving until every photo answered", seed, step);
      if (photos.save() == Photos::Save::Done) expect(answered == batch, "done when every photo answered", seed, step);
      const auto days = photos.days(now);
      for (std::size_t d = 0; d < days.size(); ++d) {
        const std::size_t end = d + 1 < days.size() ? days[d + 1].first : items.size();
        expect(days[d].first < end, "each day has a photo", seed, step);
        for (std::size_t i = days[d].first; i < end; ++i) {
          expect(dayLabel(items[i].atS * 1000, now) == days[d].second, "a photo sits under its own day", seed, step);
        }
      }
      expect(items.empty() == days.empty() && (days.empty() || days.front().first == 0), "days start at the first photo", seed, step);
    }
  }

  // Random names and queries (mixed case, UTF-8): a hit holds the query, a miss does not, hits keep model order.
  void runSearch(unsigned seed) {
    std::mt19937 rng(seed);
    const std::array<std::string, 6> parts{"ma", "MA", "Ya", "ö", "Ö", " "};
    std::uniform_int_distribution<int> part(0, 5), len(0, 4), qlen(1, 2), count(0, 12);
    const auto word = [&](int n) { std::string w; for (int i = 0; i < n; ++i) w += parts[static_cast<std::size_t>(part(rng))]; return w; };
    const auto lower = [](std::string w) { for (char& c : w) if (static_cast<unsigned char>(c) < 0x80) c = static_cast<char>(std::tolower(c)); return w; };
    for (int step = 1; step <= 50; ++step) {
      std::vector<Thread> threads(static_cast<std::size_t>(count(rng)));
      for (Thread& t : threads) { t.name = word(len(rng)); t.last = word(len(rng)); }
      std::vector<CallEntry> calls(static_cast<std::size_t>(count(rng)));
      for (CallEntry& c : calls) { c.name = word(len(rng)); c.number = word(len(rng)); }
      std::vector<FileEntry> files(static_cast<std::size_t>(count(rng)));
      for (FileEntry& f : files) f.name = word(len(rng));
      std::vector<Photo> photos(static_cast<std::size_t>(count(rng)));
      for (Photo& ph : photos) ph.name = word(len(rng));
      const std::string q = word(qlen(rng));
      const SearchHits hits = searchHits(q, threads, calls, files, photos);
      const auto check = [&](const std::vector<std::size_t>& got, std::size_t n, auto holds) {
        std::size_t k = 0;
        for (std::size_t i = 0; i < n; ++i) {
          const bool hit = k < got.size() && got[k] == i;
          expect(hit == holds(i), "a row shows exactly when it holds the query", seed, step);
          k += hit ? 1 : 0;
        }
        expect(k == got.size(), "hits in model order, each once", seed, step);
      };
      const auto in = [&](const std::string& text) { return lower(text).find(lower(q)) != std::string::npos; };
      check(hits.threads, threads.size(), [&](std::size_t i) { return in(threads[i].name) || in(threads[i].last); });
      check(hits.calls, calls.size(), [&](std::size_t i) { return in(calls[i].name) || in(calls[i].number); });
      check(hits.files, files.size(), [&](std::size_t i) { return in(files[i].name); });
      check(hits.photos, photos.size(), [&](std::size_t i) { return in(photos[i].name); });
    }
  }

  // Random navigation with replies in any order, and save events, against the Files rules.
  void runFiles(unsigned seed) {
    using L = Files::Load;
    using S = FileSave::State;
    std::mt19937 rng(seed);
    Files files;
    std::vector<std::pair<std::uint64_t, std::string>> pending; // serial, path asked
    std::uint64_t newest = 0;
    std::string newestPath = "/";
    const std::array<std::string, 4> names{"DCIM", "b.pdf", "Alpha", "a.txt"};
    std::map<std::string, S> saves;
    for (int step = 0; step < 3000; ++step) {
      switch (rng() % 7) {
      case 0: newestPath = files.childPath(names[rng() % 4]); newest = files.go(newestPath); pending.emplace_back(newest, newestPath); break;
      case 1: newestPath = files.parent(); newest = files.go(newestPath); pending.emplace_back(newest, newestPath); break;
      case 2: {
        newestPath = files.crumbPath(rng() % 4);
        newest = files.go(newestPath);
        pending.emplace_back(newest, newestPath);
        break;
      }
      case 3:
      case 4: {
        if (pending.empty()) break;
        const std::size_t i = rng() % pending.size();
        const auto [serial, path] = pending[i];
        pending.erase(pending.begin() + static_cast<std::ptrdiff_t>(i));
        std::vector<FileEntry> list;
        for (const auto& n : names) {
          if (rng() % 2) list.push_back({.name = n, .folder = !n.contains('.')});
        }
        const bool ok = rng() % 4 ? files.listed(serial, list) : files.failed(serial, "busy");
        expect(ok == (serial == newest && path == files.path()), "only the newest reply lands", seed, step);
        break;
      }
      case 5: {
        const std::string path = "/x/" + names[rng() % 4];
        const auto before = saves.contains(path) ? std::optional(saves[path]) : std::nullopt;
        const bool started = files.saveStarted(path, 10);
        expect(started == (!before || *before != S::Running), "one save per file at a time", seed, step);
        if (started) saves[path] = S::Running;
        break;
      }
      case 6: {
        const std::string path = "/x/" + names[rng() % 4];
        const bool running = saves.contains(path) && saves[path] == S::Running;
        const bool done = rng() % 2;
        const bool moved = done ? files.saveDone(path, "/d/" + path) : files.saveFailed(path);
        expect(moved == running, "a save ends only while it runs", seed, step);
        if (moved) saves[path] = done ? S::Done : S::Failed;
        break;
      }
      }
      expect(files.path() == newestPath, "the path is the newest asked", seed, step);
      expect(files.crumbs().size() == static_cast<std::size_t>(std::ranges::count(files.path(), '/')) + (files.path() == "/" ? 0 : 1),
             "one crumb per folder", seed, step);
      expect(files.crumbPath(files.crumbs().size() - 1) == files.path(), "the last crumb is the path", seed, step);
      const auto& entries = files.entries();
      for (std::size_t i = 1; i < entries.size(); ++i) {
        expect(entries[i - 1].folder >= entries[i].folder, "folders first", seed, step);
      }
      if (files.load() == L::Failed) expect(entries.empty() && !files.error().empty(), "a failed folder lists nothing", seed, step);
      for (const auto& [path, state] : saves) expect(files.save(path) && files.save(path)->state == state, "save states", seed, step);
    }
  }

  void runMirror(unsigned seed) {
    using K = Mirror::Kind;
    using E = MirrorEvent::Kind;
    const std::set<std::pair<K, K>> edges{{K::Idle, K::Asking},    {K::Ended, K::Asking},    {K::Failed, K::Asking},
                                          {K::Asking, K::Showing}, {K::Showing, K::Ended},   {K::Asking, K::Failed},
                                          {K::Showing, K::Failed}, {K::Asking, K::Idle},     {K::Showing, K::Idle}};
    std::mt19937 rng(seed);
    Mirror mirror;
    for (int step = 0; step < 3000; ++step) {
      // Events of the current session, or of an older one (a late reply).
      const std::uint64_t session = rng() % 3 ? mirror.session : mirror.session - std::min<std::uint64_t>(mirror.session, 1 + rng() % 2);
      const std::uint32_t width = rng() % 4 ? 1080 : 0;
      const MirrorEvent event{.kind = static_cast<E>(rng() % 5), .session = session, .width = width, .height = 2400, .error = "denied"};
      const Mirror before = mirror;
      const bool moved = mirrorStep(mirror, event);
      if (moved) {
        expect(edges.contains({before.kind, mirror.kind}), "only allowed moves", seed, step);
        const bool sessionFree = event.kind == E::Start || event.kind == E::Stop;
        expect(sessionFree || session == before.session, "an older session's event never lands", seed, step);
        expect(mirror.session == before.session + (event.kind == E::Start ? 1 : 0), "Start alone opens a session", seed, step);
      } else {
        expect(mirror.kind == before.kind && mirror.session == before.session, "a rejected event changes nothing", seed, step);
      }
      if (event.kind == E::Opened && session == before.session && before.kind == K::Asking) {
        expect(mirror.kind != K::Asking, "an open reply always leaves Asking", seed, step);
      }
      expect((mirror.kind == K::Showing) == (mirror.width > 0), "a size exactly while Showing", seed, step);
      expect(mirror.kind == K::Failed || mirror.error.empty(), "an error only while Failed", seed, step);
    }
  }

  void lastSeen() {
    const std::int64_t now = 1'763'640'000; // 2025-11-20 12:00 UTC
    expect(lastSeenText(0, now) == "Not connected", "never seen", 0, 0);
    expect(lastSeenText(now - 20, now) == "Last seen just now", "just now", 0, 0);
    expect(lastSeenText(now - 12 * 60, now) == "Last seen 12 min ago", "minutes", 0, 0);
    expect(lastSeenText(now - 3 * 3600, now) == "Last seen 3 h ago", "hours", 0, 0);
    expect(lastSeenText(now - 86400, now) == "Last seen yesterday", "yesterday", 0, 0);
    expect(lastSeenText(now - 9 * 86400, now) == "Last seen 11 Nov", "date", 0, 0);
  }

  // Random edit sessions against a server that may answer late, out of order or with an error.
  void runCommands(unsigned seed) {
    std::mt19937 rng(seed);
    const auto pick = [&](int n) { return static_cast<int>(rng() % static_cast<unsigned>(n)); };
    const std::array<std::string, 5> words{"", " ", "Lock", "loginctl lock-session", std::string(600, 'n')};
    const auto valid = [](const std::vector<Command>& list) {
      return list.size() <= CommandsEditor::kMax && std::ranges::all_of(list, [](const Command& c) {
               return c.name.find_first_not_of(" \t\r\n") != std::string::npos && c.name.size() <= CommandsEditor::kMaxName &&
                      c.line.find_first_not_of(" \t\r\n") != std::string::npos;
             });
    };
    CommandsEditor ed;
    std::vector<std::uint64_t> inflight;
    for (int step = 0; step < 4000; ++step) {
      const CommandsEditor before = ed;
      switch (pick(12)) {
      case 0: {
        std::vector<Command> server;
        for (int k = pick(3) == 0 ? 64 : pick(6); k > 0; --k) server.push_back({std::format("S{}", k), "true"});
        ed.listed(server);
        if (before.mode() == CommandsEditor::Mode::Editing && before.index() < before.list().size() &&
            before.sync() == CommandsEditor::Sync::Synced) {
          const Command& was = before.list()[before.index()];
          if (std::ranges::count(server, was) == 1 && std::ranges::count(before.list(), was) == 1)
            expect(ed.index() < ed.list().size() && ed.list()[ed.index()] == was, "the open row follows its command", seed, step);
        }
        if (before.sync() != CommandsEditor::Sync::Synced) expect(ed.list() == before.list(), "local list wins while unsynced", seed, step);
        if (before.mode() == CommandsEditor::Mode::Editing) expect(ed.draft() == before.draft(), "server update keeps the draft", seed, step);
        break;
      }
      case 1: ed.open(static_cast<std::size_t>(pick(70))); break;
      case 2: {
        const bool ok = ed.add();
        if (before.mode() != CommandsEditor::Mode::Editing && !before.canAdd())
          expect(!ok && ed.list() == before.list() && ed.mode() == before.mode(), "no add at the cap", seed, step);
        for (int i = 0; i < 70 && ed.add(); ++i) ed.editName("N"), ed.editLine("x");
        break;
      }
      case 3:
        if (ed.editName(words[static_cast<std::size_t>(pick(5))]))
          expect(ed.draft().line == before.draft().line && ed.list() == before.list(), "typing changes only the draft", seed, step);
        break;
      case 4: ed.editLine(words[static_cast<std::size_t>(pick(4))]); break;
      case 5:
        ed.close();
        if (before.mode() == CommandsEditor::Mode::Editing && ed.mode() == CommandsEditor::Mode::Editing)
          expect(ed.draft() == before.draft() && ed.invalid() != CommandsEditor::Invalid::None, "a refused commit keeps the text and says why", seed, step);
        break;
      case 6: {
        const bool removed = ed.remove() && ed.mode() == CommandsEditor::Mode::Removed;
        if (removed && ed.undo()) expect(ed.list() == before.list(), "undo restores the list", seed, step);
        break;
      }
      case 7:
        if (pick(2) == 0) {
          ed.undo();
          break;
        }
        {
          const std::size_t from = static_cast<std::size_t>(pick(8)), to = static_cast<std::size_t>(pick(8));
          if (ed.move(from, to) && before.mode() != CommandsEditor::Mode::Editing) {
            std::vector<Command> expected = before.list();
            const Command moved = expected[from];
            expected.erase(expected.begin() + static_cast<std::ptrdiff_t>(from));
            expected.insert(expected.begin() + static_cast<std::ptrdiff_t>(to), moved);
            expect(ed.list() == expected, "a move lands the row at its target, the others keep their order", seed, step);
          }
        }
        break;
      case 8:
        if (auto send = ed.takeSend()) {
          expect(valid(send->second) && send->second == ed.list(), "only a valid list is sent", seed, step);
          inflight.push_back(send->first);
        }
        break;
      case 9:
      case 10:
        if (!inflight.empty()) {
          const std::size_t i = static_cast<std::size_t>(pick(static_cast<int>(inflight.size())));
          const std::uint64_t serial = inflight[i];
          inflight.erase(inflight.begin() + static_cast<std::ptrdiff_t>(i));
          const bool latest = std::ranges::none_of(inflight, [&](std::uint64_t s) { return s > serial; });
          const bool changed = ed.sent(serial, pick(3) == 0 ? std::optional<std::string>("Link is not running") : std::nullopt);
          if (!latest) expect(!changed, "an older reply changes nothing", seed, step);
          if (changed) expect(ed.list() == before.list(), "a reply keeps the list", seed, step);
        }
        break;
      default: ed.retry(); break;
      }
      expect(valid(ed.list()), "the list holds only complete commands, at most 64", seed, step);
      if (ed.mode() == CommandsEditor::Mode::Editing) expect(ed.index() <= ed.list().size(), "the open row exists", seed, step);
    }
  }

  // Random snapshots and clock steps: the position stays in [0, length], holds while paused, never goes back while playing.
  void runPlayer(unsigned seed) {
    std::mt19937 rng(seed);
    std::uniform_int_distribution<std::int64_t> ms(0, 400000);
    for (int step = 1; step <= 50; ++step) {
      const PhonePlayer p{.playing = ms(rng) % 2 == 0, .positionMs = ms(rng), .lengthMs = ms(rng) % 3 == 0 ? 0 : ms(rng) + 1,
                          .atMs = ms(rng)};
      std::int64_t now = ms(rng), last = -1;
      for (int tick = 0; tick < 8; ++tick, now += ms(rng) / 50) {
        const std::int64_t at = p.position(now);
        expect(at >= 0 && (p.lengthMs == 0 || at <= p.lengthMs), "position within the length", seed, step);
        if (!p.playing) expect(at == std::min(p.positionMs, p.lengthMs > 0 ? p.lengthMs : p.positionMs), "paused holds", seed, step);
        expect(at >= last, "playing never goes back", seed, step);
        if (p.playing && p.lengthMs == 0 && now > p.atMs) expect(at == p.positionMs + now - p.atMs, "playing advances with the clock", seed, step);
        last = at;
      }
    }
  }

} // namespace

int main() {
  i18n::Service::instance().addDomain("link", ANDROMEDA_TRANSLATIONS_DIR);
  i18n::Service::instance().setLanguage("en");
  for (unsigned seed = 1; seed <= 20; ++seed) runPlayer(seed);
  runMirrorKeys();
  for (unsigned seed = 1; seed <= 20; ++seed) runPair(seed);
  lastSeen();
  for (unsigned seed = 1; seed <= 20; ++seed) runPhotos(seed);
  for (unsigned seed = 1; seed <= 20; ++seed) runFiles(seed);
  for (unsigned seed = 1; seed <= 20; ++seed) runSearch(seed);
  for (unsigned seed = 1; seed <= 20; ++seed) runMirror(seed);
  setenv("TZ", "UTC", 1);
  tzset();
  for (unsigned seed = 1; seed <= 20; ++seed) runFeed(seed);
  for (unsigned seed = 1; seed <= 20; ++seed) runMessages(seed);
  presence();
  days();
  for (unsigned seed = 1; seed <= 20; ++seed) runNewPhotos(seed);
  dialing();
  phoneSwitch();
  photosFollowThePhone();
  for (unsigned seed = 1; seed <= 20; ++seed) runActivity(seed);
  for (unsigned seed = 1; seed <= 20; ++seed) runCommands(seed);
  if (failures == 0) std::puts("link_model: ok");
  return failures == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
