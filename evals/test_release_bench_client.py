"""The bench's client reaches the last release's daemon after a protocol bump."""
import json
import os
import socket
import tempfile
import threading
import unittest

from aether_client import PROTOCOL_VERSION, AetherClient, AetherError, common_version, protocol

OLDER = PROTOCOL_VERSION - 1


def refusal(asked, newest):
    return AetherError(
        "incompatible",
        f"client protocol {asked} is not served: this daemon speaks {protocol.MIN_PROTOCOL_VERSION} to {newest}",
    )


class OlderDaemon:
    """A Unix-socket daemon that serves up to OLDER, refusing newer hellos the
    way a real daemon does, and records every version it was asked for."""

    def __init__(self, path):
        self.asked = []
        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._listener.bind(path)
        self._listener.listen(4)
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        while True:
            try:
                conn, _ = self._listener.accept()
            except OSError:
                return
            with conn, conn.makefile("rwb") as stream:
                hello = json.loads(stream.readline())
                version = hello["protocol_version"]
                self.asked.append(version)
                if version > OLDER:
                    result = {"Err": {"code": "incompatible", "message": refusal(version, OLDER).message}}
                else:
                    result = {"Ok": {"reply": "welcome", "data": {"protocol_version": version}}}
                stream.write((json.dumps({"frame": "reply", "id": hello["req"], "result": result}) + "\n").encode())
                stream.flush()
                if version > OLDER:
                    continue
                stream.readline()

    def close(self):
        self._listener.close()


class CommonVersionTests(unittest.TestCase):
    def test_an_older_daemon_is_spoken_to_at_its_newest_version(self):
        self.assertEqual(common_version(refusal(PROTOCOL_VERSION, OLDER), PROTOCOL_VERSION), OLDER)

    def test_no_retry_without_a_newer_common_version(self):
        self.assertIsNone(common_version(refusal(PROTOCOL_VERSION, PROTOCOL_VERSION), PROTOCOL_VERSION))
        self.assertIsNone(common_version(refusal(PROTOCOL_VERSION, protocol.MIN_PROTOCOL_VERSION - 1), PROTOCOL_VERSION))
        self.assertIsNone(common_version(AetherError("unauthorized", "bad token"), PROTOCOL_VERSION))
        self.assertIsNone(common_version(AetherError("incompatible", "some other wording"), PROTOCOL_VERSION))

    def test_the_client_reconnects_once_at_the_common_version(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/var/tmp")) as scratch:
            path = os.path.join(scratch, "d.sock")
            daemon = OlderDaemon(path)
            self.addCleanup(daemon.close)
            client = AetherClient(path)
            self.addCleanup(client.close)
            self.assertEqual(client.welcome["protocol_version"], OLDER)
            self.assertEqual(daemon.asked, [PROTOCOL_VERSION, OLDER])


if __name__ == "__main__":
    unittest.main()
