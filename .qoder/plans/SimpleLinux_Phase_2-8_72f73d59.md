# SimpleLinux 教学操作系统 — Phase 2–8 实施计划

## 总览

- **基础**：Phase 0（工具链）与 Phase 1（boot + serial + vga + `print!/println!`）**已完成**，当前是仓库根目录下的**单 crate**（`src/main.rs`、`src/serial.rs`、`src/vga.rs`）。
- **本次目标**：先做一次**结构重构**（单 crate → `kernel/` + `user/` workspace），再实现 **Phase 2–8**。
- **技术栈不变**：Rust `no_std` + `x86_64-unknown-none` + `bootloader 0.9`（`map_physical_memory`）+ QEMU + `cargo bootimage`。
- **教学约束（贯穿全程）**：每个 `.rs` ≤ 400 行、每个函数 ≤ 60 行；选最朴素算法；用 Rust 类型系统（enum 状态机、trait 多态、newtype）表达 OS 概念；不做 SMP/COW/信号/ext2/swap。

## 可复用的现有模式（务必沿用，保持一致性）

- `src/serial.rs`：`lazy_static! { pub static ref SERIAL1: Mutex<SerialPort> }` + `init()` 触发 + `_print(args)`。
- `src/vga.rs`：`Writer` 实现 `core::fmt::Write`；`WRITER: Mutex<Writer>`；`init()` 清屏；volatile 读写 `0xB8000`。
- `src/main.rs`：`#[macro_export] print!/println!` 双通道（串口 + VGA）；`entry_point!(kernel_main)`；`#[panic_handler]`。
- `Color` enum（`#[repr(u8)]`）作为 newtype 风格的样板。

---

## Phase 0.5：Workspace 结构重构（前置于所有新功能）

> 这是本次计划相对原文档的**关键修正**：原文档假设了 `kernel/` + `user/` 结构但当前代码没照做；而 Phase 5（用户态 ELF）在单 crate 下**不可行**（`#[entry_point]` 与 user 的独立 `#[no_main]` 产物冲突，无法同时产出 initrd 内的用户 ELF）。因此必须先拆分。

**任务**：
1. 根 `Cargo.toml` 改为 workspace 清单：`[workspace] members = ["kernel", "user"]`（把原 `[package]`/`[dependencies]`/`[profile]`/`[package.metadata.bootimage]` 迁入 `kernel/Cargo.toml`）。
2. 新建 `kernel/Cargo.toml`：承接原 `[package] name = "simplelinux-kernel"`、bootloader/x86_64/uart_16550/lazy_static/spin/linked_list_allocator 依赖、`panic = "abort"` profile、`[package.metadata.bootimage] run-command`。
3. 把 `src/main.rs`、`src/serial.rs`、`src/vga.rs` 原样移动到 `kernel/src/`（内容不变，仅路径变化）。
4. 新建 `user/` crate：`user/Cargo.toml`（`name="simplelinux-user"`, `no_std`, 无 bootloader 依赖）、`user/src/lib.rs`（暂空 stub，Phase 5 填充 syscall 包装）。
5. `.cargo/config.toml`：`[build] target` 与 `[unstable] build-std` 保持 workspace 级（两 crate 都是 no_std / 同一 target，兼容）。移除任何 `runner` 相关项（用 Makefile 驱动）。
6. `Makefile` 更新：`build` → `cargo bootimage -p simplelinux-kernel`；`run/debug` 里镜像路径改为 `target/x86_64-unknown-none/{debug,release}/bootimage-simplelinux-kernel.bin`（bootimage 产物名随 kernel 包名变化，**验证时以实际 `ls target/.../` 为准**）。
7. 更新 `.gitignore`（`target/`、`*.iso`、`*.bin` 已有，确认覆盖 workspace 各 target）。

**验证**：`cargo bootimage -p simplelinux-kernel` 成功，`make run` 串口出现与重构前**完全相同**的 Phase 1 输出（纯结构迁移，行为零变化）。

**依赖**：无（第一步）。**后续所有 Phase 的代码都写在 `kernel/src/` 下。**

---

## Phase 2：中断与定时器

**目标**：响应 CPU 异常与硬件中断，PIT 以 100Hz 触发。

**新增文件**：
- `kernel/src/gdt.rs`：`x86_64::structures::gdt::GlobalDescriptorTable` — kernel code/data 段 + TSS（此阶段先只有内核段与最小 IST 区，user 段留到 Phase 5）。`lazy_static! GDT` + `load()`。
- `kernel/src/interrupts.rs`：`x86_64::structures::idt::InterruptDescriptorTable`；注册 `page_fault`/`general_protection`/`double_fault`(设 IST 索引)/`breakpoint`；用 `pic8259::Pic8259` 重映射 IRQ0–15 → int 32–47；`Idt::load()`、`disable_and_remap`。
- `kernel/src/timer.rs`：`PICK` 通道 0，1193182/100 分频 → 100Hz；`static TICKS: AtomicU64`，IRQ0 handler 递增并 `eoi`。

**新增依赖（kernel/Cargo.toml）**：`pic8259 = "10"`、`pc-keyboard = "0.5"`（Phase 7 用，此处一并加）。

**Rust 要点**：`extern "x86-interrupt" fn handler(...)`；`set_stack_index` 需 `x86_64` 的 `StackMutable`；`lazy_static! IDT`。

**main.rs 接线**：`gdt::init(); interrupts::init(); timer::init();` 然后在循环里每 ~100 tick 打印一次。

**验证**：串口打印 `tick=100/200/...`；主动 `int3` 触发 breakpoint handler 正确打印；访问 `0x0` 触发 page fault 且打印 `CR2`。

**依赖**：Phase 0.5。

---

## Phase 3：内存管理

**目标**：物理帧分配 + 虚拟映射 + 内核堆（启用 `alloc`）。

**新增文件**：
- `kernel/src/memory/mod.rs`：子模块声明 + 统一 `init(boot_info)` 编排。
- `kernel/src/memory/pmm.rs`：`pub trait FrameAllocator { fn allocate_frame(&mut self) -> Option<PhysFrame>; fn deallocate_frame(&mut self, f: PhysFrame); }`；`BitmapFrameAllocator`（1 bit / 4KiB 帧，位图静态数组）；从 `boot_info.memory_map` 标记 kernel/boilerplate/usable；利用 `map_physical_memory` 得到的 `PhysicalToVirtualTranslator`（`PhysFrame::start_address() + OFFSET_0xFFFF800000000000`）访问任意物理帧。
- `kernel/src/memory/vmm.rs`：基于 `x86_64::structures::paging::{OffsetPageTable, Mapper, Translator}`；封装 `map_to(vaddr, paddr, flags, &alloc)` / `unmap`；复用 bootloader 交的 PML4（`boot_info.page_table`）。
- `kernel/src/memory/heap.rs`：`linked_list_allocator` 提供 `#[global_allocator]`；`init_heap()` 用 `mapper` 在内核高地址映射一段（初始 1MiB，可扩），`LockedHeap::init`。

**约定**：堆初始化前**不得**使用 `Vec/Box/String`；`memory::init` 必须在 `main.rs` 早期、其它依赖堆的模块之前调用。

**验证**：分配/释放 1000 帧后 `free_count` 复原；`vec![0u8; 4096]` 分配成功并可用；访问未映射地址由 Phase 2 的 page fault handler 打印诊断。

**依赖**：Phase 2（page fault 诊断）。

---

## Phase 4：进程与线程调度

**目标**：内核态协作/抢占多任务，轮转调度可见交错输出。

**新增文件**：
- `kernel/src/process/mod.rs`：模块声明 + `init()`。
- `kernel/src/process/task.rs`：`TaskId(u32)` newtype；`enum TaskState { Ready, Running, Blocked(WaitReason), Zombie }`；`struct TaskContext { rsp, rip, callee_saved... }`；`struct KernelStack`（每任务 8KiB，`Drop` 归还堆）；`struct Task`。
- `kernel/src/process/scheduler.rs`：`RoundRobinScheduler { queue: VecDeque<Task> }`（`spin::Mutex` 保护）；`schedule()`；`add_task`；`WaitQueue`（sleep/join）。
- `kernel/src/process/context.rs`：`context_switch(prev, next)` — `global_asm!` naked 函数保存/恢复 rbx,rbp,r12–r15,rsp,rip（本阶段仅内核任务，**不切 CR3**）。
- `sleep/yield`：基于 Phase 2 的 `TICKS`。

**接线**：IRQ0（timer）handler 末尾调用 `scheduler::tick()` → 达时间片则 `schedule()`。

**验证**：3 个内核任务交错打印 `t1/t2/t3 #n` 各 5 轮；`join` 正确阻塞；32 任务压力测试无死锁/无泄漏（`free_count` 稳定）。

**依赖**：Phase 2（tick）、Phase 3（堆→`VecDeque`/`KernelStack`、`alloc`）。

---

## Phase 5：内核态/用户态分离与系统调用

**目标**：ELF 用户程序运行于 ring 3，经 `syscall/sysret` 请求内核服务。

**新增/修改**：
- `kernel/src/gdt.rs` 扩展：加 **user data(0x23)/user code(0x1B)** 段；补全 **TSS**：`ist[0]`（double fault 栈）+ `privilege_stack_table[0]`（ring3→ring0 的 RSP0）。
- `kernel/src/process/elf.rs`：最小 ELF64 解析（手动读 header/program header，避免重依赖）；把 `PT_LOAD` 段映射进**新建的用户页表**低地址区；`user_ptr` 校验。
- `kernel/src/process/mod.rs` 增加 `spawn_user(elf: &[u8]) -> Task`：为该任务建独立 PML4（内核高半区共享同一套映射，低半区放用户段，用户页 `USER|PRESENT|WRITABLE`，代码页不可写）。
- `kernel/src/syscall/mod.rs`：`extern "C"` naked `syscall_entry`（`swapgs`→保存寄存器到任务 `TaskContext`→`dispatch`→恢复→`sysret`）；配置 MSR：`STAR`/`LSTAR`/`SFMASK`（`wrmsr`）。
- `kernel/src/syscall/handler.rs`：读 `rax`(号)+`rdi/rsi/rdx`；分发；`Result<i64, Errno>` 转返回码。
- `kernel/src/syscall/table.rs`：`#[repr(u64)] enum SyscallNr { Write=4, Exit=1, ... }`（子集：`write/exit/open/read/close/getpid/yield`）。
- 首次进入 ring3：构造 `iretq` 帧（SS=0x23,RSP,RFLAGS=0x202,CS=0x1B,RIP=entry）。
- `user/src/lib.rs` + `user/src/syscall.rs`：`no_std`、`syscall1/3` 包装（`core::arch::asm!("syscall")`）；`user/src/bin/hello.rs`：`write(1, "hello from user", 15)` 后 `exit`。

**风险与缓解**：`bootloader 0.9` 不直接支持用户态。缓解：内核始终用**高半区映射**（`map_physical_memory`）访问自身与物理内存，用户进程用**独立低半区**页表；`swapgs` 需 GDT 里内核 GS 段布局与 `bootloader` 默认段兼容——实现时先验证 `swapgs` 后仍能访问内核，否则退回 `int 0x80` 软件中断方案（在 IDT 注册 vector 0x80，走同一 `dispatch`）。

**验证**：`hello` 输出 "hello from user"；未知 syscall 号返回 `-ENOSYS`；用户态读 `0x0` → 内核捕获 page fault → 仅 kill 该进程，系统存活。

**依赖**：Phase 3（页表/堆）、Phase 4（Task 作为进程载体）。

---

## Phase 6：虚拟文件系统 (VFS)

**目标**：trait 实现"一切皆文件"。

**新增文件**：
- `kernel/src/fs/mod.rs`：模块声明 + 全局 `init()`。
- `kernel/src/fs/vfs.rs`：`trait FileSystem: Send+Sync { fn name; fn root_inode(&self)->Arc<dyn Inode>; }`；`trait Inode: Send+Sync { lookup/read/write/stat; list_dir 默认 Err(NotDir) }`；`enum FsError { NotFound, PermissionDenied, NotDir, IsDir, InvalidArg, NoSpace }`；`FileStat`；挂载表 `Vec<(String, Arc<dyn FileSystem>)>`（最长前缀）；路径解析按 `/` split 逐级 `lookup`；`struct File { inode: Arc<dyn Inode>, offset }`；per-process `fd_table: Vec<Option<Arc<File>>>`（≤64）。
- `kernel/src/fs/initrd.rs`：解析 ustar（512B 头），`impl FileSystem`，只读。
- `kernel/src/fs/devfs.rs`：`/dev/console`(走 serial)、`/dev/null`、`/dev/zero`，各 `impl Inode`。
- `kernel/src/fs/procfs.rs`：动态 `/proc/meminfo`、`/proc/uptime`（读 Phase 3 free 计数 + Phase 2 tick）。
- `kernel/src/syscall/handler.rs` 接线：`open/read/write/close` → VFS。

**验证**：用户态 `open("/dev/console")+write` → 串口输出；`open("/nope")` → `NotFound`；`ls /initrd/`（`list_dir`）列出打包文件。

**依赖**：Phase 5（syscall 通道 + 进程 fd_table）。

---

## Phase 7：设备驱动

**目标**：键盘输入可用 + e1000 网卡收发。

**新增文件**：
- `kernel/src/drivers/mod.rs`。
- `kernel/src/drivers/keyboard.rs`：IRQ1 → 读端口 `0x60` → `pc-keyboard` 解析 scancode → ASCII；`spin::Mutex<VecDeque<u8>>` 环形缓冲；在 `devfs` 注册 `/dev/kbd`（`read` 无数据时阻塞任务，接 Phase 4 `WaitQueue`）。
- `kernel/src/drivers/e1000.rs`：PCI 配置空间扫描（`0xCF8/0xCFC`，vendor=0x8086 device=0x100E）→ BAR0 取 MMIO 基址；`#[repr(C)] struct E1000Regs { ctrl: Volatile<u32>, ... }`（用 `volatile` crate 或手写 `Volatile<T>`）；初始化 `CTRL.RST`→ 配置 RX/TX 描述符环（各 8 项，物理页来自 Phase 3 PMM，经高半映射读写）；`RDBAL/RDLEN/RDT`、`TDT` 触发发送；IRQ handler 读 `ICR` 清除并处理收包。

**新增依赖**：`volatile = "0.4"`、（可选）`volatile_register`（择一，教学用自写 `Volatile<T>` 更清晰）。

**验证**：QEMU 窗口键盘输入 → `/dev/kbd` 回显到串口；启动打印 e1000 link up + MAC；发一个 ARP 请求收到 reply（为 Phase 8 铺垫）。

**依赖**：Phase 2（IRQ）、Phase 3（DMA 物理页）、Phase 6（`/dev/kbd`）。

---

## Phase 8：网络栈 + Shell 集成

**目标**：自写分层协议栈 + 交互式 shell。

**新增文件（网络，自底向上）**：
- `kernel/src/net/mod.rs`：`NetworkInit`、按 EtherType/protocol 的分发注册。
- `kernel/src/net/ethernet.rs`：14B 以太网头，按 EtherType 分发。
- `kernel/src/net/arp.rs`：`BTreeMap<Ipv4Addr, MacAddr>` 缓存 + 请求/应答。
- `kernel/src/net/ip.rs`：IPv4 头 + checksum，按 protocol 分发。
- `kernel/src/net/icmp.rs`：echo request→reply（支撑 `ping`）。
- `kernel/src/net/udp.rs`：端口→socket 映射，`sendto/recvfrom`。
- `kernel/src/net/tcp.rs`：最小教学版，`enum TcpState { Closed, Listen, SynSent{..}, SynReceived{..}, Established{..}, FinWait1{..}, FinWait2{..}, CloseWait{..}, LastAck{..}, TimeWait{..} }`；固定 MSS=536、1s 重传、无拥塞控制。
- `kernel/src/net/socket.rs`：BSD 风格 socket，`SocketInode impl Inode` 融入 VFS（"网络也是文件"）；新增 `socket/send/recv` syscall。

**网络配置**：硬编码 `IP=10.0.2.15, GW=10.0.2.2`（QEMU user-mode）。

**Shell**：
- `tools/make_initrd.py`：把 `user/` 产物 + 文本文件打包为 `initrd.tar`（ustar）。
- `user/src/bin/shell.rs`：从 `/dev/kbd` 读行 → `split_whitespace` → 匹配命令；内置 `help/ls/cat/echo/ps/free/ping/exit`；外部命令 `fork+exec+wait`。
- 内核启动 init 进程 → init 拉起 shell。
- `Makefile`/`bootimage` run-command 已含 `-netdev user,... -device e1000`（现有 `Cargo.toml` 已配，保留）。initrd 需通过 bootloader `custom_cmdline` 或编译期内嵌方式传入（`bootloader 0.9` 无 initrd 参数 → **用 `include_bytes!` 把 `initrd.tar` 内嵌进内核**，最简可靠）。

**验证命令矩阵**：主机 `ping 10.0.2.15` 收到 reply；`echo hi | nc -u -w1 127.0.0.1 9999` → 串口打印；shell 内 `help/ls /` 等全部工作；连续运行 5 分钟无 panic。

**依赖**：Phase 6（VFS/socket fd）、Phase 7（e1000 收发）。

---

## 阶段依赖顺序

```
Phase 0.5 (workspace 重构)
  └→ P2 (中断/GDT/IDT/PIT)
       └→ P3 (PMM/VMM/heap, 启用 alloc)
            └→ P4 (进程/线程/调度)
                 └→ P5 (用户态/syscall/ELF)
                      └→ P6 (VFS)
                           └→ P7 (键盘 + e1000 驱动)
                                └→ P8 (网络栈 + Shell)
```
严格串行；P6–P8 中「接口先冻结」的模块（VFS trait、net 分发）可适度并行，但驱动 P7 依赖 P6 的 `/dev` 注册。

---

## 构建与运行（重构后）

`Makefile`（`build`/`run`/`debug`/`iso`/`clean`/`initrd`），核心：
- `build`: `cargo bootimage -p simplelinux-kernel`（`debug` 默认；`--release` 可选）
- `run`: `qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-simplelinux-kernel.bin -m 128M -serial stdio -display none -no-reboot -netdev user,id=n0,hostfwd=tcp::1234-:1234,hostfwd=udp::9999-:9999 -device e1000,netdev=n0`
- `initrd`(P8): `python3 tools/make_initrd.py`（生成供 `include_bytes!` 内嵌的 tar）

> 具体 bootimage 产物文件名以 `ls target/x86_64-unknown-none/debug/` 实测为准（随 kernel 包名变化）。

---

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| workspace 重构破坏现有 Phase 1 行为 | 纯移动文件 + 拆 manifest，`make run` 输出须与重构前逐字节一致后再进 Phase 2 |
| `bootimage` 产物名/路径变化 | 每次 `ls target/.../` 实测；Makefile 路径集中定义一处 |
| `bootloader 0.9` 不支持用户态/initrd | 高半映射 + 独立低半用户页表；`swapgs` 不兼容则退回 `int 0x80`；initrd 用 `include_bytes!` 内嵌 |
| naked fn / `asm!` 随 nightly 变动 | `rust-toolchain.toml` 已锁 `nightly-2025-01-01`，勿随意升级 |
| 上下文切换汇编错 | 先做纯内核任务（不切 CR3）跑通，再在 P5 叠加 CR3 切换 |
| e1000 寄存器偏移错 | 严格对照 Intel 82540EM 手册；用 QEMU `-trace`/日志验证；先 ARP 再 IP 分层排查 |
| 堆启用前误用 `Vec/Box` | 模块 init 顺序在 `main.rs` 固定；`heap::init` 前只允许静态类型 |
| 全局锁重入死锁（interrupt 里 println 抢 WRITER/SERIAL） | handler 内尽量不打印；必要时用 try_lock 或专用无锁缓冲 |

---

## 关键文件清单

1. `Cargo.toml`（根，改为 workspace）+ `kernel/Cargo.toml` — 依赖与 bootimage 配置中枢。
2. `kernel/src/main.rs` — 初始化编排（每阶段在此按序接线）。
3. `kernel/src/memory/pmm.rs` + `vmm.rs` + `heap.rs` — Phase 3，后续所有动态结构的地基。
4. `kernel/src/syscall/mod.rs` + `table.rs` + `handler.rs` — Phase 5/6/8 的内核入口。
5. `kernel/src/fs/vfs.rs` — Phase 6 的 trait 抽象，被驱动(P7)/网络(P8)复用。

---

## 未采用方案（Rejected Alternatives）

| 方案 | 拒绝原因 |
|---|---|
| **保持单 crate 扁平结构**（Plan C） | Phase 5 不可行：`bootloader` 的 `#[entry_point]` 与用户态独立 ELF 产物在同一 no_std crate 冲突，无法产出 initrd 内用户程序。用户已选 workspace。 |
| **推迟重构到 Phase 5 再做**（Plan A 变体） | 会把最大结构风险压到最复杂阶段叠加，回滚代价高；前置重构使每阶段依赖清晰。 |
| **换用 `limine`/自写 Multiboot2 引导** | 增加引导层复杂度，偏离教学重心；现有 `bootloader 0.9` + `map_physical_memory` 足以覆盖 P2–P8。 |
| **`smoltcp` 替代自写网络栈** | 省事但失去协议分层教学价值，与项目"帮助理解 OS 核心实现思路"目标冲突。 |
| **async/await 调度器** | no_std 内核中过度复杂，轮转调度更贴合教学且可控。 |
| **initrd 用 bootloader 命令行传址** | `bootloader 0.9` 无 initrd 参数支持；`include_bytes!` 内嵌最简可靠。 |
