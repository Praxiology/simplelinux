我会先把工程里现有的存储相关边界定位清楚：它目前实现的是 VFS、initrd 和内存文件系统式抽象，还是已经触及块设备；然后以这条边界为起点，对照 Kubernetes 中主流的本地盘、Ceph、云盘 CSI、分布式文件系统和对象存储，逐层追踪从 Pod 到 CPU、PCIe、DMA、磁盘介质的异步数据流。

Read 

Read 

Read 

Read , lines 1 to 240

可以。这个问题的关键是把 Kubernetes 存储拆成两条链：

```text
控制面链路：Kubernetes/CSI 决定“卷在哪里、如何挂载”
数据面链路：业务数据真正从 Pod 流到磁盘、网络和副本
```

你这个 SimpleLinux 工程目前实现的是数据面最前面的简化部分：

```text
用户程序
  ↓ syscall
VFS / fd
  ↓
initrd 内存文件
```

它还没有：

```text
文件系统 → 块设备 → PCIe → 磁盘控制器 → 持久化介质
```

因此，理解 Kubernetes 分布式存储时，可以把当前工程的 `Inode`、`FileSystem`、`read/write` 看成最上层入口，再向下补齐块设备、DMA、PCIe 和远程复制。

---

# 一、从 SimpleLinux 到真实存储系统

当前工程的读路径大致是：

```text
用户程序
  ↓ int 0x80
sys_read()
  ↓
VFS fd 表
  ↓
Inode::read()
  ↓
Initrd 内存中的 Vec<u8>
```

相关实现见：

- syscall/mod.rs
- fs/vfs.rs
- fs/initrd.rs

`initrd` 在启动时把 tar 文件解析成内存中的 inode 树：

```text
initrd.tar
  ↓ 启动时解析
InodeNode
  ↓
Vec<u8>
  ↓
read()
```

所以它没有真实的：

- 块设备
- 磁盘扇区
- I/O 请求队列
- DMA
- 磁盘控制器寄存器
- 文件系统日志
- 崩溃恢复
- 副本一致性

真实 Kubernetes 存储会把这条链扩展为：

```text
Pod
  ↓
用户态程序
  ↓
系统调用
  ↓
容器 mount namespace
  ↓
VFS
  ↓
具体文件系统
  ↓
块层或网络文件系统
  ↓
块设备驱动 / 网络驱动
  ↓
PCIe / virtio / SoC 总线
  ↓
存储控制器或远程存储服务
  ↓
磁盘介质 / 多副本节点
```

---

# 二、一次本地磁盘写入的底层链路

先不考虑分布式，只看 Kubernetes 节点本地 SSD。

业务执行：

```c
write(fd, buffer, 4096);
```

## 1. Pod 发起系统调用

```text
应用程序
  ↓
write(fd, buffer, length)
  ↓
CPU 执行 syscall/sysenter
  ↓
内核系统调用入口
```

在你的项目中是：

```text
用户态
  ↓ int 0x80
__syscall_entry
  ↓
syscall_dispatch
  ↓
sys_write()
```

Linux 中的机制更复杂，但概念相同。

CPU 此时会使用自己的寄存器：

```text
RAX：系统调用号或返回值
RDI：fd
RSI：buffer 地址
RDX：长度
RIP：当前指令地址
RSP：用户栈
CR3：当前地址空间页表
```

## 2. MMU 验证用户缓冲区

CPU 不能直接相信用户提供的虚拟地址。内核需要确认：

```text
buffer 地址是否映射？
是否属于当前进程？
是否可读？
是否跨越非法页？
```

MMU 通过：

```text
虚拟地址
  ↓ TLB
页表
  ↓
物理页
```

完成地址转换和权限检查。

例如：

```text
用户虚拟地址 0x7f00_1000
        ↓
物理页      0x1234_5000
```

之后内核可能把用户数据复制到 page cache，或者通过零拷贝/固定页机制直接用于 DMA。

## 3. VFS 和具体文件系统处理写入

VFS 根据文件描述符找到：

```text
进程 fd
  ↓
struct file
  ↓
inode
  ↓
具体文件系统
```

例如：

```text
ext4
XFS
Btrfs
```

文件系统负责把：

```text
文件偏移 1 MiB
```

转换成：

```text
逻辑文件块
  ↓
文件系统逻辑块
  ↓
底层块设备扇区
```

例如：

```text
文件偏移 1 MiB
  ↓
文件系统逻辑块 262144
  ↓
块设备扇区 2097152
```

此时数据通常先进入 page cache：

```text
用户 buffer
  ↓
内核 page cache
```

`write()` 返回，不一定代表数据已经落到 SSD。

```text
write() 返回
≠
数据已经持久化
```

只有在 `fsync()`、O_SYNC 或相应存储语义下，系统才会等待更强的持久化完成条件。

---

# 三、从文件系统到块设备

文件系统提交一个块 I/O 请求：

```text
写入逻辑块 1000
数据地址 = 内存页 P
长度 = 4096
```

Linux block layer 会把请求放入块设备队列：

```text
文件系统
  ↓
Block layer
  ↓
Request queue
  ↓
设备驱动
```

现代存储设备常见的是多队列：

```text
CPU core 0 → software queue 0
CPU core 1 → software queue 1
CPU core 2 → software queue 2
                 ↓
             hardware queue
```

这就是异步化的第一个重要位置：

```text
文件系统提交请求
  ↓
请求排队
  ↓
驱动稍后提交给控制器
  ↓
设备稍后完成
```

CPU 不会为了每个磁盘字节一直忙等。

---

# 四、NVMe SSD 的寄存器、DMA 和 PCIe

以物理机上的 NVMe SSD 为例。

## 1. NVMe 控制器由 PCIe 发现

启动时：

```text
PCIe 枚举
  ↓
发现 NVMe Vendor ID / Device ID
  ↓
读取 BAR
  ↓
分配 MMIO 地址
  ↓
启用 Memory Space 和 Bus Master
```

系统可能得到：

```text
NVMe BAR0 = 0xf1000000
```

NVMe 寄存器可能包括：

```text
CAP：能力
CC：控制器配置
CSTS：控制器状态
AQA：管理队列大小
ASQ：管理提交队列地址
ACQ：管理完成队列地址
SQ0TDBL：提交队列门铃
CQ0HDBL：完成队列门铃
```

这些是设备寄存器，不是 CPU 寄存器。

## 2. 驱动建立队列

驱动在普通 RAM 中分配：

```text
Submission Queue
Completion Queue
数据缓冲区
```

例如：

```text
提交队列 SQ：
+-----------------------------+
| 命令 0：读 LBA 1000         |
| 命令 1：写 LBA 2000         |
| 命令 2：读 LBA 3000         |
+-----------------------------+

完成队列 CQ：
+-----------------------------+
| 命令 0 完成                 |
| 命令 1 错误                 |
+-----------------------------+
```

队列本身在 RAM 中，不在 NVMe 寄存器中。

驱动通过 MMIO 告诉 NVMe：

```text
提交队列物理地址在哪里
完成队列物理地址在哪里
队列有多大
```

## 3. 提交写请求

文件系统把数据准备在内存中：

```text
RAM buffer = 0x4000_0000
长度 = 4096
目标 LBA = 2000
```

驱动在 Submission Queue 中填写命令：

```text
opcode = WRITE
PRP1   = 0x4000_0000
LBA    = 2000
length = 4096
```

其中 `PRP1` 是 NVMe 用来定位数据缓冲区的物理地址信息。

然后驱动通过 MMIO 写门铃：

```text
MMIO: SQ Tail Doorbell = 5
```

完整链路：

```text
CPU store 指令
  ↓
MMU 虚拟地址转物理地址
  ↓
Root Complex
  ↓
PCIe Memory Write TLP
  ↓
NVMe 控制器寄存器
  ↓
NVMe 看到 SQ tail 变化
```

这次 MMIO 写入本身可能很快完成，但 SSD 真正写完还需要很多步骤。

## 4. NVMe 通过 DMA 读取命令和数据

NVMe 控制器开始主动工作：

```text
NVMe 控制器
  ↓ DMA
读取 Submission Queue
  ↓
解析 WRITE 命令
  ↓ DMA
读取 RAM 中的数据 buffer
  ↓
写入 NAND Flash
```

这里的 DMA 路径是：

```text
NVMe 控制器
  ↓ PCIe DMA Read
Root Complex
  ↓
系统内存控制器
  ↓
DRAM
```

这不是 CPU 通过 MMIO 逐字节搬运。

CPU 只做：

```text
填写命令
写门铃
等待完成
```

## 5. SSD 内部继续异步工作

SSD 内部还会执行：

```text
Flash Translation Layer
  ↓
逻辑块地址 LBA → NAND 物理页
  ↓
磨损均衡
垃圾回收
纠错码 ECC
坏块管理
写放大控制
NAND program 操作
```

因此“写入 LBA 2000”并不是简单地把一个内存地址复制到固定闪存地址。

SSD 内部可能包含：

```text
PCIe 接口逻辑
NVMe 命令处理器
DMA 引擎
Flash Translation Layer
DRAM/SRAM 缓存
NAND channel controller
ECC engine
多个 NAND channel
```

这些都是独立并行工作的硬件逻辑。

## 6. NVMe 完成请求

写入完成后：

```text
SSD 更新 Completion Queue
  ↓ DMA 写入系统 RAM
触发 MSI/MSI-X 中断
  ↓
CPU 收到中断
  ↓
内核中断处理函数
  ↓
驱动读取 CQ
  ↓
通知 block layer
  ↓
通知文件系统
  ↓
唤醒等待中的进程
```

完整过程：

```text
SSD
  ↓ DMA
Completion Queue
  ↓
MSI-X
  ↓
CPU 中断入口
  ↓
NVMe driver
  ↓
Block layer
  ↓
Filesystem
  ↓
Application
```

这就是一个完整的本地存储异步链路。

---

# 五、Kubernetes CSI 的作用

Kubernetes 中，应用通常不会直接创建 `/dev/nvme0n1`。它通过 PersistentVolumeClaim 请求存储：

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 100Gi
```

这里发生的是另一条控制面链路。

## 1. PVC 到 CSI

```text
用户创建 PVC
  ↓
Kubernetes API Server
  ↓
External Provisioner
  ↓
CSI Controller
  ↓
存储后端创建卷
```

CSI Controller 可能调用：

- Ceph API
- 云厂商 API
- Longhorn 管理 API
- SAN 管理接口
- NFS 管理服务

这部分通常不在业务 I/O 的热路径上。

## 2. Pod 调度和挂载

```text
Pod 调度到 Node
  ↓
kubelet 发现需要该 PVC
  ↓
Node Plugin NodeStageVolume
  ↓
NodePublishVolume
  ↓
挂载到 Pod 的 mount namespace
```

最终 Pod 看到的可能是：

```text
/var/lib/kubelet/pods/.../volumes/.../mount
```

但这个路径背后可能是：

```text
本地 ext4
云块设备
Ceph RBD
CephFS
NFS
Longhorn replica
```

CSI 主要解决的是：

```text
卷创建
卷删除
卷挂载
卷卸载
卷扩容
快照
克隆
```

它通常不直接处理每一次 `read()` 和 `write()`。

---

# 六、主流实现一：本地盘 Local PV

## 适合场景

- 高性能数据库
- Kafka
- Elasticsearch
- 本地 NVMe
- 对延迟极度敏感的缓存
- 不需要跨节点自动迁移的数据

## 数据流

```text
Pod
  ↓
文件系统
  ↓
Linux VFS
  ↓
ext4/XFS
  ↓
Linux block layer
  ↓
NVMe driver
  ↓
MMIO doorbell
  ↓
PCIe
  ↓
NVMe controller
  ↓ DMA
DRAM
  ↓
NAND Flash
```

如果 Pod 在 Node A 上，数据物理上就在 Node A 的磁盘上：

```text
Node A 崩溃
  ↓
Pod 可重新调度，但数据不会自动出现在 Node B
```

所以 Local PV 的核心特征是：

```text
低延迟、高性能
但节点绑定明显，故障迁移能力弱
```

Kubernetes 主要负责：

```text
调度 Pod 到拥有该本地盘的节点
```

它不负责跨节点复制。

---

# 七、主流实现二：Ceph RBD 块存储

Ceph 是 Kubernetes 中非常典型的分布式存储系统。

## 1. Ceph 的主要组件

```text
Ceph Client
  ↓
RBD
  ↓
OSD
  ↓
本地文件系统/裸盘
```

控制和元数据组件还包括：

```text
MON：集群状态和成员关系
MGR：管理和监控
OSD：真正存储对象和处理 I/O
CRUSH：计算对象应该放到哪些 OSD
```

RBD 把块设备切分成对象：

```text
虚拟块设备
  ↓
RBD image
  ↓
对象 object.0、object.1、object.2
  ↓
副本或纠删码
  ↓
多个 OSD
```

## 2. Pod 写 Ceph RBD 的路径

常见 Linux 内核 RBD 路径：

```text
Pod write()
  ↓
Pod mount namespace
  ↓
ext4/XFS
  ↓
Linux block layer
  ↓
krbd
  ↓
Ceph Messenger
  ↓
TCP 或 RDMA
  ↓
Node B/C/D 的 OSD
```

更完整地展开：

```text
业务 write()
  ↓
CPU 执行系统调用
  ↓
MMU 检查用户 buffer
  ↓
VFS
  ↓
文件系统
  ↓
block layer
  ↓
RBD block device
  ↓
RBD 将块地址转换为对象名
  ↓
CRUSH 计算主 OSD 和副本 OSD
  ↓
网络协议栈
  ↓
节点网卡驱动
  ↓
网卡 MMIO/DMA
  ↓
物理网络
  ↓
OSD 节点网卡
  ↓
OSD 用户态/内核处理
  ↓
OSD 本地块设备
  ↓
NVMe/SATA 驱动
  ↓
PCIe/SATA
  ↓
本地 SSD
```

## 3. Ceph 内部的数据对象映射

假设 Pod 写入：

```text
RBD image offset = 64 MiB
```

RBD 会计算：

```text
image offset
  ↓
object number
  ↓
PG（Placement Group）
  ↓
CRUSH rule
  ↓
primary OSD
  ↓
replica OSD
```

例如：

```text
object-00000123
  ↓ hash
PG 8.17
  ↓ CRUSH
OSD.4 primary
OSD.7 replica
OSD.9 replica
```

CRUSH 的特点是：

> 客户端可以根据集群拓扑和规则直接计算对象位置，不必每次查询一个中心化目录。

## 4. Ceph 写入的异步过程

```text
1. 应用把数据交给文件系统
2. 文件系统把请求交给 RBD
3. RBD 把块转换成对象操作
4. Ceph 客户端选择 primary OSD
5. 通过网络发送写请求
6. primary OSD 接收请求
7. primary OSD 转发给 replica OSD
8. 各 OSD 把数据提交到本地队列
9. 各节点本地 SSD 异步执行写入
10. replica OSD 返回完成
11. primary OSD 汇总副本确认
12. primary OSD 返回客户端
13. RBD 完成 block request
14. 文件系统结束 fsync 或写请求
```

用箭头表示：

```text
Pod
  ↓
RBD client
  ↓ network
OSD primary
  ├── local NVMe
  ├── network → OSD replica 1 → local NVMe
  └── network → OSD replica 2 → local NVMe
```

## 5. Ceph 的“完成”到底是什么意思

这取决于存储策略和操作语义。

可能的完成层次包括：

```text
客户端已把请求发送出去
primary OSD 已收到
副本 OSD 已收到
副本已写入本地日志
副本已写入本地数据区域
SSD 已确认持久化
```

`fsync()` 需要等待的通常是存储系统定义的持久性边界，而不是简单等待 CPU 写入某个寄存器。

Ceph 内部还涉及：

- journal 或 BlueStore WAL
- RocksDB 元数据
- 数据块落盘
- replica ack
- recovery
- backfill
- scrubbing
- degraded mode

## 6. Ceph 中的 CPU、寄存器和 PCIe

Ceph 的远程路径中，CPU 和 PCIe 会在每一个存储节点上重复出现。

例如 OSD 节点接收网络写请求：

```text
OSD 线程
  ↓ socket send/recv
Linux TCP/IP
  ↓
网卡驱动
  ↓ MMIO doorbell
PCIe
  ↓
网卡 DMA 写入 RAM
  ↓
CPU 收到 MSI-X
  ↓
OSD 读取内存中的网络数据
```

OSD 再把数据提交给本地 NVMe：

```text
OSD
  ↓ io_uring/libaio
Linux block layer
  ↓
NVMe driver
  ↓ MMIO Submission Queue doorbell
PCIe
  ↓
NVMe DMA
  ↓
SSD
```

因此一条 Ceph 写请求可能包含多次：

```text
网络 DMA
网络中断
CPU 协议处理
存储 DMA
存储中断
副本节点重复上述过程
```

---

# 八、主流实现三：CephFS 分布式文件系统

Ceph RBD 给 Pod 提供块设备，CephFS 直接给 Pod 提供文件系统。

## 1. RBD 和 CephFS 的区别

```text
Ceph RBD：
  Ceph 提供块设备
  客户端自己运行 ext4/XFS

CephFS：
  Ceph 直接提供分布式文件系统
  客户端访问目录、文件和权限
```

路径区别：

```text
RBD：
Pod → ext4 → block layer → krbd → OSD

CephFS：
Pod → CephFS client → MDS/OSD → 网络 → OSD
```

## 2. CephFS 读写路径

```text
Pod open("/data/a")
  ↓
CephFS client
  ↓
MDS 查询目录和 inode 元数据
  ↓
得到文件布局
  ↓
数据直接访问对应 OSD
```

数据写入：

```text
应用 write()
  ↓
CephFS client page cache
  ↓
文件布局计算
  ↓
目标对象
  ↓
对应 OSD
  ↓
副本 OSD
  ↓
本地块设备
```

MDS 主要负责：

```text
目录
文件名
inode
权限
租约
锁
元数据分布
```

OSD 主要负责：

```text
文件实际数据对象
```

因此 CephFS 中元数据和数据路径可能分离：

```text
元数据：Client ↔ MDS
数据：Client ↔ OSD
```

## 3. 适合场景

- 多个 Pod 同时挂载
- `ReadWriteMany`
- 共享配置和用户上传文件
- CI 构建目录
- 大量小文件
- 需要 POSIX 风格目录和权限

代价是：

- 元数据操作复杂
- 锁和租约复杂
- 小文件性能依赖 MDS
- 网络往返多
- 故障恢复机制复杂

---

# 九、主流实现四：云厂商远程块存储

以云环境中的 EBS、云硬盘、云磁盘为代表。

Pod 看到的通常是：

```text
/dev/xvda
/dev/nvme1n1
```

但这个设备可能并不对应节点内部真实插入的一块 SSD。

## 1. 客户端视角

```text
Pod
  ↓
ext4/XFS
  ↓
Linux block layer
  ↓
virtio-blk / virtio-scsi / 虚拟 NVMe
  ↓
虚拟机监控器
  ↓
云平台存储网络
  ↓
后端存储集群
```

在虚拟机中，可能是：

```text
Guest CPU
  ↓
Guest MMU
  ↓
虚拟 PCIe/virtio 设备
  ↓
virtqueue
  ↓
VM exit / vhost / hypervisor
  ↓
宿主机存储网络
```

## 2. virtio 路径

virtio 设备通常使用共享内存队列：

```text
Guest driver
  ↓
virtqueue descriptor
  ↓
通知寄存器或 eventfd
  ↓
vhost/hypervisor
  ↓
宿主机后端
```

这和真实 NVMe 的思想很类似：

```text
寄存器：通知队列有新请求
内存：保存请求描述符和数据
设备/后端：异步处理
```

区别是，virtio 后面的“设备”可能是软件实现的，而不是一块真实 PCIe NVMe 控制器。

## 3. 云盘后端

云平台后端可能是：

```text
前端虚拟块设备
  ↓
存储网络
  ↓
分布式块存储集群
  ↓
多个存储节点
  ↓
副本/纠删码
  ↓
本地 NVMe
```

所以应用看到的是块设备，但真正的数据仍然可能经过：

```text
虚拟 PCIe
虚拟队列
宿主机 DMA
网络交换芯片
分布式存储服务
多个 SSD
```

## 4. 适合场景

- 普通数据库
- 有状态服务
- 需要卷快照和扩容
- 节点故障后将卷挂到其他节点
- 不希望自己管理存储集群

云 CSI 插件主要负责：

```text
CreateVolume
AttachVolume
MountVolume
DetachVolume
DeleteVolume
```

真正的 I/O 通常走已挂载的虚拟块设备，不需要每次写入都调用 Kubernetes API Server。

---

# 十、主流实现五：Longhorn 一类云原生分布式块存储

Longhorn 的典型特点是：

```text
每个卷由多个 replica 组成
每个 replica 运行在某个节点
节点之间通过网络复制块写入
```

## 1. Longhorn 路径

```text
Pod
  ↓
ext4/XFS
  ↓
Linux block layer
  ↓
Longhorn block frontend
  ↓
网络协议
  ↓
Longhorn replica 进程
  ├── Node A 本地盘
  ├── Node B 本地盘
  └── Node C 本地盘
```

## 2. 写入过程

```text
1. Pod 向块设备写入
2. Longhorn frontend 接收块请求
3. Volume engine 选择 replica
4. 通过 TCP 发送到多个 replica
5. replica 进程写本地文件或块设备
6. 各 replica 返回 ack
7. engine 按副本策略决定完成
8. 返回给 Pod
```

对应底层路径：

```text
Longhorn replica
  ↓ write()
Linux VFS
  ↓
本地文件系统
  ↓
block layer
  ↓
NVMe/SATA
  ↓
PCIe/SATA
  ↓
本地盘
```

## 3. 特点

优点：

- Kubernetes 原生部署
- 节点内软件实现副本
- 可以迁移 replica
- 适合中小规模云原生集群

代价：

- 网络和 CPU 开销较高
- 每次写入可能产生多份网络和磁盘 I/O
- 延迟受副本节点和网络影响
- 存储软件本身需要占用节点资源

---

# 十一、主流实现六：NFS 和分布式文件服务

NFS 是 Kubernetes 中常见的共享文件存储。

## 1. NFS 数据路径

```text
Pod
  ↓
VFS
  ↓
NFS client
  ↓
TCP/RDMA
  ↓
网卡 DMA
  ↓
网络
  ↓
NFS server
  ↓
服务端文件系统
  ↓
服务端块设备
  ↓
SSD/HDD
```

客户端写入：

```text
应用 write()
  ↓
NFS client page cache
  ↓
RPC 请求
  ↓
TCP segmentation
  ↓
NIC TX DMA
  ↓
交换机
  ↓
服务器 NIC RX DMA
  ↓
NFS server
  ↓
服务器 VFS
  ↓
本地文件系统
  ↓
本地 NVMe
```

服务端响应反向经过：

```text
NVMe completion
  ↓
服务器 CPU
  ↓
NFS response
  ↓
服务器网卡 DMA
  ↓
客户端网卡 DMA
  ↓
客户端 CPU 中断
  ↓
NFS client
  ↓
应用
```

## 2. 适合场景

- 多 Pod 共享目录
- 用户上传文件
- 备份
- 构建产物
- 配置和脚本
- 对 POSIX 文件访问友好但不追求极低延迟的场景

NFS 的主要特点：

```text
简单
成熟
易共享
网络依赖明显
元数据和小文件操作可能成为瓶颈
```

---

# 十二、对象存储不是块存储

S3、OSS、COS、MinIO 这类对象存储的路径完全不同。

应用通常通过 HTTP API：

```text
PUT /bucket/object
GET /bucket/object
```

数据流：

```text
应用
  ↓ HTTP client
TCP
  ↓
IP
  ↓
网卡驱动
  ↓ MMIO/DMA
PCIe
  ↓
网络
  ↓
对象存储网关
  ↓
对象元数据服务
  ↓
对象数据节点
  ↓
多个磁盘/副本/纠删码
```

对象存储通常不把自己伪装成 POSIX 块设备，而是提供：

```text
对象名
对象内容
元数据
版本
生命周期
ACL
```

## 适合场景

- 图片、视频、安装包
- 日志归档
- 备份
- 数据湖
- 静态资源
- 大文件顺序读写

不适合直接替代：

```text
数据库数据目录
需要 POSIX rename 语义的程序
高频小块随机写
```

在 Kubernetes 中，对象存储通常通过：

- SDK
- HTTP API
- S3 Gateway
- 专用 Fuse 挂载

接入，而不是典型的 CSI 块卷。

---

# 十三、不同存储方案的数据流对比

| 类型 | Pod 看到的接口 | 数据是否跨节点 | 复制位置 | 典型场景 |
|---|---|---:|---|---|
| Local PV | 本地块设备/文件系统 | 否 | 无或应用自身复制 | 高性能数据库、Kafka |
| 云块存储 | 虚拟块设备 | 通常是 | 云平台后端 | 普通有状态服务 |
| Ceph RBD | 块设备 | 是 | OSD 副本/纠删码 | 通用块存储 |
| CephFS | 分布式文件系统 | 是 | OSD | RWX、共享目录 |
| Longhorn | 分布式块设备 | 是 | replica 节点 | 云原生中小规模存储 |
| NFS | 远程文件系统 | 是 | NFS 后端 | 共享文件、备份 |
| 对象存储 | HTTP 对象 API | 是 | 对象存储后端 | 图片、日志、归档 |

---

# 十四、把“异步环节”完整串起来

以 Ceph RBD 写入为例，一次 `fsync()` 可能经历下面这些并发组件：

```text
应用线程
  ↓ syscall
CPU 执行系统调用
  ↓
MMU/TLB 转换用户地址
  ↓
Linux VFS
  ↓
文件系统 page cache
  ↓
block layer 请求队列
  ↓
RBD client
  ↓
Ceph 网络线程
  ↓
CPU TCP/IP 协议处理
  ↓
网卡 TX ring
  ↓ MMIO doorbell
PCIe Root Complex
  ↓
PCIe TLP
  ↓
网卡 DMA
  ↓
物理网络
  ↓
远端网卡 DMA
  ↓
远端 CPU/OSD
  ↓
远端本地文件系统或 BlueStore
  ↓
远端 block layer
  ↓
NVMe submission queue
  ↓ MMIO doorbell
PCIe
  ↓
NVMe DMA
  ↓
SSD 控制器
  ↓
FTL/ECC
  ↓
NAND Flash
```

如果有三副本，则远端 OSD 可能形成：

```text
Primary OSD
  ├── 本地 SSD
  ├── Replica OSD 1 → 网络 → SSD
  └── Replica OSD 2 → 网络 → SSD
```

每一层都有自己的状态和队列：

```text
CPU：
  指令流水线、store buffer、缓存

MMU：
  TLB、页表遍历

Linux：
  page cache、block request queue、socket buffer

网卡：
  TX/RX descriptor ring、DMA engine

PCIe：
  TLP queue、completion tracking、credit flow control

网络：
  NIC queue、交换机 buffer、TCP send/receive queue

Ceph：
  client queue、OSD op queue、replication queue

NVMe：
  submission queue、completion queue

SSD：
  FTL queue、NAND channel queue、GC queue
```

这就是为什么一个简单的：

```text
write(fd, buffer, 4096)
```

背后可能有几十个异步状态机同时推进。

---

# 十五、数据“完成”的多个层次

存储系统中，“写完成”不是单一事件。

可能依次经过：

```text
1. 应用把数据交给内核
2. 数据进入 page cache
3. 文件系统生成块请求
4. 块请求进入驱动队列
5. 数据进入网卡发送队列
6. 远端 OSD 收到请求
7. 主 OSD 写入日志
8. 副本 OSD 收到请求
9. 本地 NVMe 接收命令
10. SSD 控制器完成 NAND 编程
11. 副本达到 quorum
12. 存储系统返回 ack
13. 文件系统完成 fsync
14. 应用得到返回值
```

不同系统对“成功”的定义可能不同：

```text
write 返回：
  内核接受了数据

fsync 返回：
  存储系统承诺达到某种持久化边界

副本 ack：
  多个节点确认收到或持久化

对象 PUT 返回：
  对象服务完成其一致性承诺
```

因此业务选择存储系统时，必须问清楚：

- `write()` 返回代表什么？
- `fsync()` 是否真正等待设备持久化？
- 副本数量是多少？
- 是否等待 quorum？
- 节点断电后数据是否保留？
- 网络分区时是否允许继续写入？
- 是否可能丢失最近几秒数据？
- 是否支持快照和一致性恢复？

---

# 十六、Kubernetes 的控制面不在每次 I/O 路径上

一个非常重要的边界是：

```text
Kubernetes API Server
```

通常不在每次业务读写的热路径上。

创建卷时：

```text
PVC
  ↓
API Server
  ↓
CSI Controller
  ↓
后端存储 API
```

挂载完成后，Pod 的实际读写通常是：

```text
Pod
  ↓
Linux VFS
  ↓
本地块设备、网络文件系统或分布式存储客户端
```

不会每次写 4 KiB 都经过：

```text
Pod → kube-apiserver → CSI Controller
```

否则性能和可靠性都会不可接受。

因此：

```text
Kubernetes：
  管理资源生命周期和期望状态

CSI：
  负责卷创建、挂载、卸载等控制操作

Linux 内核和存储客户端：
  负责热路径 I/O

设备驱动：
  负责本地硬件访问

硬件：
  负责 DMA、队列和介质操作
```

---

# 十七、从这个工程继续学习存储，建议增加哪些抽象

当前工程已经有：

```rust
trait FileSystem
trait Inode
```

可以继续向下设计：

```rust
trait BlockDevice {
    fn block_size(&self) -> usize;
    fn read_block(&self, index: u64, buf: &mut [u8]) -> Result<()>;
    fn write_block(&mut self, index: u64, buf: &[u8]) -> Result<()>;
}
```

然后实现：

```text
MemoryBlockDevice
  ↓
RamDisk
  ↓
简单文件系统
```

再逐步增加：

```text
BlockDevice trait
  ↓
virtio-blk
  ↓
NVMe
  ↓
异步请求队列
  ↓
DMA buffer
  ↓
中断完成
  ↓
文件系统日志
  ↓
mount
```

推荐的教学演进路线：

```text
1. 内存块设备
2. 读写固定大小扇区
3. 简单 inode 文件系统
4. block request queue
5. virtio-blk
6. DMA 描述符
7. 中断完成队列
8. ext2 或简化日志文件系统
9. 网络块设备
10. 多副本写入协议
```

其中最能连接到 Kubernetes 的是：

```text
BlockDevice
  ↓
异步 I/O
  ↓
网络复制
  ↓
quorum
  ↓
故障恢复
```

---

# 总结

可以用下面这张总图理解 Kubernetes 分布式存储：

```text
Pod
  ↓
系统调用
  ↓
CPU 寄存器 / MMU / 页表
  ↓
VFS / 文件系统
  ↓
块层或网络文件系统
  ↓
本地存储客户端：RBD、Longhorn、NFS、virtio
  ↓
网络协议栈或块设备驱动
  ↓
网卡/NVMe 寄存器
  ↓
MMIO / PCIe TLP
  ↓
设备 DMA 读写 RAM
  ↓
网卡网络传输或 NVMe 控制器
  ↓
远程 OSD / NFS Server / 云存储后端
  ↓
副本、日志、纠删码、故障恢复
  ↓
多个物理磁盘
```

最核心的对应关系是：

```text
SimpleLinux 的 Inode：
  把文件操作抽象成 read/write

Linux VFS：
  把不同文件系统统一起来

Block layer：
  把文件系统请求统一成块 I/O

设备驱动：
  把块 I/O 翻译成寄存器、队列和 DMA

PCIe：
  把 CPU/设备请求在硬件之间传递

分布式存储客户端：
  把本地块 I/O 翻译成网络请求和副本操作

Kubernetes CSI：
  管理卷的创建、挂载和生命周期

Kubernetes：
  管理 Pod 使用哪个存储资源
```

所以，Ceph、Longhorn、云块存储和 NFS 的核心差别，不在于它们是否使用 CPU、寄存器、DMA 和 PCIe，而在于：

```text
它们把“本地块设备之后的持久化和复制逻辑”
放在了不同位置：
本地内核、远程 OSD、云平台后端或文件服务器。
```