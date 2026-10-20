#!/usr/bin/env python3
"""Exercise the real terminal renderer with a mock transcript and no daemon."""
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time


def exercise(executable, workspace):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 1280, 640))
    env = dict(os.environ, AETHER_VIEWER_FIXTURE="1", AETHER_VIEWER_WORKSPACE=workspace,
               AETHER_VIEWER_ASSERT_CLOSED="1")
    process = subprocess.Popen([executable, "--ignored", "--exact",
                                "runner::viewer_tests::terminal_fixture", "--nocapture"],
                               stdin=slave, stdout=slave, stderr=slave, env=env,
                               start_new_session=True,
                               preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
    os.close(slave)
    output = bytearray()
    deadline = time.monotonic() + 12

    def receive(needle):
        while needle not in output:
            assert time.monotonic() < deadline, ("terminal timed out", bytes(output[-3000:]))
            if select.select([master], [], [], 0.05)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError as error:
                    raise AssertionError(bytes(output[-3000:])) from error
                assert chunk, "fixture closed early"
                output.extend(chunk)

    try:
        receive(b"sample.png")
        # Keyboard activation takes the same load path as a transcript click.
        os.write(master, b"\x1bo")
        receive(b"Files")
        os.write(master, b"\r")
        receive("▀".encode())
        os.write(master, b"\x1b[19~")  # F8: more mock agent output.
        receive(b"continues")
        os.write(master, b"\x1b")
        time.sleep(0.08)
        os.write(master, b"\x03")
        process.wait(timeout=3)
        assert process.returncode == 0, bytes(output[-3000:])
        print("viewer PTY passed: picker, half-block image, streaming, close")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        os.close(master)


if __name__ == "__main__":
    exercise(*sys.argv[1:])
