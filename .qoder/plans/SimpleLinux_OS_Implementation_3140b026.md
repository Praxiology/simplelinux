# SimpleLinux 教学演示操作系统 — Rust 实施计划

## 总体方针

**技术栈**：Rust（no_std）+ x86_64 + bootloader crate + cargo + QEMU

**为什么改用 x86_64**：
- Rust 官方支持 `x86_64-unknown-none` target（Tier 2），无需自定义 target JSON
- `bootloader` crate 自动处理 Multiboot2 + 32→64 位过渡 + 初始页表建立
- `x86_64` crate 提供安全的 GDT/IDT/页表/中断抽象，大幅减少 unsafe 汇编
- 教学重心放在 OS 概念本身，而非底层引导细节

**核心 Rust crate 依赖**：
| Crate | 用途 |
|---|---|
| `bootloader` | 生成可引导镜像，处理 Multiboot2/UEFI + 长模式切换 |
| `x86_64` | 页表、GDT、IDT、寄存器抽象 |
| `pic8259` | 8259A PIC 中断控制器 |
| `pc-keyboard` | PS/2 键盘扫描码解析 |
| `uart_16550` | 串口输出 |
| `spin` | 自旋锁（Mutex/RwLock） |
| `lazy_static` | 全局静态初始化 |
| `linked_list_allocator` | 简单堆分配器（教学用） |
| `raw-cpuid` | CPU 特性检测 |
| `smoltcp` | 嵌入式网络栈（可选，或自写教学版） |

**教学约束**：
- 每个 `.rs` 文件 ≤ 400 行，每个函数 ≤ 60 行
- 所有算法选最朴素直观的实现
- 不做：SMP/多核、COW、slab/buddy、CFS、信号、动态链接、swap、ext2
- 利用 Rust 类型系统表达 OS 概念（newtype、enum 状态机、trait 多态）

---

## 目录结构

```
/root/simplelinux/
├── Cargo.toml                      # 工作区配置
├── rust-toolchain.toml             # 固定 nightly 工具链
├── .cargo/config.toml              # target、runner、build-std 配置
├── Makefile                        # 便捷命令（make run/debug/iso/clean）
├── kernel/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                 # kmain 入口 + 初始化编排
│       ├── lib.rs                  # 模块声明
│       ├── gdt.rs                  # GDT + TSS 设置
│       ├── interrupts.rs           # IDT + 异常处理 + IRQ 分发
│       ├── serial.rs               # 串口日志输出
│       ├── vga.rs                  # VGA 文本缓冲 (0xB8000)
│       ├── timer.rs                # PIT 定时器
│       ├── memory/
│       │   ├── mod.rs
│       │   ├── pmm.rs             # 物理页帧分配器（位图）
│       │   ├── vmm.rs             # 虚拟内存（4级页表操作）
│       │   └── heap.rs            # 内核堆分配器
│       ├── process/
│       │   ├── mod.rs
│       │   ├── task.rs            # 进程/线程结构体
│       │   ├── scheduler.rs       # 轮转调度器
│       │   ├── context.rs         # 上下文切换
│       │   └── elf.rs             # ELF64 加载器
│       ├── syscall/
│       │   ├── mod.rs
│       │   ├── handler.rs         # syscall 分发
│       │   └── table.rs           # 系统调用表
│       ├── fs/
│       │   ├── mod.rs
│       │   ├── vfs.rs             # VFS 核心（trait 多态）
│       │   ├── initrd.rs          # tar initrd 只读 FS
│       │   ├── devfs.rs           # 设备文件系统
│       │   └── procfs.rs          # /proc 伪文件系统
│       ├── drivers/
│       │   ├── mod.rs
│       │   ├── keyboard.rs        # PS/2 键盘
│       │   └── e1000.rs           # Intel e1000 网卡
│       └── net/
│           ├── mod.rs
│           ├── ethernet.rs        # 以太网帧
│           ├── arp.rs             # ARP 协议
│           ├── ip.rs              # IPv4
│           ├── icmp.rs            # ICMP (ping)
│           ├── udp.rs             # UDP
│           ├── tcp.rs             # 最小 TCP
│           └── socket.rs          # Socket 接口层
├── user/                           # 用户态程序
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                 # 用户态 libc-mini
│       ├── syscall.rs             # syscall 包装
│       ├── hello.rs               # hello world
│       └── shell.rs               # 交互式 shell
├── tools/
│   └── make_initrd.py             # initrd.tar 打包
├── tests/                          # 集成测试
└── docs/
    └── STYLE.md                   # 编码规范
```

---

## Phase 0：工具链准备

**任务**：
1. 安装 Rust nightly + 组件：
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   rustup toolchain install nightly
   rustup component add rust-src llvm-tools-preview --toolchain nightly
   cargo install bootimage cargo-binutils
   ```
2. 安装 QEMU：`apt-get install -y qemu-system-x86 xorriso`
3. 验证：`qemu-system-x86_64 --version`、`cargo +nightly build --target x86_64-unknown-none`
4. `git init` + `.gitignore`（target/、*.iso、*.bin）

**配置文件**：

`rust-toolchain.toml`：
```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
targets = ["x86_64-unknown-none"]
```

`.cargo/config.toml`：
```toml
[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler_builtins-mem"]

[build]
target = "x86_64-unknown-none"

[target.'cfg(target_os = "none")']
runner = "make run"
```

**验证**：最小 no_std 程序能通过 `cargo bootimage` 生成 ISO 并在 QEMU 中启动

---

## Phase 1：引导与内核入口

**目标**：QEMU 启动后串口+屏幕显示 "Hello SimpleLinux"

**实现内容**：
1. `kernel/Cargo.toml`：依赖 `bootloader`、`x86_64`、`uart_16550`、`lazy_static`、`spin`
2. `kernel/src/main.rs`：`#![no_std] #![no_main]`，`#[entry_point]` 标注的 `kernel_main(boot_info)` 
3. `kernel/src/serial.rs`：用 `uart_16550` crate 初始化 COM1，实现 `SerialPort::write_str`
4. `kernel/src/vga.rs`：VGA 文本缓冲（`0xB8000`），用 Rust struct 封装 `Writer`（含颜色、滚屏、光标）
5. 实现 `core::fmt::Write` trait → 支持 `write!` / `println!` 宏双通道输出
6. `Makefile`：`make run` = `qemu-system-x86_64 -drive format=raw,file=target/.../bootimage-simplelinux.bin -m 128M -serial stdio`

**Rust 特色**：
```rust
// VGA Writer 利用 Rust trait 系统
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result { ... }
}
// 全局串口用 lazy_static + spin::Mutex
static ref SERIAL: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });
```

**验证**：串口输出 `Hello SimpleLinux! Boot info at 0x...`

**学习价值**：理解 OS 引导过程（bootloader crate 帮我们处理了 Multiboot2 + 长模式切换）；为什么内核需要 `no_std`（没有 OS 就没有标准库）；Rust 的 `fmt::Write` trait 如何优雅地实现 print 宏。

---

## Phase 2：中断与定时器

**目标**：响应 CPU 异常和硬件中断，PIT 以 100Hz 触发

**实现内容**：
1. `kernel/src/gdt.rs`：用 `x86_64::structures::gdt` 建立 GDT（kernel code/data + user code/data + TSS）
2. `kernel/src/interrupts.rs`：
   - 用 `x86_64::structures::idt` 建立 IDT
   - 注册异常处理：page_fault、general_protection、double_fault、breakpoint
   - PIC 重映射（`pic8259` crate）：IRQ0-15 → interrupt 32-47
   - IRQ handler：timer(32)、keyboard(33)
3. `kernel/src/timer.rs`：PIT 通道 0，100Hz，每次 IRQ 递增 `TICKS` 计数器
4. Double fault 处理：设置 IST（Interrupt Stack Table）避免栈溢出时的递归 fault

**Rust 特色**：
```rust
// IDT 用 lazy_static 全局初始化
static ref IDT: InterruptDescriptorTable = {
    let mut idt = InterruptDescriptorTable::new();
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.double_fault.set_handler_fn(double_fault_handler)
        .set_stack_index(DOUBLE_FAULT_IST_INDEX);
    idt[BREAKPOINT_INT_ID].set_handler_fn(breakpoint_handler);
    idt
};

// 类型安全的中断处理
extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    println!("PAGE FAULT at {:?}, error: {:?}", CR2::read(), error_code);
}
```

**验证**：串口打印 `tick=100, 200, ...`；故意触发 breakpoint 异常能正确打印；page fault 显示 CR2 地址

**学习价值**：Rust 的 `extern "x86-interrupt"` ABI 自动处理寄存器保存/恢复（对比 C 需要手写汇编 stub）；中断描述符表的本质；PIC 级联与重映射的必要性。

---

## Phase 3：内存管理

**目标**：物理页帧分配 + 虚拟地址映射 + 内核堆

**实现内容**：

**3A — 物理内存（PMM）**：
1. 从 boot_info 获取内存映射（`bootloader::BootInfo.memory_map`）
2. 实现位图分配器：`BitmapFrameAllocator`，1 bit / 4KB frame
3. 标记内核映像、bootloader 区域为已用
4. Trait 抽象：`trait FrameAllocator { fn allocate(&mut self) -> Option<PhysFrame>; fn deallocate(&mut self, frame: PhysFrame); }`

**3B — 虚拟内存（VMM）**：
1. 利用 `x86_64::structures::paging` 操作 4 级页表（PML4 → PDPT → PD → PT）
2. bootloader 已建立初始映射（identity map），我们在此基础上添加新映射
3. `Mapper` trait 封装：`map_to(vaddr, paddr, flags)` / `unmap(vaddr)`
4. 内核区域：`0xFFFF_8000_0000_0000` 起的 direct map（bootloader 已建好）
5. 用户区域：`0x0000_0000_0040_0000` 起（后续 Phase 5 使用）

**3C — 内核堆**：
1. 使用 `linked_list_allocator` crate 提供 `#[global_allocator]`
2. 堆区：固定虚拟地址范围（如 448KB 初始，按需扩展）
3. 实现 `alloc` crate 支持 → 可用 `Vec`、`Box`、`String`、`BTreeMap`

**Rust 特色**：
```rust
// Trait 多态实现分配器可替换
pub trait FrameAllocator<S: PageSize> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>>;
    fn deallocate_frame(&mut self, frame: PhysFrame<S>>);
}

// 全局堆分配器 — 启用后可用 Vec/Box/String
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// 类型安全的页表操作（编译期防止错误标志组合）
mapper.map_to(page, frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE, allocator)?;
```

**验证**：分配释放 1000 帧后 free_count 复原；故意访问未映射地址 → page fault handler 打印诊断；`Vec<u8>` 堆分配正常工作

**学习价值**：物理页 vs 虚拟页的分离；4 级页表如何把虚拟地址翻译成物理地址；Rust trait 如何优雅地表达"分配器"这一抽象；`#[global_allocator]` 如何启用 Rust 标准集合类型。

---

## Phase 4：进程与线程调度

**目标**：多任务并发，轮转调度可见交错输出

**实现内容**：

**4A — 任务结构**：
```rust
pub struct Task {
    pub id: TaskId,
    pub name: &'static str,
    pub state: TaskState,          // Ready / Running / Blocked / Zombie
    pub context: TaskContext,      // 保存的寄存器（rsp, rip, callee-saved）
    pub kernel_stack: KernelStack, // 每任务独立 8KB 内核栈
    pub page_table: Option<PageTable>, // 用户进程有独立页表
}

pub enum TaskState { Ready, Running, Blocked(WaitReason), Zombie }
```

**4B — 调度器**：
1. 全局 `RoundRobinScheduler`：`VecDeque<Task>` 就绪队列
2. PIT IRQ handler 末尾调用 `scheduler.schedule()`
3. 时间片 = 1 tick（10ms）
4. `schedule()`：保存当前 context → 取下一个 Ready 任务 → 恢复其 context
5. 阻塞：`WaitQueue`（等待 IO / sleep / join）

**4C — 上下文切换**：
1. 纯 Rust `context_switch(prev: &mut TaskContext, next: &TaskContext)` — 使用 `global_asm!` 或 naked function
2. 保存/恢复：rbx, rbp, r12-r15, rsp, rip + 切换 CR3
3. `fork()`：深拷贝页表 + 栈 + context（子任务 rip=返回点, rax=0）
4. `exit()` / `wait()`：状态转 Zombie，父任务回收

**4D — 线程**：
1. `Thread`：同进程内共享页表，独立栈和 context
2. `thread::spawn(fn)` / `thread::join()`
3. `spin::Mutex` 保护共享数据

**Rust 特色**：
```rust
// 上下文切换用 naked function（Rust nightly）
#[naked]
extern "C" fn context_switch(old: *mut TaskContext, new: *const TaskContext) {
    unsafe {
        core::arch::asm!(
            "push rbx", "push rbp", "push r12", "push r13", "push r14", "push r15",
            "mov [rdi], rsp",       // 保存旧 rsp
            "mov rsp, [rsi]",       // 恢复新 rsp
            "pop r15", "pop r14", "pop r13", "pop r12", "pop rbp", "pop rbx",
            "ret",
            options(noreturn)
        )
    }
}
```

**验证**：3 个内核线程交错打印 `t1 #n / t2 #n / t3 #n` 各 5 轮；线程 join 正确阻塞；32 线程压力测试无死锁

**学习价值**：上下文切换的本质 = 保存/恢复寄存器 + 切栈 + 切页表；Rust 的 enum 状态机完美表达任务生命周期；轮转调度虽简单但揭示了时间片、就绪队列、阻塞队列的核心概念；所有权模型自然防止了"忘记释放任务"的 bug。

---

## Phase 5：内核态/用户态分离与系统调用

**目标**：用户程序运行在 ring 3，通过 syscall 请求内核服务

**实现内容**：
1. `kernel/src/gdt.rs` 扩展：添加 user code (0x1B) / user data (0x23) 段 + TSS（含 IST）
2. `kernel/src/syscall/mod.rs`：
   - 使用 `syscall`/`sysret` 指令（x86_64 标准）
   - 配置 MSR：STAR（段选择子）、LSTAR（handler 地址）、SFMASK（进入时关闭的 flags）
   - Handler：swapgs → 切内核栈 → 保存用户寄存器 → 分发 → 恢复 → swapgs → sysret
3. `kernel/src/syscall/table.rs`：
   ```rust
   pub enum Syscall {
       Exit = 1, Read = 3, Write = 4, Open = 5, Close = 6,
       Fork = 2, Exec = 11, Wait = 7, GetPid = 20,
       Brk = 45, Yield = 158, Mmap = 9,
   }
   ```
4. `kernel/src/process/elf.rs`：ELF64 加载器 — 解析 PT_LOAD 段，映射到用户地址空间
5. 跳转到 ring 3：构造 iretq 帧（SS=0x23, RSP, RFLAGS=0x202, CS=0x1B, RIP=entry）
6. `user/src/syscall.rs`：用户态 syscall 包装
   ```rust
   pub fn syscall0(nr: u64) -> i64 {
       let ret: i64;
       unsafe { core::arch::asm!("syscall", inlateout("rax") nr => ret, lateout("rcx") _, lateout("r11") _); }
       ret
   }
   ```
7. `user/src/hello.rs`：通过 syscall write 输出字符串

**验证**：用户态 hello 输出 "hello from user"；非法 syscall 号返回 -ENOSYS；用户态触发 page fault → 内核 kill 该进程不影响系统

**学习价值**：x86_64 的 `syscall/sysret` 是内核/用户态切换的最快路径；TSS 在 64 位的作用（存 IST + RSP0）；Rust 的 enum 让系统调用编号类型安全；ELF 格式是"程序如何变成进程"的桥梁。

---

## Phase 6：虚拟文件系统

**目标**：用 Rust trait 实现"一切皆文件"的统一抽象

**实现内容**：
1. `kernel/src/fs/vfs.rs` — 核心 trait：
   ```rust
   pub trait FileSystem: Send + Sync {
       fn name(&self) -> &str;
       fn root_inode(&self) -> Arc<dyn Inode>;
   }
   
   pub trait Inode: Send + Sync {
       fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>, FsError>;
       fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;
       fn write(&self, offset: usize, data: &[u8]) -> Result<usize, FsError>;
       fn stat(&self) -> FileStat;
       fn list_dir(&self) -> Result<Vec<String>, FsError> { Err(FsError::NotDir) }
   }
   ```
2. 路径解析：按 `/` split，逐级调用 `inode.lookup(name)`
3. 挂载表：`Vec<(String, Arc<dyn FileSystem>)>`，最长前缀匹配
4. 文件描述符表：per-process `Vec<Option<Arc<File>>>`（最多 64 个）
5. `kernel/src/fs/initrd.rs`：解析 tar（ustar 512 字节头），实现 `FileSystem` trait
6. `kernel/src/fs/devfs.rs`：`/dev/console`、`/dev/null`、`/dev/zero` — 每个设备实现 `Inode` trait
7. `kernel/src/fs/procfs.rs`：动态内容生成 `/proc/meminfo`、`/proc/uptime`
8. 将 read/write/open/close syscall 接到 VFS

**Rust 特色**：
```rust
// trait object 多态 — 运行时决定调用哪个文件系统的实现
let inode: Arc<dyn Inode> = vfs.resolve("/dev/console")?;
inode.write(0, b"hello")?;

// enum 表达错误类型 — 编译器确保所有情况被处理
pub enum FsError {
    NotFound, PermissionDenied, NotDir, IsDir, InvalidArg, NoSpace,
}
```

**验证**：用户态 `open("/dev/console") + write` → 串口输出；`open("/nope")` 返回 NotFound；`ls /initrd/` 列出打包文件

**学习价值**：VFS 是 Linux "一切皆文件"哲学的核心 — Rust 的 trait 系统天然适合表达这种"接口多态"；inode/file/mount 三层抽象；为什么 `/proc` 可以是动态生成的假文件系统；对比 C 的函数指针表，Rust trait 更安全更清晰。

---

## Phase 7：设备驱动

**目标**：键盘输入可用 + e1000 网卡收发数据

**实现内容**：
1. `kernel/src/drivers/keyboard.rs`：
   - IRQ1 handler → 读端口 0x60 → `pc-keyboard` crate 解析 scancode → ASCII
   - 环形缓冲 `VecDeque<u8>`（spin::Mutex 保护）
   - devfs 暴露 `/dev/kbd`（read 阻塞直到有按键）
2. `kernel/src/drivers/e1000.rs`：
   - PCI 配置空间扫描找 e1000（vendor=0x8086, device=0x100E）
   - 读 BAR0 获取 MMIO 基址，用 `volatile` 读写寄存器
   - 初始化：CTRL.RST → CTRL.SLU → 配置 RX/TX 描述符环（8 项）
   - RX：分配物理页作 DMA buffer，配置 RDBAL/RDLEN/RDT
   - TX：填描述符（addr + len + cmd），写 TDT 触发
   - IRQ handler：读 ICR 清中断，处理收到的包
3. 使用 Rust `volatile` 访问硬件寄存器（避免编译器优化掉 MMIO 读写）：
   ```rust
   #[repr(C)]
   struct E1000Regs {
       ctrl: Volatile<u32>,      // 0x0000
       status: Volatile<u32>,    // 0x0008
       // ...
       rctl: Volatile<u32>,      // 0x0100
       // ...
   }
   ```

**验证**：QEMU 窗口键盘输入 → 串口回显；e1000 link up + MAC 地址打印；ARP 请求网关收到回复

**学习价值**：MMIO vs Port IO 两种硬件交互方式；`volatile` 为什么在驱动中必不可少；PCI 配置空间与 BAR 的作用；DMA 描述符环的工作原理；Rust 的类型系统如何帮助正确表达硬件寄存器布局。

---

## Phase 8：网络栈 + Shell 集成

**目标**：完整网络协议栈 + 交互式 shell

**实现内容**：

**8A — 网络栈**（自底向上）：
1. 网络配置：硬编码 IP=10.0.2.15, GW=10.0.2.2（QEMU user-mode 默认）
2. `net/ethernet.rs`：以太网帧（14 字节头），按 EtherType 分发
3. `net/arp.rs`：ARP 缓存（`BTreeMap<Ipv4Addr, MacAddr>`）+ 请求/应答
4. `net/ip.rs`：IPv4 头 + checksum + 按 protocol 分发
5. `net/icmp.rs`：echo request → echo reply
6. `net/udp.rs`：端口 → socket 映射，sendto/recvfrom
7. `net/tcp.rs`（最小教学版）：
   - 用 Rust enum 表达状态机：
   ```rust
   enum TcpState {
       Closed, Listen, SynSent { .. }, SynReceived { .. },
       Established { .. }, FinWait1 { .. }, FinWait2 { .. },
       CloseWait { .. }, LastAck { .. }, TimeWait { .. },
   }
   ```
   - 固定 MSS=536，无拥塞控制，1 秒重传
8. `net/socket.rs`：BSD socket API，socket fd 融入 VFS（实现 Inode trait）

**8B — Shell**：
1. `tools/make_initrd.py`：打包用户程序为 initrd.tar
2. `user/src/shell.rs`：
   - 从 `/dev/kbd` 读行 → split_whitespace → 匹配命令
   - Builtin：help, ls, cat, echo, ps, free, ping, exit
   - 外部命令：fork + exec + wait
3. 内核启动 init 进程 → init 启动 shell
4. QEMU 启动参数加 `-netdev user,id=n0,hostfwd=tcp::1234-:1234 -device e1000,netdev=n0`

**验证命令矩阵**：
- 主机 `ping 10.0.2.15` → 收到 reply
- `echo hi | nc -u -w1 127.0.0.1 9999` → 串口打印收到数据
- Shell：`help` / `ls /` / `cat /proc/meminfo` / `ps` / `free` / `ping 10.0.2.2` / `echo hello`
- 连续运行 5 分钟无 panic

**学习价值**：协议分层 = 每层只关心自己的头和下一层的 payload；Rust enum 状态机让 TCP 状态转换一目了然（编译器强制处理所有状态）；socket 融入 fd 抽象实现"网络也是文件"；shell 本质是 fork-exec-wait 循环。

---

## 阶段依赖关系

```
Phase 0 (工具链)
  └→ Phase 1 (boot/hello)
       └→ Phase 2 (中断/GDT/IDT/PIT)
            └→ Phase 3 (PMM/VMM/heap)
                 └→ Phase 4 (进程/线程/调度)
                      └→ Phase 5 (用户态/syscall/ELF)
                           ├→ Phase 6 (VFS)
                           │    └→ Phase 7 (驱动: 键盘+网卡)
                           │         └→ Phase 8 (网络栈 + Shell)
                           └→ Phase 6-8 可适度并行（接口先冻结）
```

---

## 构建与运行

`Makefile`（便捷命令包装 cargo）：
```makefile
.PHONY: build run debug iso clean test

build:
	cargo bootimage --release

run: build
	qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/release/bootimage-simplelinux.bin \
		-m 128M -serial stdio -display gtk \
		-netdev user,id=n0,hostfwd=tcp::1234-:1234,hostfwd=udp::9999-:9999 \
		-device e1000,netdev=n0

debug: build
	qemu-system-x86_64 ... -s -S &
	gdb target/.../simplelinux -ex "target remote :1234"

clean:
	cargo clean
```

---

## Rust vs C 的教学优势总结

| 维度 | Rust 带来的改善 |
|---|---|
| 内存安全 | 编译器阻止 use-after-free、buffer overflow，减少内核 bug |
| 类型系统 | enum 状态机（TaskState、TcpState）让状态转换穷举，漏处理直接编译错误 |
| Trait 多态 | VFS / FrameAllocator / Device 用 trait 表达比 C 函数指针表更清晰安全 |
| 所有权 | 自然防止资源泄漏，Drop trait 自动清理 |
| 错误处理 | `Result<T, E>` 替代 C 的 int 返回码 + errno，`?` 操作符简化传播 |
| 工具链 | cargo 一键构建、依赖管理、测试；clippy lint 发现潜在问题 |
| 并发安全 | Send/Sync trait 在编译期防止数据竞争 |

**注意**：内核中仍有大量 `unsafe`（硬件交互、页表操作、上下文切换），但 Rust 将 unsafe 限制在最小范围，其余代码享受安全检查。

---

## 风险缓解

| 风险 | 缓解 |
|---|---|
| Rust nightly 特性变动（naked fn、asm!） | `rust-toolchain.toml` 锁定具体 nightly 版本 |
| bootloader crate 版本更新 break | Cargo.lock 锁定版本；备选：手写 multiboot2 头 |
| no_std 下缺少常用类型 | `alloc` crate 提供 Vec/Box/String；自写缺失部分 |
| 上下文切换需要 naked function | 备选：用单独 .S 文件 + `global_asm!` |
| e1000 驱动 volatile 寄存器布局错 | 严格对照 Intel 82540EM 手册偏移表；用 QEMU trace 验证 |
| 教学 Rust 学习曲线 | 每个 Phase 的 docs 附带 Rust 语法说明；代码重注释 |

---

## 预期工作量

| Phase | 核心代码量 | 预计时间 |
|---|---|---|
| 0 (工具准备) | 配置 ~50 行 | 15 分钟 |
| 1 (Boot) | ~300 行 Rust | 1-2 小时 |
| 2 (中断) | ~400 行 Rust | 2-3 小时 |
| 3 (内存) | ~600 行 Rust | 3-5 小时 |
| 4 (进程/调度) | ~700 行 Rust + ~50 行 asm | 4-6 小时 |
| 5 (用户态/syscall) | ~600 行 Rust + ~30 行 asm | 4-5 小时 |
| 6 (VFS) | ~700 行 Rust | 3-5 小时 |
| 7 (驱动) | ~500 行 Rust | 3-4 小时 |
| 8 (网络+Shell) | ~1200 行 Rust | 6-8 小时 |
| **总计** | **~5000 行 Rust + ~80 行 asm** | **27-38 小时** |

注：Rust 代码量比 C 少约 30%（类型系统减少样板代码，crate 生态提供基础设施），但单行表达力更强。

---

## 未采用方案（Rejected Alternatives）

| 方案 | 拒绝原因 |
|---|---|
| C 语言实现 | 用户选择 Rust；Rust 生态有更好的 OS 开发 crate |
| i386 (32-bit) target | Rust 无官方 `i386-unknown-none` target；需自定义 target JSON + 手写引导代码；x86_64 有更好的 crate 支持 |
| 手写 bootloader | `bootloader` crate 已经处理了所有复杂性，教学重心应在 OS 概念而非引导细节 |
| UEFI boot | 比 BIOS/Multiboot 更复杂，QEMU BIOS 模式对教学更友好 |
| smoltcp crate 替代自写网络栈 | 虽然省事但失去了理解协议分层的教学价值；自写更符合学习目标 |
| async/await 调度器 | Rust async 在 no_std 内核中过于复杂，轮转调度更适合教学 |
