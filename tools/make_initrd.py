#!/usr/bin/env python3
"""Build a tiny ustar archive for the kernel's Phase-6 initrd.

The kernel embeds `user/initrd.tar` (via `include_bytes!`) and parses it with
`kernel/src/fs/initrd.rs`, so `ls /initrd/` and `cat /initrd/<file>` work at
runtime. Only the fields our parser reads are meaningful: name, size, typeflag.
"""
import io
import os
import tarfile

OUT = os.path.join(os.path.dirname(__file__), "..", "user", "initrd.tar")

FILES = {
    "motd.txt": "Welcome to SimpleLinux (Phase 6 VFS)!\n",
    "about.txt": "initrd: a real ustar archive parsed into the VFS.\n",
    "etc/motd": "nested directory entry to prove path walking works.\n",
}


def add_bytes(tar, name, data: bytes, is_dir=False):
    info = tarfile.TarInfo(name=name)
    info.type = tarfile.DIRTYPE if is_dir else tarfile.REGTYPE
    info.size = 0 if is_dir else len(data)
    info.mode = 0o755 if is_dir else 0o644
    tar.addfile(info, io.BytesIO(b"" if is_dir else data))


def main():
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with tarfile.open(OUT, "w", format=tarfile.USTAR_FORMAT) as tar:
        add_bytes(tar, "etc", b"", is_dir=True)
        for name, text in sorted(FILES.items()):
            add_bytes(tar, name, text.encode())
    with open(OUT, "rb") as f:
        blob = f.read()
    print("wrote %s (%d bytes)" % (OUT, len(blob)))


if __name__ == "__main__":
    main()
