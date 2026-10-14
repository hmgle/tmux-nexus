"""Exercise motion through an attached tmux client and a real terminal."""

import fcntl
import os
from pathlib import Path
import pty
import re
import select
import shlex
import struct
import subprocess
import tempfile
import termios
import time


ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("TNX_BIN", ROOT / "target/debug/tnx"))


def check_motion():
    assert BIN.is_file(), "build tnx before running motion integration tests"
    with tempfile.TemporaryDirectory(prefix="tnx-motion-ui-") as directory:
        tmp = Path(directory)
        socket = str(tmp / "tmux.sock")
        env = {
            key: value for key, value in os.environ.items()
            if key not in ("TMUX", "TMUX_PANE") and not key.startswith("TNX_")
        }
        env.update({
            "TERM": "xterm-256color",
            "TNX_TMUX_ARGS": shlex.join(["-S", socket]),
            "TNX_BIN": str(BIN),
            "TMPDIR": directory,
            "INPUTRC": "/dev/null",
        })

        def tmux(*args):
            return subprocess.check_output(
                ["tmux", "-S", socket, *args], env=env, text=True, timeout=5,
            ).removesuffix("\n")

        fixture = tmp / "shell.bash"
        # Padding resembles a shell prompt's rendered grid and exposes
        # reflow when a zoomed pane temporarily returns to its tiled size.
        fixture.write_text(
            "PS1='                                                          prompt\\n> '\n"
            "git() { printf 'origin\\thttps://example.test/repo (fetch)\\n"
            "origin\\thttps://example.test/repo (push)\\n'; }\n"
        )
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 160, 0, 0))
        attached = None
        terminal_output = bytearray()

        def drain(seconds=0.1):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                    terminal_output.extend(os.read(master, 65536))

        def wait_until(predicate, description):
            for _ in range(60):
                drain(0.05)
                if predicate():
                    return
            raise AssertionError(
                f"timed out waiting for {description}\n"
                + tmux("capture-pane", "-p") + "\n"
                + tmux("show-messages")
            )

        def attach_setup():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        try:
            pane = tmux(
                "-f", "/dev/null", "new-session", "-d", "-x", "160", "-y", "44",
                "-P", "-F", "#{pane_id}",
                shlex.join(["bash", "--noprofile", "--rcfile", str(fixture), "-i"]),
            )
            window = tmux("display-message", "-p", "-t", pane, "#{window_id}")
            capabilities = subprocess.run(
                ["tmux", "-S", socket, "list-commands", "new-pane"], env=env,
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False, text=True,
            )
            window_backend = capabilities.returncode == 0 and capabilities.stdout.startswith("new-pane ")
            backend = "window" if window_backend else "popup"
            assert os.environ.get("TNX_TEST_MOTION_BACKEND", backend) == backend, "wrong motion backend"
            tmux("set-hook", "-w", "-t", window, "window-layout-changed",
                 "set-option -g @tnx_test_layout_changed 1")
            tmux("set-window-option", "-t", pane, "mode-keys", "vi")
            tmux("set-window-option", "-t", pane, "scroll-on-clear", "on")
            tmux("split-window", "-v", "-d", "-t", pane, "sleep 60")
            right = tmux(
                "split-window", "-h", "-d", "-t", pane, "-P", "-F", "#{pane_id}", "sleep 60",
            )
            tmux("split-window", "-v", "-d", "-t", right, "sleep 60")
            for key, script in [("s", "motion-s.sh"), ("S", "motion-s2.sh")]:
                tmux(
                    "bind-key", key, "run-shell",
                    shlex.quote(str(ROOT / "scripts" / script))
                    + " #{client_pid} #{window_id} #{q:client_name}",
                )
            attached = subprocess.Popen(
                ["tmux", "-S", socket, "attach-session"],
                stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=attach_setup,
            )
            drain(0.4)

            def grid():
                return tmux("capture-pane", "-p", "-t", pane)

            def metadata():
                return tmux(
                    "display-message", "-p", "-t", pane,
                    "#{history_size},#{cursor_x},#{cursor_y},#{pane_width},#{pane_height},#{window_zoomed_flag}",
                )

            for zoom in (False, True):
                if zoom:
                    tmux("resize-pane", "-Z", "-t", pane)
                for key, query in [(b"s", b"h"), (b"S", b"ht")]:
                    for choice in ("fetch", "push", "cancel"):
                        os.write(master, b"\x0c")
                        drain()
                        os.write(master, b"git remote -v\r")
                        wait_until(lambda: "(push)" in grid(), "git output")
                        drain()
                        before_grid, before_metadata = grid(), metadata()
                        rows = before_grid.split("\n")
                        expected_row = next(
                            row for row, line in enumerate(rows) if f"({choice})" in line
                        ) if choice != "cancel" else None

                        terminal_output.clear()
                        tmux("set-option", "-g", "@tnx_test_layout_changed", "0")
                        os.write(master, b"\x02" + key)
                        prompt = b"motion:" if key == b"s" else b"motion 1/2:"
                        wait_until(lambda: prompt in terminal_output, "motion prompt")
                        for index, character in enumerate(query):
                            terminal_output.clear()
                            os.write(master, bytes([character]))
                            if index + 1 < len(query):
                                wait_until(
                                    lambda: b"motion 2/2:" in terminal_output,
                                    "second motion prompt",
                                )
                        wait_until(lambda: list(tmp.glob("tnx_motion.*.json")), "motion snapshot")
                        wait_until(
                            lambda: re.search(rb"\x1b\[(?:[0-9]+;)*31m", terminal_output),
                            "red hint labels",
                        )
                        if window_backend:
                            assert tmux("display-message", "-p", "#{window_id}") != window
                            assert tmux("display-message", "-p", "#{pane_title}").startswith("tnx-motion-ready-")
                        else:
                            assert tmux("list-windows", "-F", "#{window_id}") == window
                        drain()
                        assert tmux("show-option", "-gqv", "@tnx_test_layout_changed") == "0", "opening hints changed the source layout"
                        assert grid() == before_grid, "opening hints changed the source grid"
                        assert metadata() == before_metadata, "opening hints resized or scrolled the source"

                        os.write(master, {"fetch": b"s", "push": b"a", "cancel": b"\x1b"}[choice])
                        if choice != "cancel":
                            wait_until(
                                lambda: tmux("display-message", "-p", "-t", pane, "#{pane_in_mode}") == "1",
                                "copy mode after selection",
                            )
                            actual = tmux("display-message", "-p", "-t", pane, "#{copy_cursor_x},#{copy_cursor_y}")
                            assert actual == f"8,{expected_row}", (actual, expected_row)
                            os.write(master, b"\r")
                        wait_until(
                            lambda: tmux("display-message", "-p", "-t", pane, "#{pane_in_mode}") == "0"
                            and not list(tmp.glob("tnx_motion.*.json")),
                            "motion cleanup",
                        )
                        drain()
                        assert grid() == before_grid, "leaving motion changed the source grid"
                        assert metadata() == before_metadata, "leaving motion resized or scrolled the source"
                        assert tmux("display-message", "-p", "#{window_id}") == window
                        assert tmux("list-windows", "-F", "#{window_id}") == window, "overlay window leaked"
                        assert tmux("show-option", "-gqv", "@tnx_test_layout_changed") == "0", "closing hints changed the source layout"
                        print(f"motion ({backend}): zoom={zoom}, query={query.decode()}, {choice}: passed")
        finally:
            subprocess.run(
                ["tmux", "-S", socket, "kill-server"], env=env,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5,
            )
            if attached is not None:
                attached.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    check_motion()
