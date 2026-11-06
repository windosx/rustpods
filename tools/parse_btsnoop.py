#!/usr/bin/env python3
"""Parse a btsnoop capture and extract AACP / RTBuddy frames.

btsnoop is the format `btmon -w file.btsnoop` writes on Linux:

    header   : b"btsnoop\\0" | version:u32 | datalink:u32
    record   : orig_len:u32 | incl_len:u32 | flags:u32
               | drops:u32 | timestamp:i64 | data[incl_len]

`flags` bit 0 is the direction: 0 = received by the host, 1 = sent by it.

The point of this script is to show every byte of the head-tracking exchange,
including frames the accessory sends on channels our own socket never sees.

Usage:
    python3 parse_btsnoop.py capture.btsnoop [--hex] [--aacp-only]
"""

import argparse
import struct
import sys

AACP_HEADER = bytes([0x04, 0x00, 0x04, 0x00])
# RTBuddy frames the accessory uses for sensor work.
DESCRIPTOR_SENSOR = 0x00000800
DESCRIPTOR_SENSOR_TRIGGER = 0x00040000
DESCRIPTOR_SENSOR_DATA_WX = 0x00100000


def read_records(path):
    """Yields (sent_by_host, timestamp_us, payload) for every HCI packet."""
    with open(path, "rb") as fh:
        blob = fh.read()

    if not blob.startswith(b"btsnoop\x00"):
        raise SystemExit(f"{path}: not a btsnoop file")
    if len(blob) < 16:
        raise SystemExit(f"{path}: truncated header")

    version, datalink = struct.unpack_from(">II", blob, 8)
    print(f"# btsnoop version={version} datalink={datalink}")
    if datalink == 2009:
        print("# datalink: Linux monitor (has a 1-byte monitor type prefix)")
    elif datalink == 1002:
        print("# datalink: HCI UART (H4, first byte is the packet type)")
    elif datalink == 1001:
        print("# datalink: HCI unencapsulated")

    offset = 16
    while offset + 24 <= len(blob):
        orig_len, incl_len, flags, _drops, ts = struct.unpack_from(">IIIIq", blob, offset)
        offset += 24
        if incl_len > len(blob) - offset:
            print(f"# warning: truncated record at offset {offset}", file=sys.stderr)
            break
        data = blob[offset:offset + incl_len]
        offset += incl_len
        yield bool(flags & 1), ts, data


def acl_payload(data, datalink):
    """Strips HCI/monitor headers so only the L2CAP payload remains.

    Returns (handle, payload) for ACL packets, or None for anything else.
    """
    body = data
    if datalink == 2009:
        # Linux monitor: [monitor_type:1][flags:2][hdr_len:1][hdr:...]
        if len(body) < 4:
            return None
        body = body[4:]
    if not body:
        return None

    pkt_type = body[0]
    # H4 packet types: 0x02 = ACL.
    if pkt_type != 0x02:
        if body[0] & 0x0F == 0x02 or (datalink == 2009 and len(body) > 4):
            pass
        return None

    if len(body) < 5:
        return None
    handle_flags, dlen = struct.unpack_from("<HH", body, 1)
    payload = body[5:5 + dlen]
    return handle_flags & 0x0FFF, payload


def classify(payload):
    """Describes an RTBuddy frame, if the payload is one."""
    idx = payload.find(AACP_HEADER)
    while idx != -1:
        frame = payload[idx:]
        if len(frame) >= 12 and frame[4] == 0x17:
            descriptor = int.from_bytes(frame[6:10], "little")
            body_len = int.from_bytes(frame[10:12], "little")
            names = {
                DESCRIPTOR_SENSOR: "SENSOR(0x800)",
                DESCRIPTOR_SENSOR_TRIGGER: "TRIGGER(0x40000)",
                DESCRIPTOR_SENSOR_DATA_WX: "SENSOR_DATA_WX(0x100000)",
            }
            return names.get(descriptor, f"0x{descriptor:08X}"), body_len, frame
        idx = payload.find(AACP_HEADER, idx + 1)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--hex", action="store_true", help="dump full payload hex")
    ap.add_argument("--aacp-only", action="store_true", help="only RTBuddy frames")
    args = ap.parse_args()

    datalink = 0
    with open(args.path, "rb") as fh:
        head = fh.read(16)
    if len(head) >= 16:
        _, datalink = struct.unpack_from(">II", head, 8)

    total = 0
    acl = 0
    rtb = 0

    for sent, ts, data in read_records(args.path):
        total += 1
        parsed = acl_payload(data, datalink)
        if parsed is None:
            if not args.aacp_only and args.hex:
                print(f"{'SENT' if sent else 'RECV'} HCI {data[:32].hex(' ')}")
            continue
        _handle, payload = parsed
        acl += 1

        info = classify(payload)
        if info is None:
            continue
        rtb += 1
        name, body_len, frame = info
        arrow = "Mac->Pods" if sent else "Pods->Mac"
        print(f"\n=== {arrow}  {name}  body_len={body_len}  frame_len={len(frame)} ===")
        print(f"    {frame.hex(' ')}")
        body = frame[12:12 + body_len]
        if body:
            print(f"    body: {body.hex(' ')}")

    print(f"\n# HCI packets: {total}, ACL: {acl}, RTBuddy frames: {rtb}")
    if rtb == 0:
        print("# No RTBuddy frames found.")
        print("# If the capture is empty, the AirPods may not have been connected")
        print("# to this adapter while btmon was running.")


if __name__ == "__main__":
    main()
