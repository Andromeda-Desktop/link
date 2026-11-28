#!/usr/bin/env python3
"""A stand-in org.andromeda.Link1 on the session bus for headless shots of the Link app.

It serves the real interface XML with the mockup's sample data (one connected phone, a missed call), then emits a
text, a file offer and notifications once a client calls GetAll. Usage: fake_link1.py <Link1.xml> [sample|away|unpaired]
"""
import itertools
import os
import sys
import socket
import struct
import threading
import time
import zlib

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

XML, MODE = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "sample")
PATH, IFACE = "/org/andromeda/Link1", "org.andromeda.Link1"
now_ms = int(time.time() * 1000)
today = time.localtime()


def at(hour, minute):
    return int(time.mktime((today.tm_year, today.tm_mon, today.tm_mday, hour, minute, 0, 0, 0, -1)) * 1000)


def ago(days, hour, minute):
    return min(at(hour, minute) - days * 86_400_000, now_ms - 60_000)


MAYA, DAD, PARCEL, JONAS, LENA = "+4915112340001", "+491701234567", "+49 151 2345 6789", "+4915790001111", "+4917622223333"
MMS = 1 << 62
# (id, thread, address, body, date ms, status, image parts): the mockup's threads; ids 7, 8 and 5 arrive as
# TextReceived (unread). Status 0 received, 3 delivered; MMS ids have bit 62.
texts = [
    (1, 1, MAYA, "Dinner Friday? The new place on Oranienstraße", ago(1, 19, 2), 0, []),
    (2, 1, MAYA, "Yes! 19:00 works", ago(1, 19, 5), 3, []),
    (MMS + 1, 1, MAYA, "", ago(0, 8, 0), 0, [(901, "image/jpeg")]),
    (7, 1, MAYA, "Leaving the office now", ago(0, 18, 30), 0, []),
    (8, 1, MAYA, "Running 10 min late, order me the pasta?", ago(0, 18, 42), 0, []),
    (5, 2, DAD, "Call me when you're free", ago(0, 17, 5), 0, []),
    (6, 3, PARCEL, "Ihr Paket wird heute zwischen 14:00 und 16:00 Uhr zugestellt. Sendungsverfolgung: "
                   "https://example.com/a/very/long/path/that/never/breaks/0123456789abcdef", ago(0, 12, 10), 0, []),
    (4, 4, JONAS, "Sounds good, see you Friday", ago(2, 16, 0), 0, []),
    (MMS + 2, 5, LENA, "", ago(3, 9, 30), 0, [(902, "image/png")]),
]
contacts = [("Maya Okafor", [MAYA]), ("Dad", ["0170 1234567"]), ("Jonas Weber", [JONAS]), ("Lena", [LENA])]
connected = MODE in ("sample", "two")
# CMDS=empty | worst (64, with long, one-letter and non-Latin names); otherwise three.
COMMANDS = {"empty": [], "worst": [
    ("Sync the photo library to the NAS and then shut the computer down",
     "rsync -aH --delete --info=progress2 /home/user/Pictures/ nas.local:/volume1/backup/user-laptop/Pictures/&&systemctl poweroff"),
    ("X", "xset dpms force off"), ("Bildschirm sperren und Musik anhalten", "playerctl pause; loginctl lock-session"),
    ("画面をロック", "loginctl lock-session"), ("Play focus playlist", "mpv --no-video --shuffle ~/Music/Focus/")]
    + [(f"Scene {i}: lights {'on' if i % 2 else 'off'}", f"curl -s -X POST http://hue.local/api/scene/{i}") for i in range(1, 60)],
}.get(os.environ.get("CMDS", ""), [("Lock", "loginctl lock-session"), ("Suspend", "systemctl suspend"),
                                     ("Backup", "rsync -a ~/Documents /mnt/backup")])
props = {
    "Devices": GLib.Variant("a(ssb)", [] if MODE == "unpaired" else [("p1", "Pixel 8", connected)] + ([("p2", "Galaxy A54", False)] if MODE == "two" else [])),
    "DeviceInfo": GLib.Variant("a{sa{sv}}", {} if MODE == "unpaired" else {"p1": {
        "transport": GLib.Variant("s", "wifi" if connected else ""),
        "connected": GLib.Variant("b", connected),
        "last_seen": GLib.Variant("x", now_ms // 1000 - 12 * 60),
        "battery": GLib.Variant("i", 74)} | ({  # KEYS=1: every phone card key, the webcam on.
            "mic": GLib.Variant("s", "ready"), "sound": GLib.Variant("s", "ready"),
            "camera": GLib.Variant("s", "on")} if os.environ.get("KEYS") else {})} | ({"p2": {
        "connected": GLib.Variant("b", False),
        "last_seen": GLib.Variant("x", now_ms // 1000 - 3 * 3600)}} if MODE == "two" else {})),
    "DeviceStatus": GLib.Variant("a{s(ubsi)}", {"p1": (74, False, "wifi", -1)} if connected else {}),
    "DeviceControls": GLib.Variant("a{s(isii)}", {"p1": (50, "normal", 0, 0)} if connected else {}),
    "DeviceToggles": GLib.Variant("a{s(iii)}", {"p1": (1, 1, 0)} if os.environ.get("KEYS") else {}),
    "Paused": GLib.Variant("as", []),
    # ART=path gives the player a cover.
    "PhonePlayers": GLib.Variant("a{s(sssssbttixas)}", {"p1": (
        "spotify", "Spotify", "Holocene", "Bon Iver", "file://" + os.environ["ART"] if os.environ.get("ART") else "", True,
        161_000, 336_000, -1, now_ms, ["play-pause", "next", "previous", "seek"])} if MODE == "sample" else {}),
    "Pairing": GLib.Variant("b", False),
    "Grants": GLib.Variant("a{sas}", {"p1": ["notifications", "clipboard", "screen", "files", "messages", "calls"]
                                         + ([] if os.environ.get("NOGRANT") else ["commands"])}),
    "AutoAccept": GLib.Variant("as", []),
    "NotificationsWhileLocked": GLib.Variant("b", False),
    "Commands": GLib.Variant("a(ss)", COMMANDS),
    "Texts": GLib.Variant("a{sa(ttsstya(ts))}", {"p1": texts} if connected else {}),
    "Contacts": GLib.Variant("a{sa(sas)}", {"p1": contacts}),
    "Calls": GLib.Variant("a{sa(sstus)}", {"p1": [
        ("+14155550199", "Mum", ago(0, 18, 20), 0, "missed"),
        (MAYA, "", ago(0, 12, 4), 192, "outgoing"),
        (DAD, "Dad", ago(1, 19, 40), 768, "incoming"),
        (PARCEL, "", ago(2, 10, 2), 41, "incoming"),
    ]} if connected else {}),
}

node = Gio.DBusNodeInfo.new_for_xml(open(XML).read())
iface = node.lookup_interface(IFACE)
conn = Gio.bus_get_sync(Gio.BusType.SESSION)


def emit(name, signature, *args):
    conn.emit_signal(None, PATH, IFACE, name, GLib.Variant(signature, args))


def emit_sample():
    if not connected:
        return False
    for (id_, _thread, address, body, _at, _status, _parts) in texts:
        if id_ in (5, 7, 8):
            emit("TextReceived", "(stss)", "p1", id_, address, body)
    if os.environ.get("CALL"):  # ringing or active
        emit("Call", "(sssss)", "p1", os.environ["CALL"], "+44 7700 900461", "Mum", "")
    if os.environ.get("HOTSPOT"):
        emit("HotspotRequested", "(ss)", "p1", "DIRECT-Link-4F2A")
    page = os.environ.get("PAGE", "waiting")
    if page == "activity":
        emit_activity()
    if page != "waiting":
        return False  # no shell popups over another page's caption
    emit("TransferOffered", "(ssa(st))", "t1", "p1", [("Quarterly report Q3 final (2).pdf", 2_400_000)])
    emit("NotificationPosted", "(sssssaya(ssb)bas)", "p1", "n1", "Slack", "#release", "v2026.11.6 is out", b"",
         [("reply", "Reply", True)], False, ["Nice!", "On it"])
    emit("NotificationPosted", "(sssssaya(ssb)bas)", "p1", "n2", "Calendar", "Dentist", "Tomorrow 09:30", b"", [], False, [])
    return False


CLIPS = {1: "Meet at gate B12 at 18:40, boarding closes 18:55\nSeat 14C", 2: "hunter2"}


def emit_activity():
    home = os.path.expanduser("~")
    emit("TransferOffered", "(ssa(st))", "t3", "p1", [("IMG_20261120_181203.jpg", 3_400_000)])
    emit("TransferProgress", "(stt)", "t3", 3_400_000, 3_400_000)
    saved = os.environ.get("RECEIVED_DIR", home + "/Pictures")  # a test points it at a real file
    emit("TransferFinished", "(ssas)", "t3", "done", [saved + "/IMG_20261120_181203.jpg"])
    emit("TransferProgress", "(stt)", "t2", 14_000_000, 14_000_000)
    emit("TransferFinished", "(ssas)", "t2", "done", [])
    emit("TransferProgress", "(stt)", "t5", 0, 2_000_000)
    emit("TransferFinished", "(ssas)", "t5", "failed", [])
    emit("TransferProgress", "(stt)", "t4", 10_000_000, 160_000_000)
    GLib.timeout_add(500, lambda: emit("TransferProgress", "(stt)", "t4", 11_050_000, 160_000_000) or False)
    emit("TransferOffered", "(ssa(st))", "t1", "p1", [("Quarterly report Q3 final (2).pdf", 2_400_000)])
    GLib.timeout_add(300, emit_clips)  # after the transfers: the text clip is the newest row


def emit_clips():
    emit("ClipboardOffered", "(stast)", "p1", 1, ["text/plain;charset=utf-8", "text/plain"], len(CLIPS[1].encode()))
    emit("ClipboardOffered", "(stast)", "p1", 2, ["text/plain", "x-kde-passwordManagerHint"], len(CLIPS[2]))
    emit("ClipboardOffered", "(stast)", "p1", 3, ["image/png"], 48_000)
    return False


PALETTE = [((255, 40, 90), (150, 60, 200)), ((10, 25, 60), (60, 150, 210)), ((250, 250, 248), (225, 220, 210)),
           ((120, 40, 200), (40, 60, 255)), ((240, 90, 170), (200, 60, 160))]
# PHOTOS=n lists n items (default 18: 11 today, 7 yesterday, then older ones a day apart); every 4th item from the
# fifth on is a video.
def photo(i):
    video = i % 4 == 0 and i > 0
    name = f"VID_{i:05}.mp4" if video else f"IMG_{i:05}.jpg"
    at = (ago(0, 18, 0) if i < 11 else ago(1, 12, 0) if i < 18 else ago(i - 16, 12, 0)) // 1000 - i * 60
    return (f"/DCIM/Camera/{name}", name, at, video, 42 + i if video else 0)


photos = [photo(i)
          for i in range(int(os.environ.get("PHOTOS", "18")))]


_full = {}


def full_photo(index):
    """The viewer's full-size file: a 900 px PNG, made once per photo."""
    if index not in _full:
        _full[index] = png(index, 900)
    return _full[index]


def png(index, size=64):
    """A diagonal gradient PNG: one of the mockup's five photo tones."""
    a, b = PALETTE[index % len(PALETTE)]
    rows = b"".join(b"\0" + bytes(round(a[c] + (b[c] - a[c]) * (x + y) / (2 * size)) for x in range(size) for c in range(3))
                    for y in range(size))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


# The first bytes of each access unit: an H.264 or HEVC access unit delimiter, an AV1 temporal delimiter OBU.
UNIT_START = {"h264": b"\0\0\0\x01\x09", "hevc": b"\0\0\0\x01\x46\x01", "av1": b"\x12\x00"}


def stream_mirror(sock, codec="h264"):
    """MIRROR env: an Annex B H.264/HEVC clip with an AUD per access unit, or an AV1 OBU stream (ffmpeg -f obu); one
    unit per access unit at MIRROR_FPS (30), looped."""
    fps = int(os.environ.get("MIRROR_FPS", "30"))
    data = open(os.environ["MIRROR"], "rb").read()
    mark = UNIT_START[codec]
    starts = [i for i in range(len(data) - len(mark) + 1) if data[i:i + len(mark)] == mark] + [len(data)]
    units = [data[a:b] for a, b in zip(starts, starts[1:])]
    try:
        start = time.monotonic()
        for k in itertools.count():
            n = k % len(units)
            sock.sendall(struct.pack(">IQB", len(units[n]), k * 1_000_000 // fps, 1 if n == 0 else 0) + units[n])
            time.sleep(max(0.0, start + (k + 1) / fps - time.monotonic()))
    except OSError:
        sock.close()


def call(_conn, _sender, _path, _iface, method, params, invocation):
    print("call", method, params, flush=True)
    if method == "CallAction" and params[1] == "decline":
        GLib.timeout_add(100, lambda: emit("Call", "(sssss)", params[0], "idle", "", "", "") and False)
    if method in ("OpenMirror", "OpenMirrorWith"):
        # MIRROR_CODEC: the codec of the MIRROR clip, returned whether or not the viewer offered it.
        codec = os.environ.get("MIRROR_CODEC", "h264") if method == "OpenMirrorWith" else "h264"
        viewer, phone = socket.socketpair()
        threading.Thread(target=stream_mirror, args=(phone, codec), daemon=True).start()
        fds = Gio.UnixFDList.new_from_array([viewer.detach()])
        reply = GLib.Variant("(huus)", (0, 1080, 2400, codec)) if method == "OpenMirrorWith" else GLib.Variant("(huu)", (0, 1080, 2400))
        invocation.return_value_with_unix_fd_list(reply, fds)
        if os.environ.get("CORNERS"):  # corner radius in 1/10000 of the width, sent as the phone's ScreenShape
            radius = int(os.environ["CORNERS"])
            GLib.timeout_add(300, lambda: emit("ScreenShape", "(saqa(qqqq))", params[0], [radius] * 4, []) and False)
        return
    if method == "WriteFile":
        reply = GLib.Variant("(t)", (params[2] + len(params[3]),))
        delay = int(os.environ.get("WRITE_DELAY", "0"))  # ms per write: lets a test cancel a copy
        GLib.timeout_add(delay, lambda: invocation.return_value(reply) and False) if delay else invocation.return_value(reply)
        return
    if method == "PullClipboard":
        data = CLIPS[params[1]].encode()
        fd = invocation.get_message().get_unix_fd_list().get(params[3])
        os.write(fd, data)
        os.close(fd)
        invocation.return_value(GLib.Variant("(t)", (len(data),)))
        return
    if method == "SendFiles":
        invocation.return_value(GLib.Variant("(s)", ("t-drop",)))
        return
    if method == "DeleteFile":
        invocation.return_value(None)
        return
    if method == "ListFiles":
        folders = {"/": [
                       ("DCIM", True, 0, ago(0, 17, 0) // 1000), ("Documents", True, 0, ago(4, 10, 0) // 1000),
                       ("Download", True, 0, ago(0, 16, 0) // 1000),
                       ("Quarterly report Q3 final (2).pdf", False, 2_400_000, ago(0, 18, 31) // 1000),
                       ("boarding-pass-LH2034.pdf", False, 182_000, ago(3, 9, 0) // 1000),
                       ("IMG_20261120_181203.jpg", False, 3_100_000, ago(0, 18, 12) // 1000)]}
        invocation.return_value(GLib.Variant("(a(sbtt))", (folders.get(params[1], []),)))
        return
    if method == "PhotosPage":
        offset, count, media = params[1], params[2], params[3]
        kind = [p for p in photos if media == "all" or (media == "videos") == p[3]] if connected else []
        invocation.return_value(GLib.Variant("(ua(sstbu))", (len(kind), kind[offset:offset + count])))
        return
    if method == "TextImage":  # MMS_IMAGE: a picture file to serve; else a gradient
        path = os.environ.get("MMS_IMAGE")
        invocation.return_value(GLib.Variant("(ay)", (open(path, "rb").read() if path else png(params[1], 240),)))
        return
    if method == "ReadFile" and any(p[0] == params[1] for p in photos):
        index = next(i for i, p in enumerate(photos) if p[0] == params[1])
        data = full_photo(index)
        invocation.return_value(GLib.Variant("(ay)", (data[params[2]:params[2] + params[3]],)))
        return
    if method == "TakePhoto":
        GLib.timeout_add(int(os.environ.get("TAKE_DELAY", "1500")),
                         lambda: invocation.return_value(GLib.Variant("(s)", (photos[0][0],))) and False)
        return
    if method == "Thumbnail":
        index = next(i for i, p in enumerate(photos) if p[0] == params[1])
        invocation.return_value(GLib.Variant("(ay)", (png(index),)))
        return
    if method == "SavePhotoTo":
        saved = os.path.join(params[2], params[1].rsplit("/", 1)[1])
        with open(saved, "wb") as out:
            out.write(b"photo")
        invocation.return_value(GLib.Variant("(s)", (saved,)))
        return
    if method == "SavePhoto":
        invocation.return_value(GLib.Variant("(s)", ("/home/user/Downloads/" + params[1].rsplit("/", 1)[1],)))
        return
    if method == "SetCommands":
        commands = params[0]
        if len(commands) > 64 or any(not n.strip() or not c.strip() for n, c in commands):
            invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "at most 64 commands, each with a name and a line")
            return
        props["Commands"] = GLib.Variant("a(ss)", commands)
        conn.emit_signal(None, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                         GLib.Variant("(sa{sv}as)", (IFACE, {"Commands": props["Commands"]}, [])))
    if method == "PhonePlayerCommand" and params[1] == "play-pause":
        name, app, title, artist, art, playing, pos, length, vol, at, cmds = props["PhonePlayers"].unpack()[params[0]]
        clock = int(time.time() * 1000)
        pos = min(length, pos + (clock - at if playing else 0))
        props["PhonePlayers"] = GLib.Variant("a{s(sssssbttixas)}", {params[0]: (name, app, title, artist, art, not playing, pos, length, vol, clock, cmds)})
        conn.emit_signal(None, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                         GLib.Variant("(sa{sv}as)", (IFACE, {"PhonePlayers": props["PhonePlayers"]}, [])))
    if method == "SendSystemCommand" and params[1] == "set-media-volume":
        controls = props["DeviceControls"].unpack()
        _, ringer, dnd, locked = controls[params[0]]
        props["DeviceControls"] = GLib.Variant("a{s(isii)}", {params[0]: (params[2], ringer, dnd, locked)})
        conn.emit_signal(None, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                         GLib.Variant("(sa{sv}as)", (IFACE, {"DeviceControls": props["DeviceControls"]}, [])))
    if method in ("StartPairing", "CancelPairing"):
        props["Pairing"] = GLib.Variant("b", method == "StartPairing")
        conn.emit_signal(None, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                         GLib.Variant("(sa{sv}as)", (IFACE, {"Pairing": props["Pairing"]}, [])))
    if method == "StartPairing":
        invocation.return_value(GLib.Variant("(ss)", ("482913", "andromeda-link://pair?code=482913&host=sitish-laptop")))
        return
    invocation.return_value(None)


slow = [float(os.environ.get("SLOW", "0"))]  # seconds the first property read waits: a slow daemon start


def get(_conn, _sender, _path, _iface, name):
    if slow[0] > 0:
        time.sleep(slow[0])
        slow[0] = 0
    if name in props:
        return props[name]
    sig = iface.lookup_property(name).signature
    empty = {"b": False, "s": ""}
    return GLib.Variant(sig, {} if sig.startswith("a{") else [] if sig[0] == "a" else empty.get(sig, 0))


conn.register_object(PATH, iface, call, get, None)
quick = {"Visible": GLib.Variant("b", True), "Name": GLib.Variant("s", "sitish-laptop")}
conn.register_object(PATH, node.lookup_interface(IFACE + ".QuickShare"), call, lambda *a: quick[a[4]], None)
# The app's own bus name: its proxy listens from then on.
Gio.bus_watch_name_on_connection(conn, "dev.andromeda.Link", Gio.BusNameWatcherFlags.NONE,
                                 lambda *_: GLib.timeout_add(800, emit_sample), None)
Gio.bus_own_name_on_connection(conn, IFACE, Gio.BusNameOwnerFlags.NONE, None, None)
print("fake link1 ready", flush=True)
GLib.MainLoop().run()
