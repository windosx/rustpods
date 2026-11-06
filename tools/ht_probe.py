#!/usr/bin/env python3
"""Probe the AirPods SensorV2 enable frame, sweeping the mode byte.

Why this exists
---------------
A macOS capture shows the accessory only streams SensorV2 while the host sits in
spatial audio mode 3 ("head tracked"):

    DeviceManager Set Spatial Mode: bundleID: com.apple.WebKit.GPU, mode: 3 head Tracking 1
    AudioLinkManager setSpatialModeWithBundleID: ... mode:3, headtrack:1
    Notify Spatial Audio Mode changed ... to 3 ... Head Tracking YES
    Sent custom message ... type 0x800, len 14        <- 59 ms later

The 14-byte enable we send carries a byte that is 0x01 in every payload we have,
which does not match mode 3. This tool sweeps that byte (and the two other
positions whose meaning is unconfirmed) and reports which value the accessory
acknowledges with the 9-byte reply.

Run it with `btmon` going in another terminal so you can confirm the frames
actually reach the controller:

    sudo btmon -w /tmp/probe.btsnoop
    python3 tools/ht_probe.py CC:68:E0:38:9C:E9

Success looks like a line containing "ACK" - that is the 9-byte reply macOS
receives right before samples start flowing.
"""

import argparse
import socket
import struct
import sys
import time

PSM_AACP = 0x1001

# BlueZ wants an encrypted link for PSM 0x1001.
SOL_BLUETOOTH = 274
BT_SECURITY = 4
BT_SECURITY_MEDIUM = 2

HANDSHAKE = bytes.fromhex("00000400010002000000000000000000")
FEATURE_FLAGS = bytes.fromhex("040004004D00D70000000000000000")
NOTIFICATIONS = bytes.fromhex("040004000F00FFFFFFFF")
OWNS_CONNECTION = bytes([0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x06, 0x01, 0, 0, 0])

AACP_HEADER = bytes([0x04, 0x00, 0x04, 0x00])
OP_HEAD_TRACKING = 0x17
DESCRIPTOR_SENSOR = 0x00000800
DESCRIPTOR_SENSOR_DATA_WX = 0x00100000

SENSOR_V2_REPORT_ID = 0xD2
ADVERT_PREFIX_LEN = 20

# Service id 22 is what the accessory advertises for `neorelay` on the first
# connection; it increments by 8 on every reconnect, which is why the tool reads
# the advertised value instead of trusting this default.
DEFAULT_SERVICE_ID = 22
DEFAULT_COOKIE = 0x00013483


def varint(value):
    out = bytearray()
    while True:
        low = value & 0x7F
        value >>= 7
        if value:
            out.append(low | 0x80)
        else:
            out.append(low)
            return bytes(out)


def service_setting(service, interval_us):
    config = bytes([0x01]) + interval_us.to_bytes(4, "little")
    return (
        bytes([0x08])
        + varint(service)
        + bytes([0x10, 0x02, 0x1A])
        + varint(len(config))
        + config
    )


def sensor_data_wx(seq, setting):
    body = bytes([0x08]) + varint(seq) + bytes([0x42]) + varint(len(setting)) + setting
    return (
        AACP_HEADER
        + OP_HEAD_TRACKING.to_bytes(1, "little")
        + bytes([0x00])
        + DESCRIPTOR_SENSOR_DATA_WX.to_bytes(4, "little")
        + len(body).to_bytes(2, "little")
        + body
    )


def subscribe(service, interval_us, seq):
    return sensor_data_wx(seq, service_setting(service, interval_us))


def start_frame(service, interval_us, seq):
    """The START probe that LibrePods and the Linux posture tool both send."""
    return sensor_data_wx(seq, service_setting(service, interval_us))


def enable_frame(flag_byte, service_id, cookie, mode_byte, interval_us):
    """14-byte HID enable on descriptor 0x800.

    Layout as observed: [flag][service:2][pad][cookie:4][report][mode?][interval:4]
    `mode_byte` is the unknown this tool sweeps.
    """
    body = (
        bytes([flag_byte])
        + service_id.to_bytes(2, "little")
        + bytes([0x00])
        + cookie.to_bytes(4, "little")
        + bytes([SENSOR_V2_REPORT_ID])
        + bytes([mode_byte])
        + interval_us.to_bytes(4, "little")
    )
    assert len(body) == 14, f"enable body must be 14 bytes, got {len(body)}"
    return (
        AACP_HEADER
        + OP_HEAD_TRACKING.to_bytes(1, "little")
        + bytes([0x00])
        + DESCRIPTOR_SENSOR.to_bytes(4, "little")
        + len(body).to_bytes(2, "little")
        + body
    )


def parse_advertisement(frame):
    """Returns (service_name, service_id, cookie) for a sensor advertisement."""
    if len(frame) < ADVERT_PREFIX_LEN or frame[4] != OP_HEAD_TRACKING:
        return None
    descriptor = int.from_bytes(frame[6:10], "little")
    if descriptor != DESCRIPTOR_SENSOR:
        return None
    if frame[ADVERTISEMENT_MARKER] != ord("<"):
        return None
    text = frame[ADVERTISEMENT_MARKER:].decode("utf-8", "replace")
    name = None
    key = "<key>AccessoryService</key><string>"
    start = text.find(key)
    if start != -1:
        rest = text[start + len(key):]
        end = rest.find("<")
        if end != -1:
            name = rest[:end]
    service_id = int.from_bytes(frame[13:15], "little")
    cookie = int.from_bytes(frame[16:20], "little")
    return name, service_id, cookie


ADVERTISEMENT_MARKER = ADVERT_PREFIX_LEN


class Link:
    def __init__(self, mac):
        self.mac = mac
        self.sock = socket.socket(
            socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP
        )
        self.sock.setsockopt(
            SOL_BLUETOOTH, BT_SECURITY, struct.pack("BB", BT_SECURITY_MEDIUM, 0)
        )
        self.sock.settimeout(8)
        self.sock.connect((mac, PSM_AACP))

    def send(self, data):
        self.sock.send(data)

    def drain(self, seconds):
        out = []
        deadline = time.time() + seconds
        self.sock.settimeout(0.1)
        while time.time() < deadline:
            try:
                out.append(self.sock.recv(4096))
            except socket.timeout:
                continue
            except OSError:
                break
        return out

    def read(self, seconds):
        """Collects replies for `seconds`, ignoring recv timeouts."""
        out = []
        deadline = time.time() + seconds
        self.sock.settimeout(0.15)
        while time.time() < deadline:
            try:
                out.append(self.sock.recv(4096))
            except socket.timeout:
                continue
            except OSError:
                return out
        return out

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


def looks_like_ack(frame):
    return (
        len(frame) == 9
        and frame[4] == OP_HEAD_TRACKING
        and int.from_bytes(frame[6:10], "little") == DESCRIPTOR_SENSOR
    )


def looks_like_sample(frame):
    # Samples are long and start with the AACP header; the 65/80/81-byte forms are
    # all longer than any control frame we send.
    return len(frame) >= 60 and frame[:4] == AACP_HEADER and frame[4] == OP_HEAD_TRACKING


def handshake(link):
    link.send(HANDSHAKE)
    time.sleep(0.2)
    link.send(FEATURE_FLAGS)
    time.sleep(0.2)
    link.send(NOTIFICATIONS)
    time.sleep(0.2)
    link.send(OWNS_CONNECTION)
    time.sleep(0.3)


def learn_identifiers(link, service_hint):
    """Subscribes once to learn this connection's service id and cookie."""
    for service, seq in ((9, 100), (7, 101), (6, 102)):
        link.send(subscribe(service, 20000, seq))
        time.sleep(0.08)

    service_id, cookie = None, None
    for frame in link.read(2.5):
        parsed = parse_advertisement(frame)
        if not parsed:
            continue
        name, sid, ck = parsed
        print(f"    advert {name or '?':10s} service_id={sid} cookie=0x{ck:08X}")
        if name == "neorelay":
            service_id, cookie = sid, ck

    if service_id is None:
        print(f"    no neorelay advert; falling back to service={service_hint}")
        service_id = service_hint
    if cookie is None:
        print(f"    no cookie in advert; falling back to 0x{DEFAULT_COOKIE:08X}")
        cookie = DEFAULT_COOKIE
    return service_id, cookie


def probe_once(link, label, frame, wait):
    link.drain(0.1)
    print(f"  {label:34s} -> {frame[12:].hex(' ')}")
    link.send(frame)
    samples = 0
    for reply in link.read(wait):
        if looks_like_ack(reply):
            print(f"      *** ACK: {reply.hex(' ')}")
            return "ack"
        if looks_like_sample(reply):
            samples += 1
    if samples:
        print(f"      *** {samples} sample frame(s)")
        return "sample"
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("mac", help="AirPods MAC address")
    ap.add_argument(
        "--service", type=int, default=DEFAULT_SERVICE_ID,
        help="fallback service id if no advertisement arrives",
    )
    ap.add_argument(
        "--wait", type=float, default=1.2, help="seconds to wait for each reply"
    )
    ap.add_argument(
        "--modes", default="1,3,0,2",
        help="comma-separated mode byte values to try in order",
    )
    args = ap.parse_args()

    modes = [int(x, 0) for x in args.modes.split(",")]

    print(f"connecting to {args.mac} on PSM 0x{PSM_AACP:04X}...")
    try:
        link = Link(args.mac)
    except OSError as exc:
        print(f"connect failed: {exc}")
        return 1
    print("connected")

    try:
        handshake(link)
        print("handshake sent; learning service ids from advertisements")
        service_id, cookie = learn_identifiers(link, args.service)
        print(f"using service_id={service_id} cookie=0x{cookie:08X}\n")

        attempts = []

        # First: START probes, which some firmwares answer without any enable.
        for service in (service_id, 14, 16):
            frame = start_frame(service, 40000, 115)
            attempts.append((f"START service={service}", frame))

        # Then the enable, sweeping the unknown mode byte.
        for mode in modes:
            for flag in (0x01, 0x00):
                frame = enable_frame(flag, service_id, cookie, mode, 20000)
                attempts.append((f"enable flag={flag} mode={mode}", frame))

        for label, frame in attempts:
            result = probe_once(link, label, frame, args.wait)
            if result:
                print(f"\nRESULT: {label} produced {result}.")
                print("Keep this value; it goes into rtbuddy.rs.")
                return 0

        print("\nNo variant was acknowledged.")
        print("Next steps:")
        print("  1. Confirm with btmon that these frames reach the controller.")
        print("  2. Try --modes with the full range: 0..7")
        print("  3. The accessory may require the host-side spatial session macOS sets;")
        print("     see the README section on the spatial audio chain.")
        return 1
    finally:
        link.close()


if __name__ == "__main__":
    sys.exit(main())
