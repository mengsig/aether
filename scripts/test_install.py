"""install.sh against a fake release on a local server: no access to github.com.

Runs under every POSIX shell found here (`dash` and `sh`), fed on stdin the
way `curl ... | sh` feeds it. `AETHER_INSTALL_TEST_SHELL` adds one more.
"""
import functools
import hashlib
import http.server
import io
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parent.parent
INSTALLER = ROOT / "install.sh"
TARGETS = (
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
RECEIPT = ".aether-receipt"
# Everything install.sh may call, besides the downloader and the hash tool.
BASE_TOOLS = ("grep", "cut", "head", "tr", "basename", "mkdir", "tar", "gzip",
              "cp", "chmod", "mv", "rm", "mktemp", "sed", "tail")
ALL_TOOLS = BASE_TOOLS + ("curl", "wget", "sha256sum", "shasum")
FAKE_UNAME = """#!/bin/sh
case "$1" in
  -s) echo "$FAKE_UNAME_S" ;;
  -m) echo "$FAKE_UNAME_M" ;;
  *) exit 1 ;;
esac
"""


def shells():
    """Each distinct POSIX shell here, by the binary it resolves to."""
    candidates = [shutil.which("dash"), shutil.which("sh"),
                  os.environ.get("AETHER_INSTALL_TEST_SHELL")]
    found = {}
    for candidate in filter(None, candidates):
        assert Path(candidate).is_file(), f"{candidate} is not a shell binary"
        found.setdefault(Path(candidate).resolve(), candidate)
    assert found, "no POSIX shell found to run install.sh"
    return list(found.values())


def fake_binary(version):
    return f'#!/bin/sh\necho "aether {version}"\n'.encode()


def write_release(directory, version, digest=None):
    """One release's assets for every target; `digest` overrides the checksum."""
    directory.mkdir(parents=True)
    for target in TARGETS:
        archive = directory / f"aether-{target}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            # The members scripts/release.sh packs; only the binary is installed.
            for name, mode, data in (("aether", 0o755, fake_binary(version)),
                                     ("LICENSE", 0o644, b"MIT\n"),
                                     ("THIRD-PARTY-NOTICES", 0o644, b"notices\n")):
                member = tarfile.TarInfo(name)
                member.mode = mode
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        published = digest or hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(f"{archive}.sha256").write_text(f"{published}  {archive.name}\n")


class RecordingHandler(http.server.SimpleHTTPRequestHandler):
    """Serves one sandbox's files and records each path asked for."""
    requested = None
    redirect = None
    redirect_chain = 0

    def log_message(self, format, *args):
        pass

    @staticmethod
    def split_hop(path):
        """Splits '/hop/N/rest' into (N, '/rest'); other paths are hop 0."""
        parts = path.split("/", 3)
        if parts[:2] == ["", "hop"]:
            return int(parts[2]), "/" + parts[3]
        return 0, path

    def do_GET(self):
        assert self.requested is not None, "bind `requested` in a subclass per server"
        self.requested.append(self.path)
        depth, self.path = self.split_hop(self.path)
        if depth < self.redirect_chain:
            self.send_response(302)
            self.send_header("Location", f"/hop/{depth + 1}{self.path}")
            self.end_headers()
            return
        if self.redirect:
            self.send_response(302)
            self.send_header("Location", self.redirect)
            self.end_headers()
            return
        super().do_GET()


class Sandbox:
    """A fake home, a fake release server and the tools one run may see."""

    def __init__(self, test, tls=False):
        scratch = ROOT / "target" / "install-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        test.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.home = self.root / "home"
        self.tmp = self.root / "tmp"
        self.www = self.root / "www"
        for directory in (self.home, self.tmp, self.www):
            directory.mkdir()
        self.install_dir = self.home / ".local" / "bin"
        self.requested = []
        recording = type("Handler", (RecordingHandler,), {"requested": self.requested})
        handler = functools.partial(recording, directory=str(self.www))
        self.handler = recording
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.certificate = self.enable_tls() if tls else None
        serve = functools.partial(self.server.serve_forever, poll_interval=0.01)
        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        test.addCleanup(thread.join)
        test.addCleanup(self.server.server_close)
        test.addCleanup(self.server.shutdown)
        scheme = "https" if tls else "http"
        self.base_url = f"{scheme}://127.0.0.1:{self.server.server_address[1]}"

    def enable_tls(self):
        certificate = self.root / "certificate.pem"
        key = self.root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(certificate), "-days", "1",
            "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1",
        ], check=True, capture_output=True, timeout=30)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        return certificate

    def tools(self, names, fake_uname=False):
        """A new directory holding only `names` and `uname`, for use as PATH."""
        directory = Path(tempfile.mkdtemp(prefix="bin-", dir=self.root))
        for name in names:
            found = shutil.which(name)
            if found:
                (directory / name).symlink_to(found)
        if fake_uname:
            (directory / "uname").write_text(FAKE_UNAME)
            (directory / "uname").chmod(0o755)
        else:
            (directory / "uname").symlink_to(shutil.which("uname"))
        return str(directory)

    def run(self, shell, script=None, path=None, uname=None, **env):
        """Feed install.sh (or `script`) to `shell` on stdin, as `curl | sh` does."""
        assert uname is None or path is None, "a fake uname needs the default PATH"
        if path is None:
            path = self.tools(ALL_TOOLS, fake_uname=uname is not None)
        environment = {
            "HOME": str(self.home),
            "PATH": path,
            "TMPDIR": str(self.tmp),
            "AETHER_INSTALL_BASE_URL": self.base_url,
            "no_proxy": "127.0.0.1",
            "NO_PROXY": "127.0.0.1",
        }
        if uname:
            environment["FAKE_UNAME_S"], environment["FAKE_UNAME_M"] = uname
        if self.certificate:
            environment["CURL_CA_BUNDLE"] = str(self.certificate)
            wgetrc = self.root / "wgetrc"
            wgetrc.write_text(f"ca_certificate = {self.certificate}\n")
            environment["WGETRC"] = str(wgetrc)
        environment.update(env)
        return subprocess.run(
            [shell], input=script if script is not None else INSTALLER.read_text(),
            env=environment, capture_output=True, text=True, timeout=60,
        )

    def installed_version(self):
        binary = self.install_dir / "aether"
        return subprocess.run([str(binary), "--version"], capture_output=True,
                              text=True, timeout=10, check=True).stdout.strip()

    def home_files(self):
        return sorted(str(path.relative_to(self.home))
                      for path in self.home.rglob("*") if not path.is_dir())


class InstallerTests(unittest.TestCase):
    def each_shell(self):
        for shell in shells():
            with self.subTest(shell=shell):
                yield shell, Sandbox(self)

    def test_dash_is_one_of_the_shells(self):
        """Shows as skipped, with the shells that did run, when dash is not installed."""
        names = sorted(Path(shell).resolve().name for shell in shells())
        self.assertTrue(names, "at least one shell runs the installer")
        if "dash" not in names:
            self.skipTest(f"dash is not installed: install.sh ran under {', '.join(names)} only")
        self.assertIn("dash", names)

    def assert_nothing_left_behind(self, sandbox):
        self.assertEqual(list(sandbox.tmp.iterdir()), [], "temp directory removed on exit")
        if sandbox.install_dir.exists():
            leftovers = [path.name for path in sandbox.install_dir.iterdir()
                         if path.name not in ("aether", RECEIPT)]
            self.assertEqual(leftovers, [], "no staged file beside the binary")

    def test_fresh_install_touches_only_the_install_directory(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            result = sandbox.run(shell)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(sandbox.installed_version(), "aether 9.9.9")
            self.assertIn("Installed aether 9.9.9", result.stdout)
            self.assertIn("aether update", result.stdout)
            self.assertEqual(sandbox.home_files(),
                             [f".local/bin/{RECEIPT}", ".local/bin/aether"])
            self.assertIn("installer=install.sh",
                          (sandbox.install_dir / RECEIPT).read_text())
            self.assert_nothing_left_behind(sandbox)

    def test_upgrade_replaces_an_older_binary_with_a_new_file(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            sandbox.install_dir.mkdir(parents=True)
            old = sandbox.install_dir / "aether"
            old.write_bytes(fake_binary("0.0.1"))
            old.chmod(0o755)
            # A hard link keeps the old inode alive, as a running daemon does.
            running = sandbox.root / "running"
            os.link(old, running)
            result = sandbox.run(shell)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(sandbox.installed_version(), "aether 9.9.9")
            self.assertEqual(running.read_bytes(), fake_binary("0.0.1"),
                             "replaced by rename, never written in place")
            self.assert_nothing_left_behind(sandbox)

    def test_a_pinned_version_comes_from_its_own_release(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            write_release(sandbox.www / "download" / "v1.2.3", "1.2.3")
            custom = sandbox.root / "custom dir"
            for version in ("v1.2.3", "1.2.3"):
                result = sandbox.run(shell, AETHER_VERSION=version,
                                     AETHER_INSTALL_DIR=str(custom))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("Installed aether 1.2.3", result.stdout)
                self.assertTrue((custom / RECEIPT).is_file())
            self.assertFalse(sandbox.install_dir.exists(), "the default directory is unused")
            self.assertTrue(all("/download/v1.2.3/" in path for path in sandbox.requested),
                            sandbox.requested)

    def test_checksum_mismatch_installs_nothing_and_keeps_the_old_binary(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9", digest="0" * 64)
            sandbox.install_dir.mkdir(parents=True)
            old = sandbox.install_dir / "aether"
            old.write_bytes(fake_binary("0.0.1"))
            old.chmod(0o755)
            result = sandbox.run(shell)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checksum mismatch", result.stderr)
            self.assertEqual(sandbox.installed_version(), "aether 0.0.1")
            self.assertFalse((sandbox.install_dir / RECEIPT).exists())
            self.assert_nothing_left_behind(sandbox)

    def test_a_checksum_that_is_not_a_digest_is_refused(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9", digest="not-a-digest")
            result = sandbox.run(shell)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not a SHA-256 digest", result.stderr)
            self.assertFalse(sandbox.install_dir.exists())

    def test_unsupported_platforms_are_refused_before_any_download(self):
        cases = (
            (("FreeBSD", "amd64"), "unsupported operating system 'FreeBSD'"),
            (("Linux", "riscv64"), "unsupported CPU 'riscv64'"),
            (("Darwin", "ppc"), "unsupported CPU 'ppc'"),
            (("MINGW64_NT-10.0", "x86_64"), "use WSL 2"),
        )
        for uname, message in cases:
            for shell, sandbox in self.each_shell():
                write_release(sandbox.www / "latest" / "download", "9.9.9")
                result = sandbox.run(shell, uname=uname)
                self.assertNotEqual(result.returncode, 0, uname)
                self.assertIn(message, result.stderr)
                self.assertEqual(sandbox.requested, [])
                self.assertFalse(sandbox.install_dir.exists())

    def test_each_supported_platform_downloads_its_own_asset(self):
        cases = (
            (("Linux", "x86_64"), "x86_64-unknown-linux-musl"),
            (("Linux", "aarch64"), "aarch64-unknown-linux-musl"),
            (("Darwin", "arm64"), "aarch64-apple-darwin"),
            (("Darwin", "x86_64"), "x86_64-apple-darwin"),
        )
        for uname, target in cases:
            for shell, sandbox in self.each_shell():
                write_release(sandbox.www / "latest" / "download", "9.9.9")
                result = sandbox.run(shell, uname=uname)
                self.assertEqual(result.returncode, 0, result.stderr)
                asset = f"/latest/download/aether-{target}.tar.gz"
                self.assertEqual(sandbox.requested, [asset, f"{asset}.sha256"])

    def test_path_message_names_the_line_to_add_only_when_it_is_missing(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            missing = sandbox.run(shell)
            self.assertEqual(missing.returncode, 0, missing.stderr)
            self.assertIn(f"{sandbox.install_dir} is not on your PATH", missing.stdout)
            self.assertIn(f'export PATH="{sandbox.install_dir}:$PATH"', missing.stdout)
            self.assertEqual(sandbox.home_files(),
                             [f".local/bin/{RECEIPT}", ".local/bin/aether"],
                             "no shell profile is written")
            on_path = f"{sandbox.install_dir}:{sandbox.tools(ALL_TOOLS)}"
            present = sandbox.run(shell, path=on_path)
            self.assertEqual(present.returncode, 0, present.stderr)
            self.assertNotIn("not on your PATH", present.stdout)
            self.assertNotIn("comes first", present.stdout)

    def test_wget_is_used_when_curl_is_missing(self):
        if not shutil.which("wget"):
            self.skipTest("needs wget")
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            path = sandbox.tools(BASE_TOOLS + ("wget", "sha256sum", "shasum"))
            result = sandbox.run(shell, path=path)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(sandbox.installed_version(), "aether 9.9.9")

    @unittest.skipUnless(shutil.which("openssl"), "needs openssl for local TLS")
    def test_https_install_and_downgrade_rejection(self):
        for downloader in ("curl", "wget"):
            if not shutil.which(downloader):
                continue
            for shell in shells():
                with self.subTest(downloader=downloader, shell=shell):
                    sandbox = Sandbox(self, tls=True)
                    write_release(sandbox.www / "latest" / "download", "9.9.9")
                    path = sandbox.tools(BASE_TOOLS + (downloader, "sha256sum"))
                    result = sandbox.run(shell, path=path)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(sandbox.installed_version(), "aether 9.9.9")
                    destination = Sandbox(self)
                    sandbox.handler.redirect = destination.base_url + "/release"
                    sandbox.requested.clear()
                    result = sandbox.run(shell, path=path)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("could not download", result.stderr)
                    self.assertEqual(len(sandbox.requested), 1)
                    self.assertEqual(destination.requested, [], "HTTP redirect was never fetched")

    def test_redirect_target_follows_https_and_refuses_the_rest(self):
        script = INSTALLER.read_text()
        script = script[:script.rstrip("\n").rfind("\n") + 1]
        shell = shells()[0]
        sandbox = Sandbox(self)
        current = "https://example.test/a/b/asset.tar.gz?x=1"
        cases = (
            ("https://cdn.test/f", "https://cdn.test/f"),
            ("//cdn.test/f", "https://cdn.test/f"),
            ("/root/f", "https://example.test/root/f"),
            ("f.sha256", "https://example.test/a/b/f.sha256"),
            ("?token=abc", "https://example.test/a/b/asset.tar.gz?token=abc"),
            ("#frag", "https://example.test/a/b/asset.tar.gz?x=1#frag"),
            ("..", "https://example.test/a/"),
            ("../up/./f", "https://example.test/a/up/f"),
            ("https://cdn.test/x/../y/.", "https://cdn.test/y/"),
            ("", ""),
        )
        for location, expected in cases:
            with self.subTest(location=location):
                header = f"  Location: {location} [following]\r\n" if location else ""
                headers = sandbox.root / "headers"
                headers.write_text(f"  HTTP/1.1 302 Found\r\n{header}")
                result = sandbox.run(shell, script=f'{script}redirect_target "{current}" "{headers}"\n')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)
        current = "https://example.test?token=abc#old"
        for location, expected in (("/asset", "https://example.test/asset"),
                                   ("rel", "https://example.test/rel"),
                                   ("?q=2", "https://example.test/?q=2")):
            with self.subTest(location=location, current=current):
                headers.write_text(f"  Location: {location}\r\n")
                result = sandbox.run(shell, script=f'{script}redirect_target "{current}" "{headers}"\n')
                self.assertEqual(result.stdout.strip(), expected, result.stderr)
        current = "https://example.test/a/"
        headers.write_text("  Location: asset\r\n")
        result = sandbox.run(shell, script=f'{script}redirect_target "{current}" "{headers}"\n')
        self.assertEqual(result.stdout.strip(), "https://example.test/a/asset", result.stderr)
        for location in ("http://cdn.test/f", "ftp://cdn.test/f"):
            with self.subTest(location=location):
                headers.write_text(f"  Location: {location}\r\n")
                result = sandbox.run(shell, script=f'{script}redirect_target "{current}" "{headers}"\n')
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"refused redirect to non-https URL '{location}'", result.stderr)

    @unittest.skipUnless(shutil.which("openssl") and shutil.which("wget"), "needs openssl and wget")
    def test_wget_follows_https_redirects_up_to_the_limit(self):
        for shell in shells():
            with self.subTest(shell=shell):
                sandbox = Sandbox(self, tls=True)
                write_release(sandbox.www / "latest" / "download", "9.9.9")
                sandbox.handler.redirect_chain = 3
                path = sandbox.tools(BASE_TOOLS + ("wget", "sha256sum"))
                result = sandbox.run(shell, path=path)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(sandbox.installed_version(), "aether 9.9.9")
                sandbox.handler.redirect_chain = 6
                sandbox.install_dir.joinpath("aether").unlink()
                result = sandbox.run(shell, path=path)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("more than 5 redirects", result.stderr)

    def test_plain_http_redirect_is_refused(self):
        for downloader in ("curl", "wget"):
            if not shutil.which(downloader):
                continue
            for shell, sandbox in self.each_shell():
                sandbox.handler.redirect = "http://example.invalid/release"
                path = sandbox.tools(BASE_TOOLS + (downloader, "sha256sum"))
                result = sandbox.run(shell, path=path)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("could not download", result.stderr)
                self.assertEqual(len(sandbox.requested), 1)
                self.assertFalse(sandbox.install_dir.exists())

    def test_without_a_hash_tool_nothing_is_installed(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            result = sandbox.run(shell, path=sandbox.tools(BASE_TOOLS + ("curl", "wget")))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("sha256sum or shasum is needed", result.stderr)
            self.assertFalse(sandbox.install_dir.exists())
            self.assert_nothing_left_behind(sandbox)

    def test_bad_settings_are_refused_before_any_download(self):
        cases = (
            ({"AETHER_VERSION": "v1.2"}, "AETHER_VERSION must look like v1.0.0"),
            ({"AETHER_VERSION": "v1.2.3/../.."}, "AETHER_VERSION must look like v1.0.0"),
            ({"AETHER_INSTALL_BASE_URL": "http://example.com/releases"}, "must be an https:// URL"),
            ({"AETHER_INSTALL_BASE_URL": "http://127.0.0.1.example.com"}, "must be an https:// URL"),
            ({"AETHER_INSTALL_BASE_URL": "http://127.0.0.1:1@example.invalid"}, "must not contain userinfo"),
            ({"AETHER_INSTALL_DIR": "relative/bin"}, "must be an absolute path"),
        )
        for env, message in cases:
            for shell, sandbox in self.each_shell():
                write_release(sandbox.www / "latest" / "download", "9.9.9")
                result = sandbox.run(shell, **env)
                self.assertNotEqual(result.returncode, 0, env)
                self.assertIn(message, result.stderr)
                self.assertEqual(sandbox.requested, [])
                self.assertEqual(sandbox.home_files(), [])

    def test_a_release_that_does_not_exist_installs_nothing(self):
        for shell, sandbox in self.each_shell():
            write_release(sandbox.www / "latest" / "download", "9.9.9")
            result = sandbox.run(shell, AETHER_VERSION="v7.7.7")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("could not download", result.stderr)
            self.assertFalse(sandbox.install_dir.exists())
            self.assert_nothing_left_behind(sandbox)

    def test_a_truncated_download_of_the_installer_runs_nothing(self):
        script = INSTALLER.read_text()
        last_line = script.rstrip("\n").rfind("\n") + 1
        self.assertEqual(script[last_line:].strip(), 'main "$@"')
        shell = shells()[0]
        sandbox = Sandbox(self)
        write_release(sandbox.www / "latest" / "download", "9.9.9")
        path = sandbox.tools(ALL_TOOLS)
        for cut in list(range(200, last_line, 331)) + [last_line, last_line + 3]:
            with self.subTest(cut=cut):
                sandbox.run(shell, script=script[:cut], path=path)
                self.assertEqual(sandbox.requested, [], f"cut at byte {cut}")
                self.assertEqual(sandbox.home_files(), [], f"cut at byte {cut}")


if __name__ == "__main__":
    unittest.main()
