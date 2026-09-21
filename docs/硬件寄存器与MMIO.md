# 硬件寄存器、MMIO 与设备通信

本文整理 SimpleLinux 学习过程中关于 CPU、设备寄存器、MMIO、DMA、PCIe 路由和硬件异步协同的问答，重点帮助理解：驱动如何控制芯片，以及这些底层能力如何连接到 Linux 和 Kubernetes。

## 1. CPU 寄存器与设备寄存器

CPU 和外设芯片都有自己的寄存器，但职责不同：

```text
CPU 寄存器：CPU 执行指令时使用
设备寄存器：设备芯片控制和报告自身状态
```

CPU 寄存器包括通用寄存器、指令指针、栈指针和控制寄存器等。设备寄存器则可能包括控制、状态、队列地址、中断和少量数据寄存器。

寄存器通常保存短小的信息，例如：

- 控制位：复位、启用、中断开关
- 状态位：设备是否就绪、链路是否连接、操作是否完成
- 地址和长度：DMA 描述符环地址、数据长度
- 索引：发送队列尾指针、接收队列头指针
- 少量数据：UART 字节、键盘扫描码

寄存器本身通常不负责访问大量内存。它们保存地址、数据或控制信息，真正的访问由 CPU 指令或设备内部的 DMA 引擎完成。

## 2. 设备寄存器如何映射到 CPU 地址空间

设备通过 PCI/PCIe 的 BAR（Base Address Register）声明一段地址范围。例如：

```text
BAR0 = 0xf0000000
范围 = 0xf0000000 ~ 0xf000ffff
```

系统把这段物理地址分配给设备，内核再把它映射到自己的虚拟地址空间：

```text
内核虚拟地址
  ↓ MMU 页表转换
物理地址
  ↓ PCIe 总线地址路由
设备 BAR
  ↓ BAR 内偏移译码
具体设备寄存器
```

假设网卡的寄存器基址为 `0xf0000000`：

```text
BAR0 + 0x0000 → CTRL
BAR0 + 0x0008 → STATUS
BAR0 + 0x3818 → TDT（发送队列尾指针）
```

MMIO（Memory-Mapped I/O）不是地址转换器，而是一种地址布局和设备访问机制：设备寄存器被安排在 CPU 可访问的物理地址范围内，CPU 使用普通的 load/store 指令读写这些地址。

- MMU：负责虚拟地址到物理地址的转换
- MMIO：让设备寄存器出现在物理地址空间中
- PCIe/SoC 总线：根据物理地址把请求路由到设备
- 设备内部译码逻辑：根据 BAR 内偏移选择具体寄存器

设备寄存器不是普通 RAM。写入可能触发复位、启动 DMA 或推进队列；读取可能返回实时状态，甚至清除中断状态。因此驱动通常使用 `read_volatile` 和 `write_volatile`。

`volatile` 只保证编译器不会删除或合并访问，并不自动解决 CPU、总线和 DMA 的内存顺序问题。驱动仍需遵守硬件手册规定的访问顺序，并在需要时使用内存屏障和 DMA 一致性处理。

## 3. MMIO、Port I/O 与 DMA

设备通信中有三种容易混淆的机制：

```text
MMIO：CPU ↔ 设备寄存器
Port I/O：CPU ↔ x86 独立 I/O 端口
DMA：设备 ↔ 系统 RAM
```

### MMIO

CPU 通过映射后的地址访问设备寄存器：

```rust
core::ptr::write_volatile(register_address, value);
let value = core::ptr::read_volatile(register_address);
```

### Port I/O

x86 还提供独立的 I/O 地址空间和 `in`/`out` 指令。SimpleLinux 的 PS/2 键盘驱动从端口 `0x60` 读取扫描码：

```rust
Port::new(0x60).read()
```

这不是 MMIO。

### DMA

大量数据不适合通过设备寄存器逐字节传输。驱动会在普通 RAM 中准备数据和描述符，再通过 MMIO 寄存器告诉设备它们的位置：

```text
驱动在 RAM 中填写描述符
  ↓
驱动通过 MMIO 写入描述符环地址
  ↓
设备内部 DMA 引擎读取 RAM
  ↓
设备发送或接收大量数据
```

所以，设备寄存器一般保存 DMA 地址和控制信息，而 DMA 引擎才是真正访问系统内存的硬件逻辑。

## 4. 网卡发送的一次完整过程

以 e1000 网卡发送数据为例：

```text
1. CPU/驱动在普通 RAM 中准备数据包
2. CPU/驱动在 RAM 中填写发送描述符
3. 驱动确保描述符对设备可见
4. CPU 通过 MMIO 写入网卡的发送队列尾指针
5. 网卡读取寄存器中的队列信息
6. 网卡通过 DMA 读取描述符
7. 网卡通过 DMA 读取数据包
8. 网卡发送 Ethernet frame
9. 网卡更新描述符状态
10. 驱动通过轮询或中断发现发送完成
```

关键分工是：

```text
寄存器：控制、状态、地址、索引
RAM：描述符和数据包
DMA：设备对 RAM 的直接访问
网卡状态机：执行实际收发流程
```

SimpleLinux 的 [e1000 驱动](../kernel/src/drivers/e1000.rs)展示了 PCI 发现、BAR/MMIO 映射、芯片复位、DMA 描述符环和轮询式收发。

## 5. CPU 到设备寄存器的 PCIe 路由

一次网卡 MMIO 访问可以拆成以下阶段：

```text
CPU 指令
  ↓
MMU/TLB：虚拟地址转换与权限检查
  ↓
CPU 内存访问单元：判断内存类型并排队
  ↓
Root Complex：CPU 域与 PCIe 域的桥接
  ↓
PCIe Transaction Layer：生成 TLP
  ↓
PCIe 链路层/物理层：传输、校验、流控、重传
  ↓
PCIe Switch 或设备直连端口
  ↓
设备 BAR 地址匹配
  ↓
设备内部寄存器偏移译码
  ↓
具体寄存器连接的硬件状态机
```

### 5.1 CPU 和 MMU

驱动使用的是内核虚拟地址，例如：

```text
0xffff8000f0000008
```

MMU 根据 `CR3` 指向的页表，将其转换为物理地址，并检查页表权限和内存类型。设备 MMIO 映射通常使用设备内存属性，避免把设备寄存器当作普通 RAM 缓存。

### 5.2 Root Complex

Root Complex 根据物理地址判断访问目标：

```text
RAM 地址范围       → 内存控制器
网卡 BAR 地址范围  → PCIe 设备域
NVMe BAR 地址范围  → PCIe 设备域
```

它把 CPU 的内存读写转换成 PCIe 事务，并负责追踪读请求以及接收设备返回的 Completion。

### 5.3 PCIe 事务

写操作通常生成 Memory Write TLP：

```text
CPU → Root Complex → Memory Write TLP → 设备
```

读操作需要设备返回数据：

```text
CPU → Memory Read TLP → 设备
CPU ← Completion TLP ← 设备
```

PCIe 内部还会处理序号、CRC、ACK/NAK、信用流控、重传和链路训练。它不是一组简单的并行电线，而是带有协议状态机和缓冲队列的点对点互连。

### 5.4 设备内部译码

网卡收到访问后，会检查目标地址是否落在自己的 BAR 范围，再计算偏移：

```text
目标地址 - BAR0 = 寄存器偏移
0xf0003818 - 0xf0000000 = 0x3818
```

设备内部将 `0x3818` 译码为 `TDT`，然后把写入值交给发送队列控制逻辑，可能进一步唤醒 DMA 状态机。

## 6. 这些环节是否都是异步的

可以把它们理解为多个具有独立状态、缓冲区、队列和处理逻辑的硬件模块。但更准确的描述是：

```text
并行推进 + 局部异步 + 协议约束 + 必要同步
```

各部分的角色不同：

- CPU：执行指令并发起访问
- MMU/TLB：完成地址转换和权限检查
- Root Complex：转换和管理 PCIe 事务
- PCIe 链路：传输、校验、重传和流控
- 设备寄存器逻辑：解析访问并改变设备配置
- DMA 引擎：读写系统 RAM
- 设备状态机：并行执行收发、队列和链路操作
- 中断控制器：在设备需要 CPU 处理时发出通知

MMIO 写通常是 posted write。CPU 发出写请求后，通常不等待设备的业务操作完成：

```text
CPU 指令完成 ≠ 设备动作完成
```

MMIO 读需要设备返回 Completion，因此会形成较强的等待点，但“读寄存器完成”也不等于设备的整个业务操作完成。驱动还要检查 `DONE`、`READY`、`ERROR` 或 DMA 描述符状态。

设备完成工作后可以：

```text
更新描述符状态 → CPU 轮询
```

也可以：

```text
更新状态 → 触发 MSI/MSI-X → CPU 执行中断处理程序
```

## 7. 同步、可见性和访问顺序

设备和 CPU 并行运行，但访问必须遵守协议顺序。例如发送网卡数据时：

```text
1. 先填写 RAM 中的 DMA 描述符
2. 确保描述符写入对设备可见
3. 再写 MMIO 门铃或队列尾指针
4. 设备读取有效描述符
```

如果 CPU 在描述符写入真正对设备可见之前就通知网卡，网卡可能读到旧数据。

因此驱动可能需要：

- 编译器屏障
- CPU 内存屏障
- DMA 一致性处理
- 合适的 MMIO 内存属性
- 按硬件手册进行寄存器读回
- 轮询状态或等待中断

寄存器地址到设备寄存器的对应关系通常稳定，但寄存器内容是硬件状态的某个时刻快照。设备可以在 CPU 读取前后改变状态，不能把它当作与 CPU 共享的普通变量。

## 8. 与 SimpleLinux 代码的对应关系

当前项目已经包含这条链路的教学切片：

```text
PCI 配置空间发现 e1000
  → 读取 BAR0
  → 映射 MMIO
  → 配置寄存器
  → 分配 DMA 描述符和缓冲区
  → 轮询发送/接收
  → 交给 Ethernet/IP/TCP/UDP 网络栈
```

相关代码：

- [drivers/mod.rs](../kernel/src/drivers/mod.rs)：驱动模块入口
- [drivers/e1000.rs](../kernel/src/drivers/e1000.rs)：PCI、BAR、MMIO、DMA 和网卡收发
- [drivers/keyboard.rs](../kernel/src/drivers/keyboard.rs)：Port I/O、IRQ 和字符缓冲
- [fs/devfs.rs](../kernel/src/fs/devfs.rs)：将设备能力暴露为 `/dev/kbd` 等设备文件
- [fs/vfs.rs](../kernel/src/fs/vfs.rs)：通过 trait 把不同底层实现统一到 VFS 接口

## 9. 与 Kubernetes 的边界

Kubernetes 通常不会直接访问 PCI 寄存器。完整链路更接近：

```text
物理芯片
  ↓
Linux 硬件驱动
  ↓
Linux 网络/块设备/字符设备子系统
  ↓
CNI、CSI、Device Plugin、容器运行时
  ↓
Pod
```

### 网络

```text
物理网卡
  ↓ NIC 驱动
Linux net_device
  ↓
network namespace、veth、bridge、路由、eBPF
  ↓ CNI
Pod 网络
```

CNI 负责配置 Pod 网络，不是网卡驱动。

### 存储

```text
磁盘/NVMe/virtio-blk
  ↓ 块设备驱动
Linux block layer
  ↓ 文件系统和挂载
CSI plugin
  ↓ kubelet
Pod volume
```

CSI 负责把卷的创建、挂载、卸载等生命周期接入 Kubernetes，不负责替代 NVMe 或磁盘驱动。

### 设备插件

Device Plugin 负责把节点上的 GPU、FPGA、RDMA 设备等注册为可调度资源，并在 Pod 使用时协助 kubelet 将设备注入容器。它通常不负责：

- 编写 PCI 驱动
- 初始化芯片寄存器
- 实现 DMA
- 实现网络协议
- 替代 kubelet

可以这样区分：

```text
Linux 驱动：让内核能够控制硬件
设备节点：向用户态暴露设备访问入口
Device Plugin：向 kubelet 暴露可分配设备资源
kubelet/运行时：把设备注入容器
Pod：使用已经暴露的设备
```

## 10. 一句话总结

> CPU 通过 MMIO 或 Port I/O 访问设备寄存器；MMU 负责虚拟地址转换，Root Complex 和 PCIe 拓扑负责把物理地址路由到设备，设备再通过内部寄存器译码选择硬件逻辑。寄存器负责命令、状态和地址，DMA 负责大量内存数据传输，CPU、PCIe 和设备状态机通过队列、握手、完成消息和中断并行协作。
