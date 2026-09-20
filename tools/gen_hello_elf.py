#!/usr/bin/env python3
"""Generate a minimal static ELF64 the kernel's Phase-5 loader can map.

The program is tiny hand-assembled x86-64 that:
    write(1, msg, len)   ; syscall number 1 in rax, args rdi/rsi/rdx
    exit(0)              ; syscall number 2 in rax, code in rdi
both via `int 0x80`. Everything (code + the message string) lives in a single
RWX PT_LOAD at 0x400000 so the loader only has to map one page and copy bytes.
No relocations, no symbols -- just header + one program header + payload.
"""
import struct
import sys
import os

BASE = 0x400000
MSG = b"Hello from user ring3!\n"

# --- hand-assembled machine code (fixed sizes) ---
def mov_rax_imm32(v): return b"\x48\xc7\xc0" + struct.pack("<I", v)
def mov_rdi_imm32(v): return b"\x48\xc7\xc7" + struct.pack("<I", v)
def mov_rdx_imm32(v): return b"\x48\xc7\xc2" + struct.pack("<I", v)
def mov_rsi_imm64(v): return b"\x48\xbe" + struct.pack("<Q", v)
INT80 = b"\xcd\x80"

CODE_LEN = 7 + 7 + 10 + 7 + 2 + 7 + 7 + 2  # see layout below
MSG_ADDR = BASE + CODE_LEN

code = b""
code += mov_rax_imm32(1)          # syscall: write
code += mov_rdi_imm32(1)          # fd = 1 (console)
code += mov_rsi_imm64(MSG_ADDR)   # buf = msg
code += mov_rdx_imm32(len(MSG))   # len
code += INT80
code += mov_rax_imm32(2)          # syscall: exit
code += mov_rdi_imm32(0)          # code = 0
code += INT80
assert len(code) == CODE_LEN, (len(code), CODE_LEN)

payload = code + MSG

# --- ELF64 ---
Ehsize, Phentsize = 64, 56
phoff = Ehsize
shoff = 0
entry = BASE
seg_off = Ehsize + Phentsize

e_ident = b"\x7fELF" + bytes([2, 1, 1, 0]) + b"\x00" * 8  # 64-bit, LSB, SysV
ehdr = e_ident + struct.pack(
    "<HHIQQQIHHHHHH",
    2,               # e_type = ET_EXEC
    0x3E,            # e_machine = EM_X86_64
    1,               # e_version
    entry,           # e_entry
    phoff,           # e_phoff
    shoff,           # e_shoff
    0,               # e_flags
    Ehsize,          # e_ehsize
    Phentsize,       # e_phentsize
    1,               # e_phnum
    64,              # e_shentsize
    0,               # e_shnum
    0,               # e_shstrndx
)
# PT_LOAD, flags RWX(7)
phdr = struct.pack(
    "<IIQQQQQQ",
    1,               # p_type = PT_LOAD
    7,               # p_flags = PF_R|PF_W|PF_X
    seg_off,         # p_offset
    BASE,            # p_vaddr
    BASE,            # p_paddr
    len(payload),    # p_filesz
    len(payload),    # p_memsz
    0x1000,          # p_align
)

elf = ehdr + phdr + payload
out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "user", "hello.elf")
with open(out, "wb") as f:
    f.write(elf)
print("wrote %s (%d bytes, entry=%#x, msg@%#x)" % (out, len(elf), entry, MSG_ADDR))
