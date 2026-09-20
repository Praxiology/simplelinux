#!/usr/bin/env python3
"""Generate a Phase-6 user ELF that exercises the VFS via `int 0x80`.

Program (hand-assembled x86-64), all in one RWX PT_LOAD at 0x400000:
    fd = open("/dev/console", 0)      ; rax=3, rdi=path, rsi=flags -> int 0x80
    write(fd, msg, len)               ; rax=1, rdi=fd,  rsi=buf, rdx=len -> int 0x80
    exit(0)                           ; rax=2, rdi=0
Syscall numbers match `kernel/src/syscall/mod.rs::SyscallNr`.
"""
import struct
import sys
import os

BASE = 0x400000
PATH = b"/dev/console\x00"                     # NUL-terminated, read by sys_open
MSG = b"Hello via VFS /dev/console!\n"


def mov_rax(v): return b"\x48\xc7\xc0" + struct.pack("<I", v)
def mov_rdi(v): return b"\x48\xc7\xc7" + struct.pack("<I", v)
def mov_rsi(v): return b"\x48\xc7\xc6" + struct.pack("<I", v)
def mov_rdx(v): return b"\x48\xc7\xc2" + struct.pack("<I", v)
def mov_rsi_q(v): return b"\x48\xbe" + struct.pack("<Q", v)
MOV_RDI_RAX = b"\x48\x89\xc7"                  # mov rdi, rax
INT80 = b"\xcd\x80"

# Fixed-size code: open(3) | write(1) | exit(2)
CODE_LEN = (7 + 7 + 7 + 2) + (3 + 7 + 10 + 7 + 2) + (7 + 7 + 2)
PATH_ADDR = BASE + CODE_LEN
MSG_ADDR = PATH_ADDR + len(PATH)

code = b""
code += mov_rax(3) + mov_rdi(PATH_ADDR) + mov_rsi(0) + INT80   # open -> fd in rax
code += MOV_RDI_RAX + mov_rax(1) + mov_rsi_q(MSG_ADDR) + mov_rdx(len(MSG)) + INT80  # write
code += mov_rax(2) + mov_rdi(0) + INT80                        # exit(0)
assert len(code) == CODE_LEN, (len(code), CODE_LEN)

payload = code + PATH + MSG

# --- ELF64 (single PT_LOAD, RWX) ---
Ehsize, Phentsize = 64, 56
entry = BASE
seg_off = Ehsize + Phentsize
e_ident = b"\x7fELF" + bytes([2, 1, 1, 0]) + b"\x00" * 8
ehdr = e_ident + struct.pack(
    "<HHIQQQIHHHHHH", 2, 0x3E, 1, entry, Ehsize, 0, 0,
    Ehsize, Phentsize, 1, 64, 0, 0,
)
phdr = struct.pack(
    "<IIQQQQ", 1, 7, seg_off, BASE, BASE, len(payload)
) + struct.pack("<QQ", len(payload), 0x1000)
elf = ehdr + phdr + payload

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "user", "vfs.elf")
with open(out, "wb") as f:
    f.write(elf)
print("wrote %s (%d bytes, entry=%#x, path@%#x, msg@%#x)"
      % (out, len(elf), entry, PATH_ADDR, MSG_ADDR))
