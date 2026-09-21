有，而且现代分布式存储大量采用这类技术。不过要先区分：

```text
用户态 I/O      = 减少内核参与和上下文切换
零拷贝          = 减少数据在不同内存缓冲区之间复制
内核旁路        = 用户态直接访问设备或网络队列
```

它们不是完全相同的概念。

## 1. 传统路径

以 Ceph RBD 写入为例，传统路径可能是：

```text
应用 buffer
  ↓ copy
内核 Socket buffer
  ↓ TCP/IP
网卡 DMA
  ↓
远端内核 Socket buffer
  ↓ copy
OSD 用户态 buffer
  ↓ copy 或写入缓存
内核块层
  ↓
NVMe 驱动
  ↓ DMA
SSD
```

数据可能经历：

- 用户态到内核态复制
- 内核网络缓冲区到 OSD 缓冲区复制
- OSD 缓冲区到文件系统页缓存复制
- 多次上下文切换
- 多次协议栈处理
- 网卡和 NVMe 各自的 DMA

注意，CPU 不一定搬运每个字节，很多环节由 DMA 完成，但仍可能发生“DMA 到一个缓冲区，再复制到另一个缓冲区”。

## 2. 用户态 I/O

用户态 I/O 的基本思想是：

```text
用户态存储进程
  ↓
直接管理 I/O 队列
  ↓
NVMe / RDMA / 网卡
```

用户态程序通过库直接操作设备队列、共享内存和完成队列，而不是每次 I/O 都经过内核系统调用路径。

典型技术包括：

- SPDK
- DPDK
- SPDK NVMe-oF
- RDMA verbs
- Seastar
- Ceph Crimson
- 用户态 virtio
- VPP、用户态网络栈

### SPDK

SPDK 让用户态程序直接管理 NVMe：

```text
用户态存储程序
  ↓
SPDK NVMe driver
  ↓
PCIe BAR/MMIO
  ↓
NVMe Submission Queue
  ↓
NVMe DMA
  ↓
SSD
```

CPU 负责：

```text
填写 NVMe 命令
更新队列 tail
轮询 Completion Queue
```

不再频繁经过：

```text
write()
  ↓
VFS
  ↓
block layer
  ↓
内核 NVMe driver
```

这会减少：

- syscall
- 上下文切换
- 内核锁竞争
- 通用块层开销
- 中间缓冲区

但设备仍然通过 PCIe、MMIO 和 DMA 工作。用户态并没有绕过硬件，只是绕过了通用内核数据路径。

## 3. 用户态 NVMe 的完整流程

```text
1. 用户态程序申请大页或锁页内存
2. 将数据放入 DMA 可访问的缓冲区
3. 用户态 SPDK 填写 NVMe 命令
4. 写 MMIO doorbell
5. NVMe 控制器 DMA 读取命令和数据
6. SSD 完成 NAND 操作
7. NVMe DMA 写 Completion Queue
8. 用户态轮询 Completion Queue
9. 请求完成
```

这里仍然有数据流：

```text
用户态 DMA buffer ↔ NVMe 控制器 ↔ NAND
```

如果应用数据一开始就在 DMA buffer 中，就可以避免额外复制。

## 4. 零拷贝到底是什么

零拷贝并不意味着“数据从不移动”，而通常是：

> 数据不在 CPU 管理的多个软件缓冲区之间重复复制。

例如传统路径：

```text
应用 buffer
  ↓ copy
内核 buffer
  ↓ DMA
网卡
```

零拷贝路径：

```text
应用 DMA buffer
  ↓ DMA
网卡
```

或者网络到存储：

```text
网卡 DMA → 用户态/共享 DMA buffer → NVMe DMA
```

中间数据仍可能被设备 DMA 读取或写入，但 CPU 不需要把它复制到另一个缓冲区。

## 5. 分布式存储中的网络零拷贝

如果使用 RDMA，路径可以进一步简化：

```text
发送端应用 buffer
  ↓ 注册为 RDMA 内存
RDMA NIC DMA
  ↓
网络
  ↓
接收端 RDMA NIC DMA
  ↓
接收端应用 buffer
```

传统 TCP 路径通常是：

```text
应用 buffer
  ↓
内核 TCP buffer
  ↓
网卡 DMA
  ↓
网络
  ↓
接收端内核 TCP buffer
  ↓
应用 buffer
```

RDMA 可以减少：

- 内核 TCP/IP 协议栈参与
- 内核和用户态之间的复制
- 上下文切换
- CPU 协议处理

典型技术：

- InfiniBand
- RoCE
- iWARP
- RDMA verbs
- NVMe over Fabrics
- Ceph RDMA messenger

## 6. NVMe-oF

NVMe-oF 是把 NVMe 命令通过网络发送到远程 NVMe 设备。

常见传输：

```text
NVMe over TCP
NVMe over RDMA
NVMe over Fibre Channel
```

### NVMe over TCP

```text
客户端 NVMe-oF
  ↓
TCP/IP
  ↓
网卡 DMA
  ↓
网络
  ↓
目标端网卡
  ↓
TCP/IP
  ↓
NVMe target
  ↓
本地 NVMe
```

兼容性好，但内核和 TCP 协议栈开销较大。

### NVMe over RDMA

```text
客户端应用/存储栈
  ↓
RDMA queue pair
  ↓
RDMA NIC DMA
  ↓
网络
  ↓
目标端 RDMA NIC DMA
  ↓
NVMe target
  ↓
本地 NVMe
```

延迟更低，CPU 开销更小，但需要：

- RDMA 网卡
- 高质量网络
- 内存注册
- 更复杂的故障处理
- 严格的网络配置

## 7. Ceph 是否使用这些技术

Ceph 的传统实现主要是：

```text
用户态 OSD
  ↓
Linux 网络栈
  ↓
Linux 块层
  ↓
内核驱动
```

但 Ceph 可以使用：

- RDMA messenger
- DPDK
- SPDK
- io_uring
- 用户态 NVMe
- BlueStore
- Crimson/Seastar

### BlueStore

BlueStore 不完全等于用户态 I/O，但它减少了传统文件系统路径：

```text
传统：
OSD → ext4/XFS → page cache → block layer → NVMe

BlueStore：
OSD → RocksDB 元数据
     → 原始块设备
     → block layer/NVMe
```

它避免把对象数据再交给通用文件系统处理，减少了一层缓存和元数据开销。

### Ceph Crimson

Ceph Crimson 基于 Seastar，倾向于：

```text
用户态事件循环
  ↓
用户态网络栈
  ↓
用户态 NVMe/SPDK
```

它追求：

- 每个 CPU 核心独占队列
- 少锁或无锁
- 异步编程
- NUMA 感知
- 减少线程切换
- 减少内核路径

可以理解为更彻底地把数据面变成：

```text
CPU core
  ↓
用户态网络队列
  ↓
用户态存储队列
  ↓
DMA
```

## 8. io_uring 属于哪一类

`io_uring` 不是完全绕过内核，而是：

```text
用户态共享 Submission Queue
  ↓
内核异步消费
  ↓
设备驱动
  ↓
Completion Queue
```

应用和内核共享环形队列：

```text
应用填写 SQE
  ↓
通知内核
  ↓
内核批量提交 I/O
  ↓
内核填写 CQE
  ↓
应用读取完成事件
```

它减少：

- 每次 I/O 的 syscall
- 上下文切换
- 请求提交开销

如果配合：

- registered buffers
- fixed files
- direct I/O
- buffer selection
- `O_DIRECT`

还可以减少复制和页缓存开销。

但它仍然经过内核，因此和 SPDK 的定位不同：

```text
io_uring：
  高效异步内核 I/O

SPDK：
  用户态、内核旁路 I/O
```

## 9. 用户态 I/O 仍然需要内核做什么

用户态 I/O 不等于完全不需要内核。内核通常仍负责：

- PCIe 设备枚举
- IOMMU 配置
- DMA 权限隔离
- 中断和 MSI-X 配置
- 内存页固定与注册
- 设备初始化
- CPU 调度
- cgroup 资源限制
- mount namespace
- 进程隔离
- 故障和权限管理

用户态程序也不能随意访问物理设备，通常需要：

```text
VFIO
IOMMU
hugepage
锁页内存
设备绑定
权限配置
```

典型流程：

```text
内核发现 PCIe 设备
  ↓
设备绑定到 vfio-pci
  ↓
IOMMU 建立 DMA 隔离
  ↓
用户态存储程序 mmap BAR
  ↓
用户态访问设备寄存器
```

这和你前面理解的 MMIO 完全对应：

```text
用户态虚拟地址
  ↓
MMU
  ↓
设备 BAR 的物理地址
  ↓
PCIe
  ↓
设备寄存器
```

区别只是发起访问的代码从内核驱动变成了用户态驱动。

## 10. 零拷贝的限制

“零拷贝”通常是相对概念，不是绝对没有复制。

### 仍可能发生的复制

```text
应用 buffer → 加密引擎 buffer
协议封装 → 对齐 buffer
压缩前 buffer → 压缩后 buffer
纠删码输入 → 编码输出
副本传输 → 不同 NUMA 节点 buffer
```

分布式存储尤其难以做到完全零拷贝，因为它可能还要执行：

- 副本复制
- 校验和
- 加密
- 压缩
- 纠删码
- 重试
- 日志写入
- 协议封装

例如 Ceph 写入可能需要：

```text
应用数据
  ↓
计算 checksum
  ↓
复制给副本
  ↓
写日志
  ↓
写数据
  ↓
纠删码编码
```

这些操作即使不发生传统 `memcpy`，也可能需要读取和重新生成数据。

所以更准确的目标是：

```text
尽量减少不必要的 CPU memcpy
尽量减少上下文切换
让 DMA 直接访问合适的缓冲区
让数据在 NUMA 本地流动
让网络和存储队列高效衔接
```

## 11. 一条高性能分布式存储数据流

以用户态 Ceph/分布式块存储为例：

```text
应用
  ↓
用户态存储引擎
  ↓
预注册 DMA buffer
  ↓
用户态网络栈
  ↓
DPDK/RDMA NIC queue
  ↓
网卡 DMA
  ↓
PCIe TLP
  ↓
交换机
  ↓
远端网卡 DMA
  ↓
远端用户态 OSD
  ↓
SPDK NVMe queue
  ↓
NVMe MMIO doorbell
  ↓
PCIe
  ↓
NVMe DMA
  ↓
SSD
```

完成路径：

```text
SSD Completion Queue
  ↓ DMA
远端用户态 OSD 轮询
  ↓ RDMA/TCP response
网卡 DMA
  ↓
客户端网卡 DMA
  ↓
客户端用户态 completion queue
  ↓
应用收到完成事件
```

这里 CPU 主要负责：

```text
构造描述符
更新 doorbell
处理状态
运行协议和副本逻辑
```

设备主要负责：

```text
DMA
队列处理
网络收发
NAND 操作
中断或完成队列更新
```

## 12. 性能和代价

| 技术 | 减少的开销 | 主要代价 |
|---|---|---|
| `io_uring` | syscall、上下文切换 | 仍经过内核 |
| `O_DIRECT` | page cache 复制 | 对齐和应用缓存管理复杂 |
| DPDK | 内核网络栈、系统调用 | 独占 CPU、内存和网卡 |
| SPDK | 内核块层和驱动开销 | 用户态管理设备，隔离复杂 |
| RDMA | TCP 栈和数据复制 | 网络、配置和故障处理复杂 |
| NVMe-oF | 远程访问 NVMe | 依赖网络低延迟和可靠性 |
| BlueStore | 通用文件系统缓存/元数据 | 存储系统自己管理更多逻辑 |
| 用户态 OSD | 锁、调度、拷贝 | 用户态故障可能影响服务，工程复杂 |

## 13. 和你的 SimpleLinux 工程如何对应

你当前项目的 e1000 驱动路径是：

```text
内核驱动
  ↓
MMIO 寄存器
  ↓
DMA 描述符
  ↓
网卡
```

如果模拟一个用户态高性能存储引擎，需要新增的抽象大致是：

```text
共享 DMA buffer
  ↓
Submission Queue
  ↓
Completion Queue
  ↓
用户态轮询
  ↓
异步 I/O future/event
```

可以按这个教学顺序扩展：

```text
1. 内存块设备
2. 异步提交队列
3. 完成队列
4. DMA 描述符
5. 用户态/内核态共享环
6. virtio-blk
7. virtio-net
8. 用户态网络协议
9. 两节点副本写入
10. quorum 和故障恢复
```

这会把当前的：

```text
Inode::read/write
```

逐渐扩展成：

```text
FileSystem
  ↓
BlockDevice
  ↓
AsyncRequestQueue
  ↓
DMA
  ↓
DeviceCompletion
```

## 总结

有，主流高性能分布式存储确实会结合：

```text
用户态 I/O
内核旁路
零拷贝
DMA
DPDK
SPDK
RDMA
io_uring
NVMe-oF
```

但它们优化的目标不同：

```text
用户态 I/O：
  减少内核路径和上下文切换

零拷贝：
  减少软件缓冲区之间的复制

DMA：
  让设备直接读写内存

RDMA：
  让远端网卡直接读写注册内存

SPDK：
  让用户态直接管理 NVMe 队列

DPDK：
  让用户态直接管理网络队列
```

最理想的数据路径是：

```text
应用预先准备的 DMA buffer
  ↓
用户态网络/存储队列
  ↓
网卡或 NVMe DMA
  ↓
PCIe
  ↓
远端节点或 SSD
```

但分布式副本、校验、加密、压缩、纠删码和故障恢复仍然需要 CPU 处理。因此实际目标不是“完全不搬运数据”，而是：

> **减少不必要的复制，让数据尽量直接在应用缓冲区、网卡 DMA、存储 DMA 和远端副本之间流动。**