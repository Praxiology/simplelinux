.PHONY: build run release debug clean initrd

# The root `simplelinux` package is a host-side runner: building it compiles the
# kernel (x86_64-unknown-none artifact), produces a bootable BIOS image via
# `bootloader`, and `cargo run` launches QEMU.

build:
	cargo build -p simplelinux

run:
	cargo run -p simplelinux

release:
	cargo run --release -p simplelinux

debug:
	cargo run -p simplelinux -- -s -S

initrd:
	python3 tools/make_initrd.py

clean:
	cargo clean
