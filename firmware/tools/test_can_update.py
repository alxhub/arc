import struct
import unittest
import zlib

import can_update


class ProtocolTests(unittest.TestCase):
    def test_begin_and_chunk_layout(self):
        image = struct.pack("<II", 0x20007800, 0x08004009) + b"123456789"
        can_update.validate_image(image, "dst-rev2")
        wire = can_update.begin(0xABCDEF, "dst-rev2", 7, image)
        self.assertEqual(len(wire), 20)
        self.assertEqual(wire[:10], bytes.fromhex("0101abcdef0200000007"))
        self.assertEqual(wire[10:14], len(image).to_bytes(4, "big"))
        self.assertEqual(wire[14:18], zlib.crc32(image).to_bytes(4, "big"))
        payload = can_update.chunk(0xABCDEF, 7, 0, image)
        self.assertEqual(len(payload), 64)
        self.assertEqual(payload[13], len(image))
        self.assertEqual(payload[14:14 + len(image)], image)
        self.assertEqual(payload[14 + len(image):], b"\xff" * (50 - len(image)))

    def test_rejects_wrong_link_address(self):
        image = struct.pack("<II", 0x20007800, 0x08000009) + b"12345678"
        with self.assertRaises(ValueError):
            can_update.validate_image(image, "psu")


if __name__ == "__main__":
    unittest.main()
