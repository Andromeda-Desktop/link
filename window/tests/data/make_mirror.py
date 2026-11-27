#!/usr/bin/env python3
# Writes mirror-h264.bin and mirror-hevc.bin: 10 frames of 320x240 test video in Link1's mirror framing (u32 length,
# u64 time in us, u8 flags, then one Annex B access unit), as a phone sends them. Needs ffmpeg with libx264/libx265.
import os, struct, subprocess

here = os.path.dirname(os.path.abspath(__file__))
src = ["-f", "lavfi", "-i", "testsrc=size=320x240:rate=10", "-frames:v", "10", "-pix_fmt", "yuv420p"]
codecs = {
    "h264": (["-c:v", "libx264", "-bf", "0", "-x264-params", "slices=1"], lambda b: b[0] & 0x1F, range(1, 6)),
    "hevc": (["-c:v", "libx265", "-x265-params", "bframes=0:log-level=error"], lambda b: (b[0] >> 1) & 0x3F,
             range(0, 32)),
}
for name, (args, nal_type, vcl) in codecs.items():
    raw = subprocess.run(["ffmpeg", "-v", "error", *src, *args, "-f", name, "-"], check=True, capture_output=True).stdout
    starts = [i for i in range(len(raw) - 3) if raw[i:i + 3] == b"\0\0\1"]
    nals = [raw[s:(e - 1 if raw[e - 1] == 0 else e) if e else len(raw)] for s, e in zip(starts, starts[1:] + [0])]
    units, unit = [], b""
    for nal in nals:  # one access unit: the non-VCL units before a frame's single slice, then the slice
        unit += b"\0\0\0\1" + nal[3:]
        if nal_type(nal[3:]) in vcl:
            units.append(unit)
            unit = b""
    with open(os.path.join(here, f"mirror-{name}.bin"), "wb") as out:
        for i, au in enumerate(units):
            out.write(struct.pack(">IQB", len(au), i * 100_000, 0) + au)
    print(name, len(units), "units")
