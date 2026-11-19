"""Runner for a networkless Hydra request checker."""
import contextlib
import ctypes
import errno
import os
import resource
import runpy
import sys

resource.setrlimit(resource.RLIMIT_AS, (64 * 1024 * 1024,) * 2)
resource.setrlimit(resource.RLIMIT_FSIZE, (4 * 1024 * 1024,) * 2)
resource.setrlimit(resource.RLIMIT_NOFILE, (64, 64))


def confine():
    """Disallow starting other processes or opening network sockets."""
    seccomp = ctypes.CDLL("libseccomp.so.2")
    seccomp.seccomp_init.argtypes = [ctypes.c_uint32]
    seccomp.seccomp_init.restype = ctypes.c_void_p
    seccomp.seccomp_rule_add.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int, ctypes.c_uint]
    seccomp.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
    seccomp.seccomp_load.argtypes = [ctypes.c_void_p]
    seccomp.seccomp_release.argtypes = [ctypes.c_void_p]
    seccomp.seccomp_release.restype = None
    rules = seccomp.seccomp_init(0x7FFF0000)  # SCMP_ACT_ALLOW
    if not rules:
        raise RuntimeError("could not initialize seccomp")
    try:
        for name in (b"clone", b"clone3", b"fork", b"vfork", b"execve", b"execveat",
                     b"socket", b"socketpair", b"io_uring_setup"):
            number = seccomp.seccomp_syscall_resolve_name(name)
            if number >= 0 and seccomp.seccomp_rule_add(rules, 0x00050000 | errno.EPERM, number, 0) != 0:
                raise RuntimeError("could not configure seccomp")
        if seccomp.seccomp_load(rules) != 0:
            raise RuntimeError("could not enable seccomp")
    finally:
        seccomp.seccomp_release(rules)


confine()
source = sys.stdin.buffer
result = sys.stdout.buffer
os.umask(0o077)

with contextlib.redirect_stdout(sys.stderr):
    check = runpy.run_path("/checker")["check"]

while header := source.read(4):
    if len(header) != 4:
        raise ValueError("incomplete request header")
    size = int.from_bytes(header, "big")
    if size > 1024 * 1024:
        raise ValueError("request too large")
    request = source.read(size)
    if len(request) != size:
        raise ValueError("incomplete request")
    with contextlib.redirect_stdout(sys.stderr):
        allowed = check(request)
    if type(allowed) is not bool:
        raise TypeError("check must return bool")
    result.write(b"\x01" if allowed else b"\x00")
    result.flush()
