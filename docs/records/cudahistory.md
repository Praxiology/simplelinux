不是从一开始就完整设计好的。CUDA 是一个持续演化的系统，基本路径是：

```text
GPGPU 实验
  → CUDA 编程模型
  → GPU 硬件计算单元
  → 深度学习需求
  → 多 GPU/多节点通信
  → 大模型推理与训练
  → 图执行、融合、量化、专用互连
```

这些概念来自不同层次：

```text
CUDA 语言/编程模型：
  Thread、Block、Grid、Warp、Kernel、Stream、Event

GPU 硬件：
  SM、CUDA Core、Tensor Core、Shared Memory、L1/L2、显存

互连和设备能力：
  P2P、NVLink、NVSwitch、GPUDirect RDMA

通信库：
  NCCL、All-Reduce、All-Gather、Reduce-Scatter

模型并行策略：
  Tensor Parallel、Pipeline Parallel、Expert Parallel

执行优化：
  CUDA Graph、Kernel Fusion、异步拷贝、量化
```

它们不是在同一时间、同一层次设计出来的。

---

# 一、CUDA 出现之前：GPU 原本是图形设备

## 1. 固定功能 GPU 时代

早期 GPU 主要负责：

```text
顶点变换
光栅化
纹理采样
像素着色
深度测试
```

程序员不能自由编写任意通用计算程序，只能通过图形 API：

```text
OpenGL
Direct3D
```

把通用问题伪装成图形问题。

例如，矩阵计算可能被编码成：

```text
输入数据 → 纹理
计算逻辑 → Fragment Shader
输出数据 → Render Target
```

这就是早期 GPGPU 的基本思想：

```text
把 GPU 的图形流水线借来执行非图形计算
```

## 2. 为什么 GPU 适合通用计算？

图形渲染天然具有高度数据并行特征：

```text
每个像素都可以独立计算
每个顶点都可以独立变换
每个纹理采样都可以并行执行
```

因此 GPU 从硬件上就逐渐形成了：

```text
大量算术单元
高显存带宽
相对简单的控制流
批量执行模型
```

这为后来的 CUDA 打下基础。

---

# 二、2000 年前后：可编程 Shader 和早期 GPGPU

随着可编程 Vertex Shader、Pixel Shader 出现，开发者开始利用 Shader 进行：

```text
线性代数
图像处理
物理模拟
金融计算
科学计算
```

但这种方式很不自然，开发者需要处理：

- 纹理格式；
- 渲染目标；
- Shader 输入输出；
- 图形 API 的限制；
- 缺少通用指针；
- 缺少通用内存模型；
- 不方便调试。

当时的核心问题不是 GPU 算力不足，而是：

```text
GPU 有并行计算能力，
但没有适合通用程序员的编程抽象。
```

---

# 三、2004 年左右：Brook 和通用流处理思想

Stanford 的 Brook/BrookGPU 等项目尝试把 GPU 描述成流处理器。

程序员可以表达：

```text
输入流
  ↓
Kernel
  ↓
输出流
```

例如：

```text
数组 A、数组 B
  ↓
逐元素加法 Kernel
  ↓
数组 C
```

这已经接近 CUDA 的思想：

```text
数据并行
Kernel
流
```

但 Brook 仍然受到图形硬件限制：

```text
内存访问受限
指针能力弱
同步能力有限
硬件结构不够统一
调试体验差
```

它证明了一个重要方向：

```text
GPU 应该暴露为通用并行处理器，
而不是只能通过图形 API 使用。
```

---

# 四、2006/2007 年：CUDA 正式出现

## 1. CUDA 的核心目标

CUDA 的设计目标不是一开始就支持大模型，而是降低 GPU 通用计算门槛。

NVIDIA 在 2006 年发布 CUDA，第一代 CUDA-capable Tesla 产品和 CUDA 1.0 大约在 2007 年进入开发者使用阶段。

CUDA 引入了几个关键抽象：

```text
Host：CPU
Device：GPU
Kernel：运行在 GPU 上的函数
Thread：并行执行实例
Block：可协作线程组
Grid：一次 Kernel 启动产生的线程集合
```

最典型的 CUDA 程序：

```cpp
__global__ void vector_add(float* a, float* b, float* c) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    c[i] = a[i] + b[i];
}
```

CPU 端启动：

```cpp
vector_add<<<grid, block>>>(a, b, c);
```

## 2. 为什么设计成 Grid、Block、Thread？

因为 GPU 的硬件调度需要可扩展：

```text
Thread：
  最小计算实例

Block：
  可以放到一个 SM 上执行，并使用 Shared Memory 和 Barrier

Grid：
  可以扩展到多个 SM
```

这种层次解决了一个硬件问题：

```text
程序不需要知道 GPU 具体有多少个 SM，
只需要指定足够多的 Block。
```

GPU 硬件可以根据资源情况调度：

```text
GPU 有 10 个 SM → 同时运行部分 Block
GPU 有 100 个 SM → 同时运行更多 Block
```

因此 Grid/Block 不是后来为了大模型才发明的，而是 CUDA 最初就存在的核心执行模型。

---

# 五、SM、Warp 和 CUDA Core：早期就存在的硬件执行基础

## 1. SM

SM，即 Streaming Multiprocessor，是 GPU 的主要执行簇。

一个 SM 通常包含：

```text
CUDA Core
Warp Scheduler
Register File
Shared Memory
L1 Cache
Load/Store Unit
特殊函数单元
```

Kernel 的 Block 会被分配到 SM。

```text
Grid
  ├── Block 0 → SM 0
  ├── Block 1 → SM 1
  ├── Block 2 → SM 0
  └── ...
```

## 2. Warp

NVIDIA GPU 通常以 32 个线程为一个 Warp 执行。

```text
Warp = 32 个逻辑线程
```

早期 GPU 采用类似 SIMD 的执行方式，但 CUDA 对外提供的是 SIMT：

```text
Single Instruction, Multiple Threads
```

程序员写的是：

```cpp
int i = threadIdx.x;
```

硬件则将这些线程组织成 Warp 执行。

Warp 并不是为了深度学习设计的，而是 GPU 图形和通用数据并行的基础。

## 3. CUDA Core

CUDA Core 是执行标量浮点、整数等基本运算的执行单元。

它不是 CPU Core 的等价物：

```text
CPU Core：
  复杂控制流、乱序执行、分支预测、较强单线程能力

CUDA Core：
  在 SM 和 Warp 调度器组织下，执行大量相似线程的算术操作
```

---

# 六、Constant、Shared、Global Memory：第一代 CUDA 就有的内存层次

GPU 的多级内存并不是大模型时代才出现的。

早期 CUDA 就有：

```text
Register
Local Memory
Shared Memory
Global Memory
Constant Memory
Texture Memory
```

## 1. Global Memory

Global Memory 通常对应 GPU 显存：

```text
容量大
所有线程可访问
延迟较高
带宽高
需要合并访问
```

## 2. Shared Memory

Shared Memory 位于 SM 附近：

```text
Block 内线程共享
容量较小
速度较快
需要显式管理
```

它的早期主要用途包括：

```text
矩阵分块
数据重用
线程协作
局部归约
```

## 3. Constant Memory

Constant Memory 适合：

```text
小量只读数据
多个线程读取相同地址
配置参数
查找表
```

如果 Warp 中的线程读取同一个 Constant Memory 地址，硬件可以高效广播。

## 4. 这些设计的根本原因

GPU 不是简单追求计算单元数量，而是要解决：

```text
算术单元很多，
但如果每次计算都等待显存，
性能仍然很低。
```

因此 CUDA 从一开始就要求程序员关注：

```text
数据放在哪一层
数据是否可以复用
线程访问是否连续
是否需要使用 Shared Memory
```

---

# 七、Stream 和 Event：异步 GPU 编程的早期基础

GPU 计算通常不是 CPU 调用函数后立即完成，而是：

```text
CPU 提交工作
  ↓
GPU 异步执行
```

因此 CUDA 很早就提供了：

```text
Stream
Event
Synchronize
```

## 1. Stream

Stream 是有序的 GPU 操作队列：

```text
Stream 0:
  H2D Copy
  Kernel A
  Kernel B
  D2H Copy
```

同一个 Stream 中通常保持顺序。

## 2. Event

Event 用于：

```text
记录 GPU 操作完成位置
跨 Stream 建立依赖
测量 GPU 时间
```

典型依赖：

```text
Stream 0：
  Kernel A
  Record Event E

Stream 1：
  Wait Event E
  Kernel B
```

这些机制源于最初的 CPU-GPU 异步执行模型，不是后来专为深度学习增加的。

---

# 八、从 CUDA 1.0 到 CUDA 5：设备能力逐渐增强

## 1. CUDA 2.x：更强的设备内存和多线程能力

CUDA 早期主要解决：

```text
让程序能够在 GPU 上执行
```

随着版本发展，逐渐增强：

- 更好的双精度支持；
- 更大的显存；
- 更强的原子操作；
- 更灵活的线程同步；
- 更成熟的工具链；
- 更好的纹理和只读缓存。

## 2. Fermi：更完整的通用计算 GPU

Fermi 架构大约在 2010 年出现，代表 GPU 通用计算能力进一步成熟。

重要方向包括：

```text
统一地址空间趋势
更强的缓存体系
更好的双精度
ECC
更强的原子操作
```

这使 GPU 更像一个真正的通用并行计算设备。

## 3. CUDA 4.0：UVA 和 P2P 逐渐成熟

CUDA 4.0 大约在 2011 年发布，重要能力包括：

```text
Unified Virtual Addressing
Peer-to-Peer Memory Access
Unified Memory 的早期方向
```

### P2P

多个 GPU 可以直接访问彼此的显存：

```text
GPU 0 VRAM
  ↔ GPU 1 VRAM
```

不再强制经过：

```text
GPU 0 → CPU RAM → GPU 1
```

这为多 GPU 计算奠定基础。

### UVA

UVA 让 CPU 和多个 GPU 的地址空间具有统一的地址视图，简化了指针管理。

但要注意：

```text
统一地址空间 ≠ 统一速度
统一地址空间 ≠ 所有内存完全一致
```

它主要是编程和地址管理抽象。

---

# 九、Kepler 时代：GPU 开始具备更强的设备端控制

Kepler 大约在 2012 年出现，CUDA 5 时代引入或强化了：

```text
Dynamic Parallelism
Shuffle
更强的 Hyper-Q
更好的并发 Kernel 执行
```

## 1. Dynamic Parallelism

GPU Kernel 可以启动另一个 GPU Kernel：

```text
Kernel A
  ↓
GPU 内部启动 Kernel B
```

这减少了部分 CPU 参与。

但 Dynamic Parallelism 并没有让 GPU 取代 CPU，因为：

- 启动成本仍然存在；
- 适用场景有限；
- 复杂动态调度仍不如 CPU 灵活；
- GPU 资源管理仍由驱动和运行时主导。

## 2. Warp Shuffle

Warp 内线程可以直接交换寄存器数据：

```text
Thread 0 的寄存器值
  ↓ shuffle
Thread 1 读取
```

这减少了：

```text
Shared Memory 写入
Shared Memory 读取
Block 级同步
```

对归约、Softmax、矩阵计算非常重要。

---

# 十、深度学习出现：CUDA 的需求发生改变

CUDA 最初主要服务：

```text
科学计算
线性代数
物理仿真
图像处理
金融建模
```

2012 年 AlexNet 使用 GPU 训练后，深度学习成为 CUDA 发展的巨大推动力。

GPU 需要面对新的工作负载：

```text
大规模 GEMM
卷积
张量归约
Softmax
Batch Normalization
反向传播
多 GPU 训练
```

## 1. cuBLAS 和 cuDNN

NVIDIA 逐步建立库层：

```text
cuBLAS：
  矩阵乘法和线性代数

cuFFT：
  快速傅里叶变换

cuSPARSE：
  稀疏矩阵

cuDNN：
  深度学习基础算子

NCCL：
  多 GPU 集体通信
```

这说明 CUDA 生态开始从：

```text
“让程序员自己写 Kernel”
```

转向：

```text
“提供高度优化的领域基础库”
```

## 2. 为什么需要库？

一个高性能 GEMM Kernel 需要考虑：

```text
矩阵分块
Shared Memory
寄存器占用
Warp 调度
线程块尺寸
显存合并访问
不同矩阵形状
不同数据类型
不同 GPU 架构
```

绝大多数应用开发者不应该重复实现这些细节。

---

# 十一、Maxwell、Pascal：GPU 逐渐进入大规模多卡时代

随着深度学习规模增大，单卡不够，需要：

```text
多 GPU
多节点
更快互连
更高显存容量
更高通信带宽
```

## 1. 多 GPU 的新问题

假设一个模型被切成四份：

```text
GPU 0：权重分片 0
GPU 1：权重分片 1
GPU 2：权重分片 2
GPU 3：权重分片 3
```

计算过程中需要：

```text
All-Reduce
All-Gather
Reduce-Scatter
Broadcast
```

这时候瓶颈可能不再是单 GPU 计算，而是：

```text
GPU-GPU 通信
CPU 内存中转
PCIe 带宽
同步等待
```

## 2. NVLink

NVLink 在 Maxwell/Pascal 时期逐步成为产品能力。

它解决的是：

```text
GPU 与 GPU 之间的高速互连
```

典型链路：

```text
GPU 0
  ↔ NVLink
GPU 1
```

相比 PCIe，它提供：

```text
更高带宽
更低延迟
更适合 GPU P2P
```

NVLink 的出现是由多 GPU 计算需求推动的，不是 CUDA 最初单卡模型中的必要组成部分。

---

# 十二、NCCL 的出现：把 GPU 通信从手工 P2P 提升为集体通信

NCCL 大约在 2015 年前后成为 NVIDIA GPU 集体通信的重要组件。

它解决的问题是：

```text
开发者不想手工编写复杂的多 GPU Ring、Tree 和拓扑通信代码。
```

## 1. NCCL 提供什么？

```text
ncclAllReduce
ncclAllGather
ncclReduceScatter
ncclBroadcast
ncclSend / ncclRecv
```

## 2. All-Reduce 从哪里来？

All-Reduce 并不是 CUDA 发明的，它早已存在于：

```text
MPI
分布式科学计算
超级计算机
```

CUDA/NCCL 做的是：

```text
把集体通信高效映射到 GPU、NVLink、PCIe、RDMA。
```

## 3. Ring All-Reduce

假设四张 GPU：

```text
GPU 0 → GPU 1 → GPU 2 → GPU 3 → GPU 0
```

分阶段完成：

```text
Reduce-Scatter
  ↓
每张卡得到聚合结果的一部分

All-Gather
  ↓
每张卡得到完整结果
```

这种方法能利用链路带宽，避免一个中心节点成为瓶颈。

---

# 十三、Volta：Tensor Core 和深度学习专用硬件出现

Volta 大约在 2017 年出现，标志着 NVIDIA 开始为深度学习增加专用矩阵计算单元。

## 1. Tensor Core

Tensor Core 用于执行小块矩阵乘加：

$$
D = A \times B + C
$$

它不等于普通 CUDA Core，而是专门优化：

```text
FP16
FP32 累加
后来的 BF16、TF32、FP8 等
```

## 2. 为什么出现 Tensor Core？

深度学习中大量计算是：

```text
GEMM
Convolution
Attention
```

而这些计算的共同特点是：

```text
矩阵密集
数据类型相对低精度
允许一定数值误差
需要极高吞吐
```

于是 GPU 硬件从通用算术单元进一步演化为：

```text
通用 CUDA Core
+
矩阵专用 Tensor Core
```

## 3. Cooperative Groups

CUDA 9 时代引入 Cooperative Groups 等更灵活的协作抽象，使线程协作不再只依赖传统 Block 级接口。

这服务于：

```text
Warp 协作
Block 协作
Grid 级协作
更复杂的并行算法
```

---

# 十四、CUDA Graph：从“逐 Kernel 提交”走向“提交执行图”

CUDA 早期执行模型通常是：

```text
CPU launch Kernel A
CPU launch Kernel B
CPU launch Kernel C
```

深度学习模型发展后，一个推理或训练步骤可能包含数百甚至数千个 Kernel。

CPU 端开销变得明显：

```text
Python
  → PyTorch
  → CUDA Runtime
  → Driver
  → GPU Command Queue
```

于是需要把一组稳定操作提前捕获。

CUDA Graph 在 CUDA 10 左右逐步成为正式能力，核心思想是：

```text
捕获：
  Kernel A → Kernel B → Memcpy → Kernel C

之后：
  一次 Graph Launch
```

它不是 CUDA 最初的基本概念，而是对早期 Stream/Kernel 提交模型的优化。

## 1. 为什么深度学习特别需要 CUDA Graph？

因为模型推理有大量重复结构：

```text
同样的 Transformer Layer
同样的 Kernel 顺序
同样的 Buffer 地址
只是输入数据不同
```

CUDA Graph 可以把：

```text
CPU 高频提交很多小操作
```

变成：

```text
CPU 更新少量数据
一次提交整张执行图
```

## 2. CUDA Graph 的限制

它不适合完全动态的场景：

```text
Kernel 数量频繁变化
Buffer 地址频繁变化
Shape 完全不固定
控制流结构高度变化
```

所以框架往往使用：

```text
多个 Graph 模板
静态 Shape Bucket
预分配 Buffer
padding
动态 Metadata
```

---

# 十五、Ampere、Hopper：异步数据搬运和更强矩阵计算

后续 GPU 架构继续围绕两个方向演化：

```text
提高矩阵计算吞吐
降低数据搬运和同步成本
```

重要能力包括：

```text
更强 Tensor Core
TF32
BF16
FP8
异步 Copy
Tensor Memory Accelerator
更强的 Shared Memory
更高效的 Barrier
更强的 NVLink
```

## 1. 异步拷贝

传统逻辑：

```text
线程从 Global Memory 读取
  ↓
写入 Shared Memory
  ↓
同步
  ↓
计算
```

更先进的方式是：

```text
Copy Engine/异步内存路径预取下一块数据
  ↓
当前线程块同时计算上一块数据
```

形成流水线：

```text
搬运 Tile 0 → 计算 Tile 0
搬运 Tile 1 → 计算 Tile 1
搬运 Tile 2 → 计算 Tile 2
```

这直接服务于：

```text
FlashAttention
GEMM
Transformer
卷积
```

---

# 十六、NVSwitch：从点到点互连扩展到 GPU 交换网络

单纯 NVLink 点到点连接在 GPU 数量增加时会遇到拓扑问题：

```text
GPU 0 ↔ GPU 1
GPU 0 ↔ GPU 2
GPU 1 ↔ GPU 3
...
```

NVSwitch 的思路是：

```text
GPU 不再只依赖局部点到点连接，
而是通过交换芯片构成 GPU Fabric。
```

典型形式：

```text
GPU 0 ─┐
GPU 1 ─┤
GPU 2 ─┼── NVSwitch Fabric
GPU 3 ─┤
GPU 4 ─┘
```

NVSwitch 主要解决：

```text
多 GPU 之间的带宽均衡
拓扑复杂度
All-Reduce 扩展性
大规模训练通信
```

它仍然不负责理解模型逻辑：

```text
NVSwitch 不知道什么是 Transformer
不负责调度 Layer
不负责管理 KV Cache
不负责实现 Tensor Parallel
```

它提供的是硬件通信基础设施。

---

# 十七、GPUDirect：减少 CPU 内存中转

GPU 集群和高性能存储发展后，出现了大量数据路径问题：

```text
GPU
  → CPU 内存
  → 网卡
  → 网络
  → CPU 内存
  → GPU
```

这条路径存在：

```text
额外复制
CPU 内存带宽消耗
CPU 参与
更高延迟
```

于是 GPUDirect 家族逐渐发展起来。

## 1. GPUDirect P2P

GPU 之间直接访问：

```text
GPU 0 VRAM ↔ GPU 1 VRAM
```

## 2. GPUDirect RDMA

网卡直接访问 GPU 显存：

```text
GPU 显存
  ↔ NIC DMA
  ↔ InfiniBand / RoCE
  ↔ NIC DMA
  ↔ 远端 GPU 显存
```

## 3. GPUDirect Storage

存储设备直接把数据送入 GPU 显存：

```text
NVMe
  → PCIe DMA
  → GPU VRAM
```

而不是：

```text
NVMe
  → CPU 内存
  → CPU memcpy
  → GPU VRAM
```

这些能力主要来自：

```text
DMA
IOMMU
PCIe P2P
GPU 地址管理
RDMA
驱动协作
```

---

# 十八、Tensor Parallel、Pipeline Parallel、Expert Parallel 的来源

这些不是 CUDA 最初设计的语言概念，而是模型和分布式系统发展后形成的软件并行策略。

## 1. Tensor Parallel

一个矩阵被切到多个 GPU：

```text
W = [W0, W1, W2, W3]
```

每张 GPU 计算一部分：

```text
GPU 0：X × W0
GPU 1：X × W1
GPU 2：X × W2
GPU 3：X × W3
```

之后使用：

```text
All-Reduce
All-Gather
Reduce-Scatter
```

它依赖 CUDA/NCCL，但不是 CUDA 语言原语。

## 2. Pipeline Parallel

模型层被切分：

```text
GPU 0：Layer 0-15
GPU 1：Layer 16-31
GPU 2：Layer 32-47
```

激活值在 GPU 间传输：

```text
GPU 0 → GPU 1 → GPU 2
```

它来自分布式深度学习的模型切分需求。

## 3. Expert Parallel

MoE 模型中，Token 被路由到不同 Expert：

```text
Router
  ├── Expert 0
  ├── Expert 1
  ├── Expert 2
  └── Expert 3
```

跨 GPU 时需要：

```text
All-to-All
Token Dispatch
Token Permutation
Token Combine
```

它是模型结构和 GPU 集群规模共同推动产生的。

---

# 十九、统一内存和页面迁移

随着 GPU 程序变复杂，开发者不希望手动管理所有 Host/Device 指针。

于是 CUDA 逐渐增强：

```text
Unified Virtual Addressing
Unified Memory
Page Fault
Page Migration
Memory Prefetch
```

典型逻辑：

```text
CPU 访问页面
  ↓
页面位于 CPU 内存

GPU 访问同一页面
  ↓
GPU Page Fault
  ↓
驱动迁移页面到 GPU 显存
  ↓
更新 GPU 页表
  ↓
GPU 继续执行
```

这提高了易用性，但可能带来性能问题：

```text
隐式页面迁移
PCIe 往返
页错误
同步停顿
```

因此高性能推理框架通常仍偏好：

```text
显式显存分配
Pinned Memory
异步 memcpy
固定 Buffer
显式 KV Cache 管理
```

---

# 二十、从 GPU 编程到大模型推理的演化链

可以总结成下面的演化过程：

```text
阶段 1：图形流水线
  目标：渲染像素和顶点

阶段 2：GPGPU
  目标：借用图形硬件执行通用计算

阶段 3：CUDA
  目标：用 C/C++ 直接编程 GPU

阶段 4：通用 GPU 计算
  目标：科学计算、线性代数、仿真

阶段 5：深度学习
  目标：大规模 GEMM、卷积、归约

阶段 6：多 GPU
  目标：P2P、NVLink、NCCL、集体通信

阶段 7：深度学习专用硬件
  目标：Tensor Core、FP16、BF16、FP8、FP4

阶段 8：低开销执行
  目标：CUDA Graph、Kernel Fusion、异步拷贝

阶段 9：大模型
  目标：KV Cache、Paged Attention、量化、MoE、MLA

阶段 10：超大规模集群
  目标：NVSwitch、RDMA、GPUDirect、跨节点并行
```

---

# 二十一、哪些概念是“硬件概念”，哪些是“软件概念”？

## 早期 CUDA 核心编程模型

```text
Kernel
Thread
Block
Grid
Warp
Stream
Event
```

这些大部分是 CUDA 对 GPU 硬件执行模型的抽象。

## GPU 硬件资源

```text
SM
CUDA Core
Tensor Core
Register
Shared Memory
L1/L2 Cache
Global Memory
NVLink
NVSwitch
DMA Engine
```

这些是硬件或硬件相关能力。

## 运行时和通信库

```text
CUDA Runtime
CUDA Driver
NCCL
cuBLAS
cuDNN
TensorRT
```

这些是软件基础设施。

## 分布式模型策略

```text
Tensor Parallel
Pipeline Parallel
Expert Parallel
Data Parallel
Sequence Parallel
```

这些不是 CUDA 的基础语法，而是建立在 CUDA、NCCL 和 GPU 互连之上的分布式算法。

## 算法操作

```text
All-Reduce
All-Gather
Reduce-Scatter
All-to-All
```

这些本来就是并行计算和 MPI 领域的概念，NCCL 把它们高效映射到 GPU 集群。

---

# 二十二、为什么这些能力没有一次性设计完？

因为最初无法准确预测未来工作负载。

2007 年 CUDA 设计时，主要问题是：

```text
如何让程序员更容易使用 GPU 计算能力？
```

那时还没有今天的：

```text
大语言模型
百亿参数模型
KV Cache
MoE
Prefill/Decode 分离
多节点推理
GPU Direct Storage
```

后来每一阶段暴露新的瓶颈：

```text
GPU 算力不够
  → 增加更多 CUDA Core

矩阵计算不够快
  → Tensor Core

显存访问成为瓶颈
  → Cache、Shared Memory、异步拷贝、融合 Kernel

单卡不够
  → P2P、NVLink

多卡通信成为瓶颈
  → NCCL、NVSwitch

CPU 提交成为瓶颈
  → CUDA Graph

CPU 内存中转成为瓶颈
  → GPUDirect RDMA、GPUDirect Storage

模型显存不够
  → 量化、分页 KV Cache、压缩表示

MoE 通信成为瓶颈
  → All-to-All、Expert Parallel 专项优化
```

这是一种典型的软硬件共同演化：

```text
市场工作负载
  ↓
暴露性能瓶颈
  ↓
框架提出优化
  ↓
库和编译器吸收
  ↓
硬件增加专用能力
  ↓
新的工作负载再次出现
```

---

# 二十三、CUDA 发展中一个重要的方向变化

CUDA 的角色发生了三次变化。

## 第一阶段：GPU 编程语言

```text
CUDA = 让 C/C++ 程序员写 GPU Kernel
```

## 第二阶段：GPU 运行时平台

```text
CUDA = Kernel + 内存 + Stream + Event + Driver + 工具链
```

## 第三阶段：GPU 计算生态

```text
CUDA = 硬件架构
      + 编译器
      + 基础库
      + 通信库
      + 图执行
      + 多 GPU
      + 分布式系统
```

现在很多开发者并不直接手写 CUDA，而是通过：

```text
PyTorch
Triton
TensorRT-LLM
vLLM
SGLang
CuPy
JAX
```

间接使用 CUDA 生态。

---

# 二十四、最终结论

CUDA 不是一开始就设计好了今天所有概念。

更准确的历史关系是：

```text
Thread / Block / Grid / Warp / Kernel：
  CUDA 初期核心编程模型

Shared / Global / Constant Memory：
  CUDA 初期就存在的 GPU 内存层次

Stream / Event：
  CUDA 初期就存在的异步执行机制

P2P / UVA：
  多 GPU 和统一地址管理需求推动发展

NVLink：
  多 GPU 带宽需求推动硬件演化

NCCL：
  多 GPU 集体通信需求推动的软件库

Tensor Core：
  深度学习矩阵计算需求推动的硬件单元

CUDA Graph：
  大量 Kernel 提交开销推动的执行图机制

NVSwitch：
  多 GPU 集群扩展需求推动的交换互连

GPUDirect RDMA：
  多机 GPU 通信和减少 CPU 中转需求推动

Tensor/Pipeline/Expert Parallel：
  大模型规模和分布式训练需求推动的软件策略
```

可以把 CUDA 的发展概括成：

```text
最初：
  “如何让 CPU 程序员使用 GPU？”

后来：
  “如何让一块 GPU 高效执行大规模并行计算？”

再后来：
  “如何让多块 GPU 共同执行一个模型？”

现在：
  “如何让数千块 GPU、显存、网络、存储和调度器组成一个统一计算系统？”
```

所以，CUDA 的历史本质上不是一次性设计完毕的产品规划，而是：

```text
编程模型、GPU 硬件、深度学习框架、通信库和市场工作负载
相互推动、逐层吸收、持续演化的结果。
```