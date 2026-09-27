"""Measure editor-internal key latency through a pty (no terminal emulator involved).

Usage: python3 bench/latency.py FILE [--startup] [--idle SECONDS] [vim] [nvim] [hx] [emacs] [nib]

--startup measures only startup, for editors that cannot edit yet.
--idle also measures CPU use and wakeups while the editor waits for keys.
BENCH_VIM, BENCH_NVIM, BENCH_HX, and BENCH_EMACS name the programs to run
instead of those on PATH, as launchers that do not exec would be measured
too.
nib is run from target/release, so build it with `cargo build --release`.

Memory and idle CPU come from proc_pid_rusage, so only on macOS.
docs/benchmarks.md lists the files and the results.
"""
import ctypes, fcntl, os, pty, re, select, statistics, struct, subprocess, sys, termios, time

ROWS, COLS = 50, 160
REPLIES = {
    b"\x1b[c": b"\x1b[?62;22c",
    b"\x1b[0c": b"\x1b[?62;22c",
    b"\x1b[>c": b"\x1b[>0;100;0c",
    b"\x1b[>0c": b"\x1b[>0;100;0c",
    b"\x1b[5n": b"\x1b[0n",
    b"\x1b[>0q": b"\x1bP>|xterm(390)\x1b\\",
    b"\x1b[>q": b"\x1bP>|xterm(390)\x1b\\",
    b"\x1b[6n": b"\x1b[1;1R",
    b"\x1b]11;?": b"\x1b]11;rgb:0000/0000/0000\x1b\\",
    b"\x1b]10;?": b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\",
}


def spawn(argv, env=None):
    pid, fd = pty.fork()
    if pid == 0:
        # Set the size before exec, so the editor never sees a 0x0 terminal.
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.environ["LANG"] = "en_US.UTF-8"
        os.environ.update(env or {})
        os.execvp(argv[0], argv)
    return pid, fd


def answer(fd, data):
    """Answers queries in the order they came, as a terminal does. Editors
    such as Neovim send a primary DA last and take its answer to mean the
    queries before it went unanswered."""
    found = []
    for q, r in REPLIES.items():
        at = data.find(q)
        while at >= 0:
            found.append((at, r))
            at = data.find(q, at + 1)
    for _, r in sorted(found):
        os.write(fd, r)


# Setting modes (`ESC[?1004h`) and hiding or showing the cursor, which
# draw nothing: Neovim turns on focus events some 70 ms after drawing.
MODES = re.compile(rb"\x1b\[\?[0-9;]*[hl]")


def draws(data):
    return bool(MODES.sub(b"", data))


def read_burst(fd, first_timeout, silence):
    """Return (t_first, t_last) relative to call, reading until `silence` s
    of no output. Output that draws nothing does not count."""
    t0 = time.perf_counter()
    t_first = t_last = None
    timeout = first_timeout
    while True:
        r, _, _ = select.select([fd], [], [], timeout)
        if not r:
            return t_first, t_last
        try:
            data = os.read(fd, 65536)
        except OSError:
            data = b""
        if not data:
            # The editor exited.
            return t_first, t_last
        now = time.perf_counter() - t0
        answer(fd, data)
        if not draws(data):
            continue
        if t_first is None:
            t_first = now
        t_last = now
        timeout = silence


class RusageInfo(ctypes.Structure):
    """rusage_info_v0 from <libproc.h>."""
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [(n, ctypes.c_uint64) for n in (
        "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
        "wired_size", "resident_size", "phys_footprint", "proc_start_abstime", "proc_exit_abstime")]


def tree(pid):
    """`pid` and its descendants: Neovim draws in one process and edits in
    a child, and launchers may start the editor without exec."""
    pids = [pid]
    for p in pids:
        found = subprocess.run(["pgrep", "-P", str(p)], capture_output=True, text=True)
        pids.extend(int(c) for c in found.stdout.split())
    return pids


def usage(pid):
    """(CPU seconds, wakeups, memory footprint in MB) of `pid` and its
    descendants, or None off macOS. Language servers the editor starts
    count too, so the benchmarks turn LSP off."""
    if sys.platform != "darwin":
        return None
    libc = ctypes.CDLL("libSystem.dylib")
    # The times are in mach ticks, which are not nanoseconds on Apple silicon.
    timebase = (ctypes.c_uint32 * 2)()
    libc.mach_timebase_info(timebase)
    cpu = wakeups = memory = 0
    for p in tree(pid):
        info = RusageInfo()
        if libc.proc_pid_rusage(p, 0, ctypes.byref(info)) != 0:
            continue
        ticks = (info.user_time + info.system_time) * timebase[0] / timebase[1]
        cpu += ticks / 1e9
        wakeups += info.pkg_idle_wkups + info.interrupt_wkups
        memory += info.phys_footprint / 2**20
    return cpu, wakeups, memory


def kill(pid, fd):
    """Kills the editor and what it started, which would keep running."""
    for p in reversed(tree(pid)):
        try:
            os.kill(p, 9)
        except ProcessLookupError:
            pass
    os.waitpid(pid, 0)
    os.close(fd)


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))]


def run(editor, startup_only, idle, n=300):
    name, argv, keys, env = editor.name, editor.argv, editor.keys, editor.env
    # startup: spawn -> the output settles, including redraws that color
    # the screen after the first frame
    starts, memory = [], []
    for _ in range(10):
        pid, fd = spawn(argv, env)
        _, last = read_burst(fd, 5.0, 0.3)
        starts.append(last)
        if u := usage(pid):
            memory.append(u[2])
        kill(pid, fd)

    print(f"## {name}")
    print(f"  startup (until drawing settles): median {statistics.median(starts)*1000:.1f} ms")
    if memory:
        print(f"  memory after startup: median {statistics.median(memory):.1f} MB")
    if startup_only:
        return

    pid, fd = spawn(argv, env)
    read_burst(fd, 5.0, 0.5)
    if idle and (before := usage(pid)):
        # Nothing to read while idle, unless the editor draws on its own.
        read_burst(fd, idle, idle)
        after = usage(pid)
        cpu = (after[0] - before[0]) / idle * 100
        print(f"  idle {idle:g} s: CPU {cpu:.3f} % of a core, {(after[1] - before[1]) / idle:.1f} wakeups/s")
    results = {}
    for label, prep, key in [
        ("insert 'a'", keys.insert, b"a"),
        ("scroll", keys.normal, keys.half_page),
    ]:
        os.write(fd, prep)
        read_burst(fd, 0.5, 0.2)
        firsts, lasts = [], []
        for i in range(n):
            if label == "scroll" and i % 40 == 0:
                os.write(fd, keys.top); read_burst(fd, 0.5, 0.1)
            os.write(fd, key)
            f, l = read_burst(fd, 1.0, 0.015)
            if f is not None:
                firsts.append(f * 1000); lasts.append(l * 1000)
            time.sleep(0.01)
        results[label] = (firsts, lasts)
    if u := usage(pid):
        print(f"  memory after {2 * n} keys: {u[2]:.1f} MB")
    kill(pid, fd)

    for label, (firsts, lasts) in results.items():
        if not firsts:
            print(f"  {label:11s} no response")
            continue
        print(f"  {label:11s} first byte: median {statistics.median(firsts):.2f} ms  p99 {pct(firsts, .99):.2f} ms"
              f" | frame done: median {statistics.median(lasts):.2f} ms  p99 {pct(lasts, .99):.2f} ms  (n={len(firsts)})")


class Keys:
    """The keys that do the same thing in each editor."""
    def __init__(self, insert=b"i", normal=b"\x1b", half_page=b"\x04", top=b"gg"):
        self.insert, self.normal, self.half_page, self.top = insert, normal, half_page, top


class Editor:
    def __init__(self, name, argv, keys=Keys(), env=None):
        self.name, self.argv, self.keys, self.env = name, argv, keys, env


# Emacs has no modes; C-v scrolls a whole screen and M-< goes to the top.
EMACS_KEYS = Keys(insert=b"", normal=b"\x07", half_page=b"\x16", top=b"\x1b<")

NIB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "nib")


def editors(path, config):
    """Each editor without user settings, and with LSP off."""
    hx_config = os.path.join(config, "helix.toml")
    with open(hx_config, "w") as f:
        f.write("[editor.lsp]\nenable = false\n")
    os.makedirs(os.path.join(config, "nib", "plugins"), exist_ok=True)
    with open(os.path.join(config, "nib", "plugins", "lsp.toml"), "w") as f:
        f.write("enabled = false\n")
    program = lambda name: os.environ.get(f"BENCH_{name.upper()}", name)
    return {
        # No swap file: runs end with SIGKILL, and a swap file left behind
        # makes the next run stop at a prompt.
        "vim": Editor("vim --clean", [program("vim"), "--clean", "-n", path]),
        "nvim": Editor("nvim --clean", [program("nvim"), "--clean", "-n", path]),
        "hx": Editor("helix (lsp off)", [program("hx"), "-c", hx_config, path]),
        "emacs": Editor("emacs -nw -Q", [program("emacs"), "-nw", "-Q", path], EMACS_KEYS),
        "nib": Editor("nib (lsp off)", [NIB, path], env={"XDG_CONFIG_HOME": config}),
    }


if __name__ == "__main__":
    import tempfile
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    args = sys.argv[2:]
    startup_only = "--startup" in args
    idle = 0.0
    if "--idle" in args:
        i = args.index("--idle")
        idle = float(args[i + 1])
        del args[i:i + 2]
    path, which = sys.argv[1], [a for a in args if a != "--startup"]
    with tempfile.TemporaryDirectory() as config:
        all_editors = editors(path, config)
        for e in which or list(all_editors):
            run(all_editors[e], startup_only, idle)
