#!/usr/bin/env python3
"""org.andromeda.Link1 over andromeda-linkd's peer-to-peer socket (macOS), where gdbus cannot go: it says Hello first.

  link1_socket.py call METHOD [JSON ARG]...   prints the reply as one JSON array
  link1_socket.py get PROPERTY                prints the value as JSON
  link1_socket.py watch                       prints each signal as {"signal": name, "args": [...]}, one per line
  link1_socket.py mirror DEVICE SECONDS       opens the mirror and prints {"width", "height", "bytes"} read

Argument types come from api/org.andromeda.Link1.xml. The socket is $ANDROMEDA_LINK_SOCKET, else the macOS default.
Needs jeepney.
"""
import json
import os
import socket
import sys
import time
import xml.etree.ElementTree as ElementTree
from pathlib import Path

from jeepney import DBusAddress, HeaderFields, MessageType, Properties, new_method_call
from jeepney.auth import BEGIN, Authenticator
from jeepney.io.blocking import DBusConnection
from jeepney.io.common import MessageFilters
from jeepney.wrappers import unwrap_msg
from jeepney.fds import FileDescriptor

INTERFACE = "org.andromeda.Link1"
LINK1 = DBusAddress("/org/andromeda/Link1", bus_name=INTERFACE, interface=INTERFACE)
XML = Path(__file__).resolve().parents[2] / "api" / "org.andromeda.Link1.xml"


class PeerConnection(DBusConnection):
    """A DBusConnection that skips Hello: a peer-to-peer server has no message bus to answer it."""

    def __init__(self, sock):
        super(DBusConnection, self).__init__(sock, enable_fds=True)
        self._filters = MessageFilters()


def socket_path():
    if path := os.environ.get("ANDROMEDA_LINK_SOCKET"):
        return path
    return str(Path.home() / "Library/Application Support/Andromeda Link/link.sock")


def connect():
    # jeepney's prep_socket sends SCM_CREDS wherever Python defines it; macOS rejects that with EINVAL.
    sock = socket.socket(socket.AF_UNIX)
    sock.settimeout(5)
    sock.connect(socket_path())
    sock.send(b"\0")
    auth = Authenticator(enable_fds=True, inc_null_byte=False)
    for request in auth:
        sock.sendall(request)
        reply = sock.recv(1024)
        if not reply:
            raise ConnectionResetError("linkd closed the socket in the handshake")
        auth.feed(reply)
    sock.sendall(BEGIN)
    sock.settimeout(None)
    return PeerConnection(sock)


def in_signature(method):
    for node in ElementTree.parse(XML).getroot().iter("method"):
        if node.get("name") == method:
            return "".join(arg.get("type") for arg in node.iter("arg") if arg.get("direction", "in") == "in")
    sys.exit(f"link1_socket.py: {method} is not in {XML.name}")


def plain(value):
    if isinstance(value, FileDescriptor):
        return value.to_raw_fd()
    if isinstance(value, bytes):
        return value.hex()
    if isinstance(value, (list, tuple)):
        return [plain(item) for item in value]
    if isinstance(value, dict):
        return {str(key): plain(item) for key, item in value.items()}
    return value


def call(connection, method, args):
    signature = in_signature(method)
    message = new_method_call(LINK1, method, signature or None, tuple(args))
    return unwrap_msg(connection.send_and_get_reply(message, timeout=60))


def main():
    command, args = sys.argv[1], sys.argv[2:]
    with connect() as connection:
        if command == "call":
            print(json.dumps(plain(call(connection, args[0], [json.loads(arg) for arg in args[1:]]))))
        elif command == "get":
            reply = connection.send_and_get_reply(Properties(LINK1).get(args[0]), timeout=60)
            print(json.dumps(plain(unwrap_msg(reply)[0][1])))
        elif command == "watch":
            while True:
                message = connection.receive()
                if message.header.message_type == MessageType.signal:
                    print(json.dumps({"signal": message.header.fields[HeaderFields.member], "args": plain(message.body)}), flush=True)
        elif command == "mirror":
            stream, width, height = call(connection, "OpenMirror", [args[0]])
            fd = stream.to_raw_fd()
            os.set_blocking(fd, False)
            deadline, total = time.monotonic() + float(args[1]), 0
            while time.monotonic() < deadline:
                try:
                    chunk = os.read(fd, 65536)
                except BlockingIOError:
                    time.sleep(0.05)
                    continue
                if not chunk:
                    break
                total += len(chunk)
            print(json.dumps({"width": width, "height": height, "bytes": total}))
        else:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
