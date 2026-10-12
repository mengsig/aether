#!/usr/bin/env python3
"""Send a message with an image through a running daemon and check the model got it.

Called by scripts/e2e_mock.sh once the daemon and the mock model are up:

    scripts/check_image_send.py <socket> <aether-binary> <scratch-dir>

The mock model answers with what it received, so this compares bytes sent
with bytes seen. A model with vision must receive the image itself; one
without is told where the daemon stored it, and that file must hold it.
"""

import base64
import hashlib
import os
import pathlib
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "evals"))

from aether_client import AetherClient  # noqa: E402

# One transparent pixel.
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="
)
VISION_MODELS = ("mock/mock-vision", "mock-responses/mock-1")
TEXT_ONLY_MODEL = "mock/mock-1"
NO_IMAGE_PREFIX = "I was sent no image, only the file "


def fail(what):
    print(f"image send: {what}", file=sys.stderr)
    sys.exit(1)


def ask(client, binary, model, images):
    """A new agent on `model`, sent one message with `images` by the CLI. Returns (agent, reply)."""
    agent = client.create_agent(role="chat", model=model, long_lived=True)["id"]
    client.subscribe(agent)
    attach = [arg for image in images for arg in ("--image", str(image))]
    subprocess.run([binary, "send", agent, *attach, "what is this?"], check=True)
    return agent, client.wait_for_reply(agent, timeout=60)


def check_vision(client, binary, shot, model):
    agent, reply = ask(client, binary, model, [shot, shot])
    seen = f"image/png, {len(PNG)} bytes, sha256 {hashlib.sha256(PNG).hexdigest()}"
    if reply != f"I was sent 2 image(s): {seen}; {seen}":
        fail(f"{model} did not receive both images byte for byte: {reply!r}")
    delivered = client.wait_for_event(agent, "inbox.delivered", timeout=10)["kind"]["item"]["kind"]
    blocks = [block["type"] for block in delivered["content"]]
    if blocks != ["text", "image_ref", "image_ref"]:
        fail(f"the delivered message is not text and two references: {delivered}")
    if base64.b64encode(PNG).decode() in str(delivered):
        fail("the event log carries the image bytes, not a reference")
    print(f"{model}: received both images byte for byte")


def check_text_only(client, binary, shot):
    _, reply = ask(client, binary, TEXT_ONLY_MODEL, [shot])
    if not reply.startswith(NO_IMAGE_PREFIX):
        fail(f"a model without vision was not told the stored file: {reply!r}")
    stored = pathlib.Path(reply[len(NO_IMAGE_PREFIX):])
    if stored.read_bytes() != PNG:
        fail(f"{stored} does not hold the image that was sent")
    print(f"{TEXT_ONLY_MODEL} (no vision): told the stored file {stored.name}")


def check_refusal(client, binary, scratch):
    """A file that is not an image is refused out loud and nothing is delivered."""
    agent = client.create_agent(role="chat", model=TEXT_ONLY_MODEL, long_lived=True)["id"]
    notes = scratch / "notes.png"
    notes.write_text("not a picture\n")
    sent = subprocess.run([binary, "send", agent, "--image", str(notes), "x"], capture_output=True, text=True)
    if sent.returncode == 0:
        fail("a file that is not an image was sent")
    if "notes.png" not in sent.stderr:
        fail(f"the refusal does not name the file: {sent.stderr!r}")
    print("not an image: refused, naming the file")


def main():
    if len(sys.argv) != 4:
        fail("usage: check_image_send.py <socket> <aether-binary> <scratch-dir>")
    socket, binary, scratch = sys.argv[1], sys.argv[2], pathlib.Path(sys.argv[3])
    shot = scratch / "shot.png"
    shot.write_bytes(PNG)
    client = AetherClient(socket)
    for model in VISION_MODELS:
        check_vision(client, binary, shot, model)
    check_text_only(client, binary, shot)
    check_refusal(client, binary, scratch)


if __name__ == "__main__":
    main()
