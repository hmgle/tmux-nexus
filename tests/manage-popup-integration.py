"""Exercise and time pane switching through an isolated attached tmux client."""

import fcntl
import json
import math
import os
import pty
import select
import shlex
import statistics
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("TNX_BIN", ROOT / "target/debug/tnx")).resolve()
LAUNCHER = Path(os.environ.get("TNX_TEST_LAUNCHER", ROOT / "tnx")).resolve()
LAUNCH_MODE = os.environ.get("TNX_TEST_LAUNCH_MODE", "override")
if LAUNCH_MODE not in ("normal", "override"):
    raise ValueError("TNX_TEST_LAUNCH_MODE must be normal or override")
SAMPLES = int(os.environ.get("TNX_TEST_SAMPLES", "3"))
PANES = int(os.environ.get("TNX_TEST_PANES", "2"))
if SAMPLES < 1 or PANES < 2:
    raise ValueError("use at least one sample and two panes")
with tempfile.TemporaryDirectory(prefix="tnx-manager-ui-") as directory:
    tmp = Path(directory)
    socket = str(tmp / "tmux socket's.sock")
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in ("TMUX", "TMUX_PANE") and not k.startswith(("TNX_", "FZF_"))
    }
    env.update(
        TERM="xterm-256color",
        SHELL="/bin/sh",
        TNX_TMUX_ARGS=shlex.join(["-S", socket]),
        TMPDIR=directory,
    )
    tools = tmp / "bin"
    tools.mkdir()
    wrapper = tools / "fzf-tmux"
    wrapper.write_text("#!/bin/sh\necho unexpected-fzf-tmux >&2\nexit 99\n")
    wrapper.chmod(0o755)
    env["PATH"] = str(tools) + os.pathsep + env["PATH"]

    def tmux(*args):
        return subprocess.check_output(
            ["tmux", "-S", socket, *args], env=env, text=True, timeout=5
        ).rstrip("\n")

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 160, 0, 0))
    output = bytearray()
    attached = None

    def drain(seconds=0.02):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                output.extend(os.read(master, 65536))

    def wait(predicate, description):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if predicate():
                return
            drain(0.005)
        raise AssertionError(
            f"timeout: {description}\n{bytes(output)[-1500:]!r}\n{tmux('show-messages')}"
        )

    def attach_setup():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    try:
        origin = tmux(
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-x",
            "160",
            "-y",
            "44",
            "-s",
            "manager",
            "-P",
            "-F",
            "#{pane_id}",
            "printf '\\033[40;1HPREVIEW_MANAGER_ORIGIN'; sleep 120",
        )
        target = tmux(
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "printf '\\033[40;1HPREVIEW_MANAGER_TARGET'; sleep 120",
        )
        for _ in range(2, PANES):
            tmux("new-window", "-d", "sleep 120")
        tmux(
            "set-option", "-g", "@tmux_nexus_manager_fzf_options", "-p -w 86% -h 58% -m"
        )
        tmux(
            "set-option",
            "-g",
            "@tmux_nexus_manager_preview_width",
            os.environ.get("TNX_TEST_PREVIEW_WIDTH", "50%"),
        )
        tmux(
            "set-option",
            "-g",
            "@tmux_nexus_manager_pane_format",
            "#{b:pane_current_path} #{=/-26/...:#{d:pane_current_path}} [#{pane_current_command}]",
        )
        done = tmp / "done"
        errors = tmp / "errors"
        launch_env = ["env", "BASH_FUNC_tnx_test%%=() { :; }"]
        if LAUNCH_MODE == "override":
            launch_env.append(f"TNX_BIN={BIN}")

        def bind_picker(category, action):
            command = shlex.join(
                launch_env + [str(LAUNCHER), "manage", category, action]
            )
            tmux(
                "bind-key",
                "f",
                "run-shell",
                "-b",
                f"{command} 2>{shlex.quote(str(errors))}; touch {shlex.quote(str(done))}",
            )

        bind_picker("pane", "switch")
        attached = subprocess.Popen(
            ["tmux", "-S", socket, "attach-session", "-t", "manager"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            # Single-threaded PTY fixture: the child needs a controlling terminal.
            preexec_fn=attach_setup,  # noqa: PLW1509
        )
        drain(0.3)
        times = []
        for _ in range(SAMPLES):
            done.unlink(missing_ok=True)
            output.clear()
            start = time.monotonic()
            os.write(master, b"\x02f")
            wait(lambda: b"pane>" in output, "pane picker")
            times.append((time.monotonic() - start) * 1000)
            wait(lambda: b"PREVIEW_MANAGER_TARGET" in output, "pane preview")
            os.write(master, b"\x1b")
            wait(done.exists, "cancel completion")
            assert tmux("display-message", "-p", "#{pane_id}") == origin
            assert not errors.read_text(), errors.read_text()
            drain(0.05)
        # Enter must activate the same pane that the preview displays.
        done.unlink()
        output.clear()
        os.write(master, b"\x02f")
        wait(lambda: b"PREVIEW_MANAGER_TARGET" in output, "preview before selection")
        os.write(master, b"\r")
        wait(done.exists, "switch completion")
        assert tmux("display-message", "-p", "#{pane_id}") == target
        assert not errors.read_text(), errors.read_text()

        def check_preview(category, action, marker):
            bind_picker(category, action)
            done.unlink()
            output.clear()
            os.write(master, b"\x02f")
            wait(lambda: marker in output, category + " preview")
            os.write(master, b"\x1b")
            wait(done.exists, category + " cancel")
            assert not errors.read_text(), errors.read_text()
            assert tmux("display-message", "-p", "#{pane_id}") == target
            drain(0.05)

        check_preview("window", "switch", b"PREVIEW_MANAGER_ORIGIN")
        tmux(
            "new-session",
            "-d",
            "-s",
            "other-session",
            "printf '\\033[40;1HPREVIEW_MANAGER_SESSION'; sleep 120",
        )
        check_preview("session", "switch", b"PREVIEW_MANAGER_SESSION")
        injected = tmp / "injected"
        buffer_name = f"buffer 'quote' $(touch {injected})"
        tmux("set-buffer", "-b", buffer_name, "PREVIEW_MANAGER_BUFFER")
        check_preview("clipboard", "buffer", b"PREVIEW_MANAGER_BUFFER")
        assert not injected.exists(), (
            "preview interpreted the buffer name as shell code"
        )

        # Nearest-rank P95: for 20 samples, use the 19th sorted value.
        p95 = sorted(times)[math.ceil(len(times) * 0.95) - 1]
        result = {
            "mode": LAUNCH_MODE,
            "launcher": str(LAUNCHER),
            "override_binary": str(BIN) if LAUNCH_MODE == "override" else None,
            "panes": PANES,
            "sample_count": SAMPLES,
            "samples_ms": times,
            "first_ms": times[0],
            "median_ms": statistics.median(times),
            "p95_nearest_rank_ms": p95,
            "max_ms": max(times),
            "tmux_version": tmux("-V"),
            "fzf_version": subprocess.check_output(
                ["fzf", "--version"], env=env, text=True
            ).strip(),
        }
        if destination := os.environ.get("TNX_TEST_RESULTS"):
            Path(destination).write_text(json.dumps(result, indent=2) + "\n")
        print(
            f"{LAUNCH_MODE}: {PANES} panes, {SAMPLES} samples; "
            f"first {times[0]:.1f} ms; median {statistics.median(times):.1f} ms; "
            f"p95 {p95:.1f} ms; max {max(times):.1f} ms"
        )
    finally:
        # This absolute private socket is the only server we clean up.
        subprocess.run(
            ["tmux", "-S", socket, "kill-server"],
            check=False,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
        if attached:
            attached.wait(timeout=5)
        os.close(master)
        os.close(slave)
