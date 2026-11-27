#include "link_bus.h"

#include <cstdint>
#include <map>
#include <print>
#include <string>
#include <vector>

using linkapp::bus::Reader;
using linkapp::bus::Struct;
using linkapp::bus::Variant;
using linkapp::bus::Writer;

namespace {

  int failures = 0;

  void expect(bool ok, const char* what) {
    if (!ok) {
      std::println(stderr, "link_bus_test: {}", what);
      ++failures;
    }
  }

  using Props = std::map<std::string, Variant>;

} // namespace

// The codec against byte layouts worked out by hand from the D-Bus specification (little endian), where a variant
// value lands at another offset modulo 8 than the one it was made at, so the copy must re-pad it.
int main() {
  {
    // a{sv} {"k": <uint32 7>}: length 16, pad to 8, "k", signature "u" at 17, pad to 20, 7.
    Writer writer;
    writer.put(Props{{"k", Variant(std::uint32_t{7})}});
    const std::vector<std::uint8_t> want{16, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 'k', 0, 1, 'u', 0, 0, 0, 0, 7, 0, 0, 0};
    expect(writer.bytes() == want, "a{sv} with a uint32 is not the spec's layout");

    Reader reader(want.data(), want.size());
    Props props;
    reader.get(props);
    expect(reader.ok() && reader.atEnd(), "a{sv} does not read to its end");
    expect(props["k"].containsValueOfType<std::uint32_t>() && props["k"].get<std::uint32_t>() == 7, "a{sv} reads back wrong");
  }
  {
    // (yv) (1, <(t) (2)>): byte 1, signature "(t)" at 1, struct pad to 8, t 2. Made at offset 0, written at 5.
    Writer writer;
    writer.put(Struct<std::uint8_t, Variant>{std::uint8_t{1}, Variant(Struct<std::uint64_t>{std::uint64_t{2}})});
    const std::vector<std::uint8_t> want{1, 3, '(', 't', ')', 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0};
    expect(writer.bytes() == want, "a struct variant is not re-padded");

    Reader reader(want.data(), want.size());
    Struct<std::uint8_t, Variant> read;
    reader.get(read);
    const auto inner = read.get<1>().get<Struct<std::uint64_t>>();
    expect(reader.ok() && inner.get<0>() == 2, "a struct variant reads back wrong");

    // Written alone, the read variant's value lands at offset 5: the copy pads it to 8 again.
    Writer again;
    again.put(read.get<1>());
    const std::vector<std::uint8_t> wantAgain{3, '(', 't', ')', 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0};
    expect(again.bytes() == wantAgain, "a read variant is not re-padded for its new offset");
  }
  {
    // as [] then s "x": an empty array still pads to its element alignment; a truncated body fails, never overreads.
    Writer writer;
    writer.put(std::vector<std::string>{});
    writer.put(std::string("x"));
    const std::vector<std::uint8_t> want{0, 0, 0, 0, 1, 0, 0, 0, 'x', 0};
    expect(writer.bytes() == want, "an empty array is not the spec's layout");

    Reader truncated(want.data(), want.size() - 3);
    std::vector<std::string> list;
    std::string text;
    truncated.get(list);
    truncated.get(text);
    expect(!truncated.ok(), "a truncated body reads as valid");
  }
  {
    // a{sa{sv}}: nested dicts each pad their entries to 8 from the body start.
    const std::map<std::string, Props> info{{"p1", Props{{"name", Variant(std::string("Pixel"))}}}, {"p2", Props{}}};
    Writer writer;
    writer.put(info);
    Reader reader(writer.bytes().data(), writer.bytes().size());
    std::map<std::string, Props> read;
    reader.get(read);
    expect(reader.ok() && reader.atEnd() && read.size() == 2 && read["p2"].empty()
               && read["p1"]["name"].get<std::string>() == "Pixel",
        "a{sa{sv}} does not round-trip");
  }
  return failures == 0 ? 0 : 1;
}
