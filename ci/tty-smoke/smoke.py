#!/usr/bin/env python3
"""Drives the real tuiba TUI inside a pseudo-terminal and checks what it draws.

The headless runs prove the emulator works everywhere; this proves the
terminal side does: raw mode, the alternate screen, key input without the
Kitty keyboard protocol, half-block drawing, the library's config paths and
a clean exit. On Linux and macOS it uses a POSIX pty, on Windows a ConPTY
(through `pywinpty`), which is what Windows Terminal itself sits on.

    python3 ci/tty-smoke/smoke.py path/to/tuiba

The cartridge is built here: a mode-3 loop that fills the screen red, or
green while A is held. So "the screen turned green" means a key went from
the pty through crossterm and the key bindings into the emulated keypad.
"""

import os
import re
import shutil
import sys
import tempfile
import threading
import time

COLS, ROWS = 100, 40
TITLE = b"TTYSMOKE"

# arm-none-eabi-as -mcpu=arm7tdmi, entry `b main` at 0 and main at 0xC0:
#     mov   r0, #0x04000000
#     ldr   r1, =0x0403          @ DISPCNT: mode 3, BG2
#     strh  r1, [r0]
# frame:
#     ldr   r3, =0x04000130
#     ldrh  r3, [r3]             @ KEYINPUT, active low
#     tst   r3, #1               @ A
#     ldreq r1, =0x03E003E0      @ green
#     ldrne r1, =0x001F001F      @ red
#     mov   r0, #0x06000000
#     ldr   r2, =19200
# fill:
#     str   r1, [r0], #4
#     subs  r2, r2, #1
#     bne   fill
#     b     frame
PROGRAM = bytes.fromhex(
    "0103a0e32c109fe5b010c0e128309fe5b030d3e1010013e320109f0520109f15"
    "0604a0e34b2ca0e3041080e4012052e2fcffff1af4ffffea0304000030010004"
    "e003e0031f001f00"
)


def cartridge():
    rom = bytearray(0xC0)
    rom[0:4] = (0xEA00002E).to_bytes(4, "little")  # b 0xC0
    rom[0xA0:0xAC] = TITLE.ljust(12)
    return bytes(rom) + PROGRAM


# Truecolor SGR parameters, foreground or background, alone or combined.
RGB = re.compile(rb"[34]8;2;(\d+);(\d+);(\d+)")


def colours(data):
    return [tuple(map(int, m.groups())) for m in RGB.finditer(data)]


def is_red(c):
    return c[0] >= 200 and c[1] <= 16 and c[2] <= 16


def is_green(c):
    return c[1] >= 200 and c[0] <= 16 and c[2] <= 16


class PosixTerm:
    def __init__(self, argv, env):
        import fcntl
        import pty
        import struct
        import termios

        pid, fd = pty.fork()
        if pid == 0:
            size = struct.pack("HHHH", ROWS, COLS, COLS * 8, ROWS * 16)
            fcntl.ioctl(0, termios.TIOCSWINSZ, size)
            os.execve(argv[0], argv, env)
        self.pid, self.fd = pid, fd

    def read(self):
        try:
            return os.read(self.fd, 65536)
        except OSError:
            return b""

    def write(self, data):
        os.write(self.fd, data)

    def wait(self):
        _, status = os.waitpid(self.pid, 0)
        return os.waitstatus_to_exitcode(status)

    def kill(self):
        try:
            os.kill(self.pid, 9)
        except OSError:
            pass


class ConPty:
    def __init__(self, argv, env):
        from winpty import PtyProcess

        self.proc = PtyProcess.spawn(argv, dimensions=(ROWS, COLS), env=env)

    def read(self):
        try:
            return self.proc.read(65536).encode("utf-8", "surrogateescape")
        except EOFError:
            return b""

    def write(self, data):
        self.proc.write(data.decode("latin-1"))

    def wait(self):
        while self.proc.isalive():
            time.sleep(0.05)
        return self.proc.exitstatus

    def kill(self):
        self.proc.terminate(force=True)


class Session:
    """A running tuiba plus everything it has written so far."""

    def __init__(self, argv, env):
        self.term = (ConPty if os.name == "nt" else PosixTerm)(argv, env)
        self.output = bytearray()
        self.lock = threading.Lock()
        self.done = False
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            data = self.term.read()
            if not data:
                self.done = True
                return
            with self.lock:
                self.output += data
            # Answer crossterm's primary device attributes query, which
            # follows its keyboard-enhancement query. Leaving `?u`
            # unanswered is what a terminal without the protocol does.
            if b"\x1b[c" in data:
                self.term.write(b"\x1b[?62;22c")
            if b"\x1b[6n" in data:
                self.term.write(b"\x1b[1;1R")

    def mark(self):
        with self.lock:
            return len(self.output)

    def since(self, mark):
        with self.lock:
            return bytes(self.output[mark:])

    def wait(self, test, mark, timeout):
        """Whether the output since `mark` passes `test` within `timeout`."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if test(self.since(mark)):
                return True
            if self.done:
                break
            time.sleep(0.05)
        return False

    def expect(self, what, test, mark, timeout=10.0):
        if self.wait(test, mark, timeout):
            print(f"ok: {what}")
            return
        tail = self.since(mark)[-1500:]
        self.term.kill()
        sys.exit(f"FAILED: {what}\nlast output: {tail!r}")

    def exited(self, what, timeout=10.0):
        deadline = time.monotonic() + timeout
        while not self.done and time.monotonic() < deadline:
            time.sleep(0.05)
        if not self.done:
            tail = self.since(0)[-1500:]
            self.term.kill()
            sys.exit(f"FAILED: {what}: still running\nlast output: {tail!r}")
        code = self.term.wait()
        if code != 0:
            sys.exit(f"FAILED: {what}: exit status {code}")
        print(f"ok: {what}, status 0")

    def send(self, keys):
        self.term.write(keys)


# Windows virtual-key code, scan code and character of the keys we press.
WIN32_KEYS = {
    b"\r": (0x0D, 0x1C, 13),
    b"\x1b": (0x1B, 0x01, 27),
    b"q": (0x51, 0x10, ord("q")),
    b"y": (0x59, 0x15, ord("y")),
    b"z": (0x5A, 0x2C, ord("z")),
}


def win32_key(key, down):
    """A key-down or key-up in win32-input-mode, the way Windows Terminal
    hands keys to a ConPTY. Plain text through a ConPTY becomes a key-down
    and key-up in the same instant, and a lone `\x1b` is ambiguous."""
    vk, sc, ch = WIN32_KEYS[key]
    return f"\x1b[{vk};{sc};{ch};{int(down)};0;1_".encode()


def press(s, key):
    """Types `key` once: a down and an up."""
    if os.name == "nt":
        s.send(win32_key(key, True) + win32_key(key, False))
    else:
        s.send(key)


def environment(root):
    env = {
        k: v
        for k, v in os.environ.items()
        # Whatever terminal runs this script must not leak into the one we fake.
        if not k.startswith(("KITTY_", "WEZTERM_", "XDG_", "TERM", "LC_TERMINAL"))
        and k not in ("WT_SESSION", "TMUX", "STY", "COLORTERM")
    }
    if os.name == "nt":
        # The Windows paths, not XDG: %APPDATA% and %LOCALAPPDATA%.
        env["APPDATA"] = os.path.join(root, "Roaming")
        env["LOCALAPPDATA"] = os.path.join(root, "Local")
        config = os.path.join(env["APPDATA"], "tuiba")
    else:
        env["TERM"] = "xterm-256color"
        env["XDG_CONFIG_HOME"] = os.path.join(root, "config")
        env["XDG_STATE_HOME"] = os.path.join(root, "state")
        config = os.path.join(env["XDG_CONFIG_HOME"], "tuiba")
    return env, config


def library_run(binary, root):
    """Library → game → A held → back to the library → q, y."""
    env, config = environment(root)
    # A space in the folder name, as Windows users' paths often have.
    games = os.path.join(root, "my games")
    os.makedirs(games)
    os.makedirs(config)
    with open(os.path.join(games, "smoke.gba"), "wb") as f:
        f.write(cartridge())
    with open(os.path.join(config, "library"), "w") as f:
        f.write(games + "\n")

    s = Session([binary, "--renderer", "blocks", "--mute"], env)
    s.expect("library lists the cartridge", lambda d: TITLE in d, 0)

    m = s.mark()
    time.sleep(0.5)
    press(s, b"\r")
    s.expect("game screen drawn red", lambda d: any(map(is_red, colours(d))), m)

    m = s.mark()
    if os.name == "nt":
        # Real key-up events: hold Z down, then let go.
        s.send(win32_key(b"z", True))
        s.expect("A (z) held turns the screen green", lambda d: any(map(is_green, colours(d))), m)
        time.sleep(1.0)
        s.send(win32_key(b"z", False))
    else:
        # Without release events a key counts as held for a while after
        # each press; keep pressing, like a key's auto-repeat would.
        for _ in range(10):
            s.send(b"z")
            time.sleep(0.1)
        s.expect("A (z) turns the screen green", lambda d: any(map(is_green, colours(d))), m)
    m = s.mark()
    s.expect("letting go turns it red again", lambda d: any(map(is_red, colours(d))), m)

    m = s.mark()
    # A second apart: well past the gap that tells two presses from one
    # press reported twice, well inside the window the first one opens.
    # A stalled runner can still hand tuiba both at once, which it rightly
    # takes for a single press, so keep pressing until the library shows.
    press(s, b"\x1b")
    for _ in range(4):
        if s.wait(lambda d: TITLE in d, m, 1.0):
            break
        press(s, b"\x1b")
    s.expect("Esc twice returns to the library", lambda d: TITLE in d, m)

    # Past the cooldown that keeps a held Esc from reaching the prompt.
    time.sleep(1.0)
    m = s.mark()
    press(s, b"q")
    s.expect("q asks before quitting", lambda d: b"Quit" in d, m)
    press(s, b"y")
    s.exited("y quits")

    if os.name != "nt":
        # The pty passes what tuiba wrote straight through.
        out = s.since(0)
        if b"\x1b[?1049l" not in out:
            sys.exit("FAILED: alternate screen not left")
        print("ok: terminal restored")
    recent = os.path.join(config, "recent")
    if not os.path.isfile(recent) or "smoke.gba" not in open(recent).read():
        sys.exit(f"FAILED: {recent} does not record the game")
    print(f"ok: recent list written to {recent}")


def kitty_run(binary, root):
    """The Kitty renderer's transport: a file in /dev/shm where there is one
    (Linux), the pixels inline everywhere else (macOS)."""
    env, _ = environment(root)
    env["TERM"] = "xterm-kitty"
    os.makedirs(root)
    rom = os.path.join(root, "kitty.gba")
    with open(rom, "wb") as f:
        f.write(cartridge())
    shm = os.path.isdir("/dev/shm")
    want = b",t=t;" if shm else b",t=d,"

    s = Session([binary, rom, "--mute"], env)
    s.expect(
        f"kitty image sent with {want.decode()!r}",
        lambda d: b"\x1b_G" in d and want in d,
        0,
    )
    s.send(b"\x11")
    s.exited("Ctrl+Q quits from the game")
    if shm:
        left = [f for f in os.listdir("/dev/shm") if f.startswith("tuiba-")]
        if left:
            sys.exit(f"FAILED: frame files left in /dev/shm: {left}")
        print("ok: /dev/shm cleaned up")


def main():
    binary = os.path.abspath(sys.argv[1])
    root = tempfile.mkdtemp(prefix="tuiba-tty-")
    try:
        library_run(binary, os.path.join(root, "library"))
        if os.name != "nt":
            kitty_run(binary, os.path.join(root, "kitty"))
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print("all good")


if __name__ == "__main__":
    main()
