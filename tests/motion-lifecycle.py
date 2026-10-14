"""Check hint-window ownership, first-frame readiness and abnormal exits."""

import fcntl
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("TNX_BIN", ROOT / "target/debug/tnx")).resolve()
TMUX = shutil.which("tmux")


class UI:
    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory(prefix="tnx-motion-life-")
        self.tmp = Path(self.directory.name)
        self.socket = str(self.tmp / "tmux.sock")
        self.env = {
            k: v for k, v in os.environ.items()
            if k not in ("TMUX", "TMUX_PANE") and not k.startswith("TNX_")
        }
        self.env.update(
            TERM="xterm-256color", SHELL="/bin/sh", TMPDIR=str(self.tmp),
            TNX_TMUX_ARGS=shlex.join(["-S", self.socket]),
        )
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
        self.output = bytearray()
        self.attached = self.wrapper = None
        self.child_pid = None
        self.pane = self.tmux(
            "-f", "/dev/null", "new-session", "-d", "-s", "test", "-x", "100", "-y", "24",
            "-P", "-F", "#{pane_id}",
            "printf 'origin\\thttps://example.test/repo (fetch)\\n"
            "origin\\thttps://example.test/repo (push)\\n'; exec sleep 120",
        )
        self.source = self.tmux("display", "-p", "-t", self.pane, "#{window_id}")
        self.other = self.tmux(
            "new-window", "-d", "-t", "test:5", "-P", "-F", "#{window_id}", "sleep 120",
        )
        self.tmux("set", "-g", "renumber-windows", "on")
        self.tmux("set", "-g", "allow-set-title", "off")
        self.tmux("set", "-sg", "escape-time", "0")
        self.attach()
        # Use a shell that fails if invoked: hint creation must exec tnx
        # directly, including when default-shell isn't POSIX compatible.
        shell = self.tmp / "bad-shell"
        shell.write_text("#!/bin/sh\nprintf invoked > " + shlex.quote(str(self.tmp / "shell-used")) + "\nexit 99\n")
        shell.chmod(0o755)
        self.tmux("set", "-g", "default-shell", str(shell))
        return self

    def tmux(self, *args):
        return subprocess.check_output(
            [TMUX, "-S", self.socket, *args], env=self.env, text=True, timeout=5,
        ).rstrip("\n")

    def drain(self, seconds=0.01):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            if select.select([self.master], [], [], max(0, until - time.monotonic()))[0]:
                self.output.extend(os.read(self.master, 65536))

    def wait(self, predicate, description, timeout=4):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            self.drain()
            if predicate():
                return
        raise AssertionError(f"timed out: {description}\n{self.tmux('show-messages')}")

    def attach(self):
        def setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        self.attached = subprocess.Popen(
            [TMUX, "-S", self.socket, "attach", "-t", "test"], env=self.env,
            stdin=self.slave, stdout=self.slave, stderr=self.slave, preexec_fn=setup,
        )
        self.wait(lambda: self.tmux("list-clients", "-F", "#{client_name}"), "attach")
        self.client = self.tmux("list-clients", "-F", "#{client_name}")

    def launch(self, fault=None):
        if fault:
            shim = self.tmp / "bin"
            shim.mkdir()
            script = shim / "tmux"
            # Fault injection affects only the tnx invocation of respawn-pane.
            script.write_text(
                f"#!{sys.executable}\nimport os, sys\nfrom pathlib import Path\n"
                "args = sys.argv[1:]\n"
                f"if 'respawn-pane' in args and {str(BIN)!r} in args:\n"
                f"    if {fault!r} == 'bad-snapshot':\n"
                "        Path(args[args.index('--snapshot') + 1]).write_text('{')\n"
                "    else:\n"
                f"        args[args.index({str(BIN)!r}):] = ['/bin/sleep', '30']\n"
                f"os.execv({TMUX!r}, [{TMUX!r}, *args])\n"
            )
            script.chmod(0o755)
            self.env["PATH"] = str(shim) + os.pathsep + self.env["PATH"]
        self.wrapper = subprocess.Popen(
            [str(BIN), "motion", "s", "--query", "h", "--target-window", self.source,
             "--target-client", self.client],
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.wait(lambda: len(self.tmux("list-windows", "-F", "#{window_id}").splitlines()) == 3,
                  "hint window creation")
        self.overlay, self.overlay_pane = next(
            line.split("\t") for line in self.tmux(
                "list-panes", "-a", "-F", "#{window_id}\t#{pane_id}",
            ).splitlines() if line.split("\t")[0] not in (self.source, self.other)
        )
        # Observe the title and grid in the same tmux queue as the selection
        # check: the first visible frame must already contain hints.
        if not fault:
            def visible():
                result = self.tmux(
                    "display", "-p", "-c", self.client, "#{window_id}\t#{pane_title}",
                    ";", "capture-pane", "-p", "-t", self.overlay_pane,
                )
                metadata, _, grid = result.partition("\n")
                if metadata.split("\t")[0] != self.overlay:
                    return False
                assert "tnx-motion-ready-" in metadata, "selected a window before its frame was parsed"
                assert "attps://" in grid or "sttps://" in grid, "selected a blank or unhinted frame"
                return True
            self.wait(visible, "prepared hint window")
        if fault != "bad-snapshot":
            self.wait(lambda: self.tmux("display", "-p", "-t", self.overlay_pane, "#{pane_pid}"),
                      "overlay process creation")
            self.child_pid = int(self.tmux("display", "-p", "-t", self.overlay_pane, "#{pane_pid}"))

    def finished(self, expected_window=None, error=None, timeout=4, renumbered=False):
        self.wait(lambda: self.wrapper.poll() is not None, "wrapper exit", timeout)
        _, stderr = self.wrapper.communicate()
        if error:
            assert self.wrapper.returncode != 0
            assert error in stderr.decode(), stderr
            assert "display-message -c" in self.tmux("show-messages"), "error wasn't shown to client"
        else:
            assert self.wrapper.returncode == 0, stderr
        assert not list(self.tmp.glob("tnx_motion*")), "motion temporary files leaked"
        assert not (self.tmp / "shell-used").exists(), "default-shell was invoked"
        assert self.tmux("list-windows", "-F", "#{window_id}") == f"{self.source}\n{self.other}"
        if not renumbered:
            assert self.tmux("list-windows", "-F", "#{window_index}") == "0\n5", "window indices changed"
        if expected_window:
            assert self.tmux("display", "-p", "#{window_id}") == expected_window, "focus stolen"
        if self.child_pid:
            def gone():
                try:
                    os.kill(self.child_pid, 0)
                    return False
                except ProcessLookupError:
                    return True
            self.wait(gone, "overlay process exit")

    def __exit__(self, *_):
        subprocess.run([TMUX, "-S", self.socket, "kill-server"], env=self.env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        for process in (self.wrapper, self.attached):
            if process is not None:
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
        os.close(self.master)
        os.close(self.slave)
        self.directory.cleanup()


def check_lifecycle():
    for action in ("cancel", "kill-window", "kill-pane", "sigkill", "detach", "switch",
                   "bad-snapshot", "startup-timeout", "detach-starting"):
        with UI() as ui:
            fault = action if action in ("bad-snapshot", "startup-timeout", "detach-starting") else None
            ui.launch(fault)
            if action == "cancel":
                ui.tmux("send-keys", "-t", ui.overlay_pane, "Escape")
            elif action in ("kill-window", "kill-pane"):
                ui.tmux(action, "-t", ui.overlay_pane)
            elif action == "sigkill":
                os.kill(ui.child_pid, signal.SIGKILL)
            elif action in ("detach", "detach-starting"):
                ui.tmux("detach-client", "-t", ui.client)
                ui.attached.wait(timeout=3)
            elif action == "switch":
                ui.tmux("select-window", "-t", ui.other)
            expected = ui.other if action == "switch" else ui.source
            error = {"sigkill": "signal", "bad-snapshot": "failed to parse", "startup-timeout": "startup timed out"}.get(action)
            ui.finished(expected, error, timeout=8, renumbered=action in ("kill-window", "kill-pane"))
            if action in ("detach", "detach-starting"):
                ui.attach()
                assert ui.tmux("display", "-p", "#{window_id}") == ui.source
        print(f"motion lifecycle: {action}: passed", flush=True)


if __name__ == "__main__":
    check_lifecycle()
