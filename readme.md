# SimpleLinux —— 教学型简易 Linux 内核

> 目标：实现一个简易版的类 Linux 操作系统，涵盖 **内存、进程、线程、虚拟文件系统、驱动、内核态、用户态、网络栈** 几大核心功能，帮助理解与掌握操作系统的核心实现思路。

这是一个用 **Rust（`no_std`）** 编写的 x86_64 教学内核。它不追求性能与完备，而是用最短、最朴素的实现把每一个 OS 概念讲清楚：每一层都能独立观测、独立验证。

---

## 一、环境前置

| 项目 | 要求 |
| --- | --- |
| Rust 工具链 | `nightly`（由 `rust-toolchain.toml` 锁定，自动下载），组件含 `rust-src`、`llvm-tools`，target `x86_64-unknown-none` |
| Cargo | 需 nightly 以启用 `bindeps`（artifact-dependencies，见 `.cargo/config.toml`） |
| QEMU | `qemu-system-x86_64`（本仓库在 QEMU 10.0.13 上验证） |
| Python 3 | 用于 `tools/` 下的用户态 ELF / initrd 生成脚本 |

## 二、快速开始

```bash
# 构建内核 + 生成可引导 BIOS 镜像（root runner 通过 bindeps 编译 kernel）
make build          # 等价于 cargo build -p simplelinux

# 构建并拉起 QEMU（串口直连 stdio）
make run            # 等价于 cargo run -p simplelinux

make release        # 优化版
make debug          # 附带 -s -S（GDB 桩，localhost:1234）
make initrd         # 重新生成 initrd（ustar）
make clean
```

> ⚠️ 只能用 `cargo build/run -p simplelinux`（root runner）。直接对 `kernel` 打包会触发 `_start` 重复符号——`bootloader 0.11` 由 runner 统一链接。

手动抓串口（判定各 Phase 输出时更稳）：

```bash
IMG=$(find target -name "bios.img" | head -1)
timeout 25 qemu-system-x86_64 -drive format=raw,file=$IMG -m 128M \
  -serial file:/tmp/serial.log -display none -no-reboot \
  -netdev user,id=n0 -device e1000,netdev=n0
cat /tmp/serial.log        # 退出码 124 属 timeout 正常结束
```

## 三、功能地图（按 Phase）

内核在 `kernel_main` 中顺序执行各 Phase 的自检 demo，全部输出到串口。任一环节 `panic` 即说明该层被破坏。

| Phase | 功能点 | 关键模块 | 预期观测 |
| --- | --- | --- | --- |
| 0.5 | 引导 + 串口/VGA | `main.rs`、`serial.rs`、`vga.rs` | 启动横幅、`Hello` 双通道输出 |
| 1 | 内存：GDT/IDT/中断/堆分配 | `gdt`、`interrupts`、`memory` | 堆分配成功、断点/缺页异常被捕获 |
| 2 | 进程与上下文切换 | `process/task`、`process/context` | 多任务往返切换计数递增 |
| 3 | 调度器（抢占/睡眠） | `process/scheduler` | 时间片轮转、`sleep` 到期唤醒 |
| 4 | 内核任务 + 键盘/空闲交错 | `process`、`drivers/keyboard` | `ps` 快照含 ready/running/blocked/zombie |
| 5 | 虚拟文件系统（VFS） | `fs/vfs`、`fs`、`devfs` | 挂载、`ls`/`cat`、fd 表读写 |
| 6 | 用户态 ring3 + syscall | `cpu/ring3`、`syscall`、`user/` | `int 0x80` 进入内核执行 `write`/`exit` |
| 7 | e1000 驱动 | `drivers/e1000` | RX/TX 环初始化、设备识别（见下方限制） |
| 8 | 网络栈 + Shell | `net/*`、`shell.rs` | 各层校验和自洽、socket 经 VFS 读写、shell 内置命令全跑通 |

网络栈自底向上分层：`ethernet → arp → ip → icmp/udp/tcp → socket`，并通过 `SocketInode` 融入 VFS（“网络也是文件”）。Shell 作为内核任务运行，读取 `/dev/kbd`，`phase8_setup` 用 `keyboard::inject_str` 注入脚本化会话演示 `help/echo/ls/cat/ps/free/ifconfig/arp/netstat/ping` 等内置命令。

## 四、目录结构

```
simplelinux/            # workspace root，同时是 host runner（build.rs 产镜像 + 拉 QEMU）
├── kernel/             # no_std 内核
│   └── src/
│       ├── main.rs serial.rs vga.rs
│       ├── gdt/ interrupts/ memory/        # Phase 1
│       ├── process/                        # Phase 2–4（task/context/scheduler）
│       ├── fs/                             # Phase 5（vfs.rs、devfs.rs、mod.rs）
│       ├── cpu/ syscall/                   # Phase 6（ring3 + int 0x80）
│       ├── drivers/                        # Phase 5/7（keyboard、e1000）
│       ├── net/                            # Phase 8（ethernet/arp/ip/icmp/udp/tcp/socket）
│       └── shell.rs                        # Phase 8 内核 shell
├── user/               # ring3 用户程序（手写 ELF 产物）
├── tools/              # gen_hello_elf.py / gen_vfs_elf.py / make_initrd.py
├── src/main.rs         # runner：拼 BIOS 镜像并 exec QEMU
├── build.rs Cargo.toml Makefile rust-toolchain.toml .cargo/config.toml
└── docs/ tests/        # 预留
```

## 五、测试脚本

| 脚本 | 作用 | 运行 |
| --- | --- | --- |
| `tools/gen_hello_elf.py` | 手写 x86-64 `hello` 用户 ELF（`write`+`exit`），`include_bytes!` 内嵌 | `python3 tools/gen_hello_elf.py` |
| `tools/gen_vfs_elf.py` | 生成带 VFS 读写的用户 ELF | `python3 tools/gen_vfs_elf.py` |
| `tools/make_initrd.py` | 打包 initrd（ustar 归档） | `make initrd` |

各 Phase 的“测试用例”即为其串口自检 demo：判定方式为运行后在串口日志中核对预期输出行、且全程无 `panic!` / Double Fault / 三方故障。

## 六、已知限制

- **e1000 TX（QEMU 10.0.13）**：出站帧字节级构造正确，但设备模型不置 `ICR.TXQE` 完成位、host 侧 `pcap` 恒 0 包，属 emulator 级限制。故 Phase 8 网络验证采用“**注入合成入站帧 → 驱动 decode/构造/应答**”的确定性内部自洽校验，host 侧 `ping`/`nc` 不可观测。
- **教学范围裁剪**：不做 SMP、COW、信号、ext2、swap；算法一律取最朴素实现。
- **代码约束**：每个 `.rs ≤ 400 行`、每函数 `≤ 60 行`，用 Rust 类型系统表达 OS 概念。

## 七、详细文档

完整《操作手册》（各功能点的前置条件、运行步骤、测试用例、预期效果与排障表）已通过 Qoder RepoWiki 维护：

> 在 IDE 中打开 **RepoWiki / 知识库 → 《操作手册》**（`.qoder/repowiki/zh/content/操作手册.md`）。

## 八、版本管理

按 Phase 分阶段提交并推送；每个 Phase 完成即做一次 `feat(...)` 提交。当前主线：`6baf059` 初始化 → `2e3c1c8` Phase 8（网络栈 + VFS socket + 交互式 shell）。
