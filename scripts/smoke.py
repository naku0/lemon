#!/usr/bin/env python3
"""Unix PTY integration checks, no hardware or third-party Python packages required.
Run after cargo build: python3 scripts/smoke.py
"""
import csv
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/debug/lemon"
ENV = {k: v for k, v in os.environ.items() if not k.startswith("LEMON_")}


def screen_text(output, width=160, height=42):
    """Reconstruct cursor-positioned terminal output; do not search ANSI diffs as plain text."""
    grid = [[" "] * width for _ in range(height)]
    x = y = 0
    for token in re.findall(r"\x1b\[[0-?]*[ -/]*[@-~]|[^\x1b]", output.decode(errors="replace")):
        if token.startswith("\x1b["):
            action = token[-1]
            raw = token[2:-1]
            if raw.startswith("?"):
                continue
            numbers = [int(n) if n else 0 for n in raw.split(";") if n.isdigit() or not n]
            n = numbers[0] if numbers else 0
            if action in ("H", "f"):
                y = max(0, (n or 1) - 1)
                x = max(0, (numbers[1] if len(numbers) > 1 else 1) - 1)
            elif action == "G":
                x = max(0, (n or 1) - 1)
            elif action == "A":
                y = max(0, y - (n or 1))
            elif action == "B":
                y = min(height - 1, y + (n or 1))
            elif action == "C":
                x = min(width - 1, x + (n or 1))
            elif action == "D":
                x = max(0, x - (n or 1))
            elif action == "J" and n in (2, 3):
                grid = [[" "] * width for _ in range(height)]
            elif action == "K" and y < height:
                begin, end = (x, width) if n == 0 else (0, x + 1) if n == 1 else (0, width)
                grid[y][begin:end] = [" "] * (end - begin)
        elif token == "\r":
            x = 0
        elif token == "\n":
            y = min(height - 1, y + 1)
        elif token.isprintable():
            if x >= width:
                x = 0
                y = min(height - 1, y + 1)
            if y < height:
                grid[y][x] = token
            x += 1
    return "\n".join("".join(row) for row in grid)


def wait_for(fd, output, text, timeout=6):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        drain(fd, output, 0.05)
        if text in screen_text(output):
            return
    (ROOT / "target/lemon-smoke-failure.txt").write_text(screen_text(output))
    raise AssertionError(f"screen did not show {text!r}; see target/lemon-smoke-failure.txt")


def drain(fd, output, timeout=0.02):
    if select.select([fd], [], [], timeout)[0]:
        try:
            output.extend(os.read(fd, 65536))
        except OSError:
            pass


def hold(fd, output, seconds):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        drain(fd, output)


def wait(proc, fd, output):
    until = time.monotonic() + 8
    while proc.poll() is None and time.monotonic() < until:
        drain(fd, output)
    if proc.poll() is None:
        proc.kill()
        raise AssertionError("process did not stop")
    drain(fd, output, 0)


def tui_run(config, exit_key, record):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 160, 0, 0))
    before = termios.tcgetattr(slave)

    def setup_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    prefs = config.parent / "settings.json"
    proc = subprocess.Popen(["/bin/sh", "-c", '"$1" demo --config "$2" --settings "$3"; rc=$?; printf "\\nAPP_EXIT=%s\\n" "$rc"; read answer; exit "$rc"', "lemon-smoke", str(BIN), str(config), str(prefs)], stdin=slave,
                            stdout=slave, stderr=slave, preexec_fn=setup_terminal,
                            env={**ENV, "TERM": "xterm-256color"})
    output = bytearray()
    try:
        wait_for(master, output, "Waveform")
        os.write(master, b"6")
        wait_for(master, output, "TUI FPS")
        os.write(master, b"\x1b[B" * 9 + b"\x1b[C")
        wait_for(master, output, "Light")
        os.write(master, b"s")
        wait_for(master, output, "Settings saved")
        assert json.loads(prefs.read_text())["appearance"]["theme"] == "Light"
        os.write(master, b"?")
        wait_for(master, output, "LEMON Help")
        os.write(master, b"\x1b")
        hold(master, output, 0.2)
        assert proc.poll() is None
        assert not termios.tcgetattr(slave)[3] & termios.ICANON, "closing Help must keep TUI in raw mode"
        os.write(master, b"\x12")  # Ctrl+R requires confirmation; cancel it
        wait_for(master, output, "Reset all settings")
        os.write(master, b"n2")
        hold(master, output, 0.2)
        if record:
            os.write(master, b"r")
            wait_for(master, output, "REC")
            hold(master, output, 2.3)
            os.write(master, b"mtest marker\r")
            wait_for(master, output, "Marker saved")
            os.write(master, b" ")  # visualization pause must not pause recording
            wait_for(master, output, "PAUSED")
            hold(master, output, 0.3)
            os.write(master, b"3")
            wait_for(master, output, "peak")
            os.write(master, b"4")
            wait_for(master, output, "Absolute history")
            os.write(master, b"5")
            wait_for(master, output, "Session name")
            os.write(master, b"1")
            wait_for(master, output, "Lightweight EEG")
            os.write(master, b"2vc ")
            hold(master, output, 0.2)
            wait_for(master, output, " | Line | ")
            for style in ("Points", "Envelope", "Line"):
                os.write(master, b"g")
                wait_for(master, output, f" | {style} | ")
        os.write(master, exit_key)
        if record:
            wait_for(master, output, "Confirm exit")
            os.write(master, b"n")
            hold(master, output, 0.2)
            assert proc.poll() is None
            os.write(master, exit_key)
            wait_for(master, output, "Confirm exit")
            os.write(master, b"y")
        until = time.monotonic() + 8
        while b"APP_EXIT=" not in output and time.monotonic() < until:
            drain(master, output)
        assert b"APP_EXIT=0" in output, output.decode(errors="replace")[-3000:]
        assert termios.tcgetattr(slave) == before, "terminal flags were not restored"
        os.write(master, b"\n")
        wait(proc, master, output)
        assert proc.returncode == 0, output.decode(errors="replace")[-3000:]
        assert b"\x1b[?1049l" in output, "alternate screen was not left"
        assert b"\x1b[?25h" in output, "cursor was not restored"
        assert b"Waveform" in output and b"Filtered" in output
        if record:
            (ROOT / "target/tui-smoke.ansi").write_bytes(output)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        os.close(master)
        os.close(slave)
    return output


def serial_run(config):
    master, slave = pty.openpty()
    port = os.ttyname(slave)
    proc = subprocess.Popen([str(BIN), "live", "--config", str(config), "--settings", str(config.parent / "settings.json"), "--headless",
                             "--record", "--port", port, "--channels", "1"],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV)
    try:
        time.sleep(0.2)
        for i in range(100):
            os.write(master, b"bad packet\n" if i == 20 else f"{i}\n".encode())
            time.sleep(0.004)
        os.close(master)
        master = None
        stdout, stderr = proc.communicate(timeout=8)
        assert proc.returncode == 1, (stdout, stderr)
        assert b"SourceError" in stderr, stderr
        assert b"samples=99" in stdout, stdout
        return stdout.decode()
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        if master is not None:
            os.close(master)
        os.close(slave)


def main():
    with tempfile.TemporaryDirectory(prefix="eeg-smoke-") as tmp:
        tmp = Path(tmp)
        config = json.loads((ROOT / "examples/synthetic.json").read_text())
        config["recording_dir"] = str(tmp / "recordings")
        path = tmp / "config.json"
        path.write_text(json.dumps(config))
        output = tui_run(path, b"q", True)
        recordings = list((tmp / "recordings").iterdir())
        assert len(recordings) == 1
        session = recordings[0]
        with (session / "raw.csv").open() as f:
            rows = list(csv.DictReader(f))
        assert len(rows) >= 625, len(rows)
        with (session / "events.csv").open() as f:
            events = list(csv.DictReader(f))
        assert any(e["kind"] == "Marker" and e["text"] == "test marker" for e in events)
        replay = subprocess.run([str(BIN), "replay", str(session), "--headless", "--speed", "20", "--settings", str(tmp / "settings.json")],
                                capture_output=True, timeout=8, env=ENV)
        assert replay.returncode == 0, replay.stderr
        assert f"samples={len(rows) * 2}".encode() in replay.stdout, replay.stdout
        print(f"PASS LEMON: Settings/theme/save, Help/Esc, reset cancel, six screens, band history, marker, pause, exit confirmation; {len(rows)} raw rows; replay exact sample count")
        (tmp / "settings.json").unlink()  # each run starts with the documented default theme
        tui_run(path, b"\x03", False)
        print("PASS TUI Ctrl+C: raw mode, alternate screen and cursor restored")
        proc = subprocess.Popen([str(BIN), "demo", "--headless", "--config", str(path), "--settings", str(tmp / "settings.json"), "--record"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV)
        time.sleep(0.3)
        proc.send_signal(signal.SIGINT)
        stdout, stderr = proc.communicate(timeout=8)
        assert proc.returncode == 0, (stdout, stderr)
        print("PASS headless SIGINT: clean exit and flushed recording")
        if sys.platform == "darwin":
            print("NOTE macOS PTYs reject serial baud ioctls; transport/dual-port/disconnect are checked by Rust TTYPort::pair tests")
            return
        serial_run(path)
        sessions = list((tmp / "recordings").iterdir())
        serial_events = []
        for item in sessions:
            if json.loads((item / "metadata.json").read_text())["config"]["source"]["mode"] == "serial-test":
                with (item / "events.csv").open() as f:
                    serial_events = list(csv.DictReader(f))
        assert any(e["kind"] == "InvalidPacket" for e in serial_events)
        assert any(e["kind"] == "SourceError" for e in serial_events)
        print("PASS serial-test PTY: valid packets, malformed packet recovery, disconnect diagnostics, raw flush")


if __name__ == "__main__":
    main()
