#!/usr/bin/env python3
"""Inspect native release artifacts and launch the shipped binary in a PTY."""

import errno
import os
import pathlib
import platform
import pty
import re
import select
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import xml.etree.ElementTree as ET


def check(binary):
    system = platform.system()
    machine = platform.machine()
    description = subprocess.check_output(["file", str(binary)], text=True)
    expected = "arm64" if system == "Darwin" else machine
    if expected == "x86_64":
        expected = "x86-64"
    assert expected in description, description
    assert binary.stat().st_mode & 0o777 == 0o755, "binary mode must be 0755"
    with tempfile.TemporaryDirectory(prefix="tinlynk-check-") as temporary:
        temporary = pathlib.Path(temporary)
        if system == "Darwin":
            subprocess.run(["codesign", "--verify", "--strict", str(binary)], check=True)
            load_commands = subprocess.check_output(["otool", "-l", str(binary)], text=True)
            assert "minos 26.0" in load_commands, load_commands
            subprocess.run(["otool", "-L", str(binary)], check=True)
            expanded = temporary / "expanded"
            subprocess.run(
                ["pkgutil", "--expand-full", str(binary) + ".pkg", str(expanded)],
                check=True,
            )
            distribution = ET.parse(expanded / "Distribution").getroot()
            assert distribution.find("options").get("hostArchitectures") == "arm64"
            assert distribution.find(".//os-version").get("min") == "26.0"
            payloads = list(expanded.glob("*/Payload/usr/local/bin/tinlynk-editor"))
            assert len(payloads) == 1, "pkg must install the executable in /usr/local/bin"
            payload_files = [path for path in expanded.glob("*/Payload/**/*") if path.is_file()]
            assert payload_files == payloads, "pkg contains unexpected files"
            assert payloads[0].read_bytes() == binary.read_bytes(), "pkg binary differs"
            assert payloads[0].stat().st_mode & 0o777 == 0o755, "pkg executable lost its mode"
            package_info = ET.parse(next(expanded.glob("*/PackageInfo"))).getroot()
            assert package_info.get("identifier") == "dev.tinlynk.terminal-editor"
            assert package_info.get("install-location") == "/"
            assert distribution.find("pkg-ref").get("version") == package_info.get("version")
        else:
            subprocess.run(["ldd", str(binary)], check=True)
            symbols = subprocess.check_output(["objdump", "-T", str(binary)], text=True)
            versions = re.findall(r"GLIBC_(\d+)\.(\d+)", symbols)
            assert versions, "no glibc symbol requirements found"
            assert max((int(a), int(b)) for a, b in versions) <= (2, 35), \
                "binary requires glibc newer than 2.35"
            with tarfile.open(str(binary) + ".tar.gz") as archive:
                members = archive.getmembers()
                assert len(members) == 1 and members[0].name == "tinlynk-editor"
                assert members[0].isfile() and members[0].mode == 0o755
                assert archive.extractfile(members[0]).read() == binary.read_bytes()
        smoke(binary, temporary)


def smoke(binary, directory):
    pid, master = pty.fork()
    if pid == 0:
        os.chdir(directory)
        os.environ["TERM"] = "xterm-256color"
        os.environ["HOME"] = str(directory)
        os.execl(str(binary), str(binary))
    output = bytearray()
    exited = False
    try:
        # Wait for rendered output, rather than sending quit before raw mode is active.
        deadline = time.monotonic() + 15
        quit_sent = False
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                output.extend(data)
                if b"\x1b[" in output and not quit_sent:
                    os.write(master, b"\x11")  # Ctrl+Q
                    quit_sent = True
            finished, status = os.waitpid(pid, os.WNOHANG)
            if finished:
                exited = True
                assert quit_sent and output, "editor exited before rendering"
                assert os.waitstatus_to_exitcode(status) == 0, output.decode(errors="replace")
                return
        raise AssertionError("packaged editor did not render and exit within 15 seconds")
    finally:
        if not exited:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(master)


if __name__ == "__main__":
    check(pathlib.Path(sys.argv[1]).resolve())
    print("Native binary, package/archive, and PTY smoke checks passed")
