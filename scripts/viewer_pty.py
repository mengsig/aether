#!/usr/bin/env python3
"""Exercise the real terminal renderer with a mock transcript and no daemon."""
import fcntl
import os
import pty
import re
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

    def read_more():
        assert time.monotonic() < deadline, ("terminal timed out", bytes(output[-3000:]))
        if not select.select([master], [], [], 0.05)[0]:
            return False
        try:
            chunk = os.read(master, 65536)
        except OSError as error:
            raise AssertionError(bytes(output[-3000:])) from error
        assert chunk, "fixture closed early"
        output.extend(chunk)
        return True

    def receive(needle, start=0):
        while needle not in output[start:]:
            read_more()

    def see(needle):
        """Wait until the screen shows `needle`; diffed redraws split text in the stream."""
        while needle not in screen(output):
            while not read_more():
                pass

    try:
        receive(b"sample.png")
        # Keyboard activation takes the same load path as a transcript click.
        os.write(master, b"\x1bo")
        receive(b"Files")
        os.write(master, b"\r")
        receive("▀".encode())
        os.write(master, b"\x1b[19~")  # F8: more mock agent output.
        receive(b"continues")
        click_documents(master, output, receive, see)
        os.write(master, b"\x1b")
        time.sleep(0.08)
        os.write(master, b"\x03")
        process.wait(timeout=3)
        assert process.returncode == 0, bytes(output[-3000:])
        assert b"SECRET_SCRIPT" not in output, "HTML script text reached the terminal"
        print("viewer PTY passed: picker, half-block image, PDF, HTML, streaming, close")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        os.close(master)


def screen(data, rows=40, columns=160):
    """The visible text after replaying cursor moves and prints; colours are ignored."""
    grid = [[" "] * columns for _ in range(rows)]
    row = column = 0
    pattern = rb"\x1b\[([0-9;?<]*)([A-Za-z])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b.|([^\x1b]+)"
    for found in re.finditer(pattern, bytes(data), re.S):
        if found.group(2) == b"H":
            numbers = [int(n or 1) for n in found.group(1).split(b";")] + [1]
            row, column = numbers[0] - 1, numbers[1] - 1
        for character in (found.group(3) or b"").decode(errors="replace"):
            if character == "\r":
                column = 0
            elif character == "\n":
                row += 1
            elif 0 <= row < rows and 0 <= column < columns:
                grid[row][column] = character
                column += 1
    return "\n".join("".join(line) for line in grid)


def click_documents(master, output, receive, see):
    """Click the PDF and the HTML reference in the transcript, as an operator would."""
    mark = len(output)
    os.write(master, b"\x1b[20~")  # F9: the fixture reports hit regions.
    receive(b"]END", mark)
    found = re.search(rb"HITS\[([^\]]*)\]END", bytes(output[mark:]))
    assert found, bytes(output[mark:])
    hits = dict(re.findall(rb"([^@;]+)@(\d+,\d+);", found.group(1)))

    def click(target):
        x, y = (int(value) + 1 for value in hits[target].split(b","))
        os.write(master, f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())

    click(b"report.pdf")
    expected = os.environ["VIEWER_PDF_SHOWS"]
    if expected == "pages":
        see("report.pdf · page 1/2")
        os.write(master, b"\x1b[6~")  # PageDown turns the page.
        see("report.pdf · page 2/2")
    else:
        see(expected)
    click(b"page.html")
    see("Quarterly report")
    see("Links (not followed)")


if __name__ == "__main__":
    exercise(*sys.argv[1:])
