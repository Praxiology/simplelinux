多卡之间的 Buffer 依赖，通常需要同时解决两个问题：

```text
1. 数据如何从 GPU A 到 GPU B
2. GPU B 如何知道 GPU A 已经写完
```

NVLink、NVSwitch 主要解决第一个问题；CUDA Event、Stream、NCCL 通信协议等解决第二个问题。

---

## 1. 最基本的数据流

假设：

```text
GPU 0 上的 Buffer A
  ↓ 计算
GPU 0 上的 Buffer X
  ↓
GPU 1 需要读取 X
```

理想路径是：

```text
GPU 0 显存
  ↓ P2P DMA
NVLink / PCIe / NVSwitch
  ↓
GPU 1 显存
  ↓
GPU 1 Kernel 读取
```

而不是：

```text
GPU 0 显存
  ↓
CPU 内存
  ↓
GPU 1 显存
```

后者会增加 CPU 内存中转和 PCIe 往返。

---

# 二、主流数据传输方式

## 1. PCIe P2P

没有 NVLink 时，多卡通常通过 PCIe 互相访问：

```text
GPU 0
  ↓ PCIe P2P
PCIe Root Complex / Switch
  ↓
GPU 1
```

GPU 0 可以直接对 GPU 1 的显存发起 DMA：

```text
GPU 0 Copy Engine
  → PCIe Memory Write
  → GPU 1 VRAM
```

CPU 不需要读取数据。

但 PCIe P2P 有几个限制：

- 带宽低于 NVLink；
- 延迟通常更高；
- 拓扑依赖很强；
- 某些主板或虚拟化环境不支持完整 P2P；
- 两张卡可能经过不同 NUMA 节点和 CPU Root Complex。

检查多卡系统时，不能只看 GPU 数量，还要看：

```text
GPU 0 ↔ GPU 1 是否支持 P2P
是否共享 PCIe Switch
是否跨 NUMA
是否经过 CPU Root Complex
```

---

## 2. NVLink

NVLink 是 GPU 之间的高速点到点互连：

```text
GPU 0 ⇄ NVLink ⇄ GPU 1
```

它通常提供：

- 高于 PCIe 的带宽；
- 更低的通信延迟；
- GPU 显存之间的直接访问；
- 更适合 All-Reduce、All-Gather、Pipeline 等通信。

数据路径：

```text
GPU 0 显存
  ↓
GPU 0 NVLink Controller
  ↓
NVLink SerDes
  ↓
GPU 1 NVLink Controller
  ↓
GPU 1 显存
```

对于 GPU Kernel 来说，远端显存通常可以通过统一虚拟地址访问：

```text
GPU 1 Kernel
  → 访问 GPU 0 的远端地址
  → NVLink
  → GPU 0 显存
```

但是要注意：

```text
远端显存可访问 ≠ 远端显存访问速度等于本地显存
```

远端访问仍然会有：

- 链路延迟；
- 链路带宽限制；
- 远端缓存/一致性约束；
- 访问模式限制。

因此高性能框架通常会尽量把数据放在计算所在 GPU 本地，只有必要时才访问远端显存。

---

## 3. NVSwitch

当 GPU 数量增加时，仅让 GPU 两两直连会导致拓扑复杂：

```text
GPU 0 ↔ GPU 1
GPU 0 ↔ GPU 2
GPU 1 ↔ GPU 3
...
```

NVSwitch 提供 GPU 之间的交换网络：

```text
GPU 0 ─┐
GPU 1 ─┤
GPU 2 ─┼── NVSwitch
GPU 3 ─┤
GPU 4 ─┘
```

典型数据路径：

```text
GPU 0
  ↓ NVLink
NVSwitch
  ↓ NVLink
GPU 5
```

NVSwitch 的价值是：

```text
让多 GPU 之间获得更规则的带宽和可预测的拓扑
```

它特别适合：

- Tensor Parallel；
- 大规模 All-Reduce；
- MoE Expert Parallel；
- 多 GPU 模型训练；
- 大模型推理中的 KV 或激活交换。

需要区分：

```text
NVLink：链路
NVSwitch：交换设备
```

NVSwitch 并不是同步器，也不会自动理解“某个 Buffer 已经写完”。它只是高性能数据传输基础设施。

---

## 4. GPUDirect RDMA

如果数据来自另一台服务器，路径通常是：

```text
GPU 0
  ↓
本机网卡
  ↓ RDMA
远端网卡
  ↓
远端 GPU 1
```

传统路径可能是：

```text
GPU 0
  → CPU 内存
  → 网卡
  → 网络
  → 远端 CPU 内存
  → GPU 1
```

GPUDirect RDMA 则允许网卡直接读写 GPU 显存：

```text
GPU 显存
  ↔ 网卡 DMA
  ↔ InfiniBand / RoCE
  ↔ 远端网卡 DMA
  ↔ 远端 GPU 显存
```

这减少：

- CPU 内存中转；
- CPU 拷贝；
- 内核网络路径；
- 上下文切换。

常见于：

- 多机训练；
- 分布式推理；
- Prefill/Decode 分离；
- GPU Direct Storage；
- 分布式 KV Cache 传输。

---

# 三、主流同步方式

数据链路和同步链路通常分开考虑。

## 1. 同一个 CUDA Stream

如果 GPU 0 上：

```text
Stream 0:
  Kernel A 写 Buffer X
  P2P Copy X → GPU 1
```

那么同一个 Stream 通常保证：

```text
Kernel A 完成后，P2P Copy 才开始
```

GPU 1 上：

```text
Stream 1:
  等待数据到达
  Kernel B 读取 X
```

---

## 2. CUDA Event

跨 Stream 或跨 GPU 时，可以使用 Event：

```text
GPU 0:
  Kernel A 写 X
  Record Event E

GPU 1:
  Wait Event E
  Kernel B 读取 X
```

抽象代码：

```cpp
kernelA<<<..., stream0>>>(..., bufferA);

cudaEventRecord(doneA, stream0);

cudaStreamWaitEvent(stream1, doneA);
kernelB<<<..., stream1>>>(bufferA, ...);
```

Event 的作用是：

```text
建立先后关系
保证前一个操作完成
避免下一个 Kernel 读取未完成的数据
```

它不是数据搬运机制。

---

## 3. CUDA Graph 依赖边

如果跨 GPU 的工作流比较固定，可以放入 Graph：

```text
[GPU 0 Kernel A]
       ↓
[GPU 0 P2P Copy]
       ↓
[GPU 1 Kernel B]
```

Graph 会记录：

```text
节点
节点之间的依赖
Stream
Buffer 地址
Kernel 参数
Memcpy 操作
```

Replay 时，GPU 按依赖执行。

不过动态多卡推理很难完全固定，因为：

- Batch 会变化；
- 请求会加入或退出；
- Buffer 地址可能变化；
- KV Page 数量会变化；
- 不同请求通信量不同。

所以实际框架通常使用多个 Graph 模板，或者只对固定阶段使用 CUDA Graph。

---

## 4. NCCL 通信同步

NCCL 是多 GPU 通信的主流库，负责：

- All-Reduce；
- All-Gather；
- Reduce-Scatter；
- Broadcast；
- Send/Recv；
- 多机 RDMA 通信。

例如 Tensor Parallel 中：

```text
GPU 0：
  Y0 = X × W0

GPU 1：
  Y1 = X × W1

GPU 2：
  Y2 = X × W2

GPU 3：
  Y3 = X × W3

All-Reduce / All-Gather
  ↓
得到完整结果
```

NCCL 不只是简单调用 `memcpy`，还会：

- 探测硬件拓扑；
- 选择 NVLink、NVSwitch、PCIe 或 RDMA；
- 构造通信 Ring 或 Tree；
- 安排多个 CUDA Stream；
- 通过 GPU Kernel 执行通信；
- 处理通信完成与同步。

---

# 四、常见多卡依赖模式

## 1. Broadcast

一个 GPU 产生数据，多个 GPU 读取：

```text
GPU 0：权重或输入 X
  ├──→ GPU 1
  ├──→ GPU 2
  └──→ GPU 3
```

适合：

- 广播模型权重；
- 广播控制信息；
- 数据并行开始时同步输入。

通常使用：

```text
NCCL Broadcast
```

---

## 2. All-Reduce

每个 GPU 都有部分结果，最终每个 GPU 都得到完整聚合结果：

```text
GPU 0：Y0
GPU 1：Y1
GPU 2：Y2
GPU 3：Y3

All-Reduce：
  Y = Y0 + Y1 + Y2 + Y3

每张 GPU 都得到 Y
```

Tensor Parallel 中经常使用。

常见实现路径：

```text
Reduce-Scatter
  ↓
交换部分结果
  ↓
All-Gather
```

而不是让所有 GPU 直接把完整数据发给所有 GPU。

---

## 3. All-Gather

每张 GPU 持有一部分数据，最终每张 GPU 都获得完整数据：

```text
GPU 0：A0
GPU 1：A1
GPU 2：A2
GPU 3：A3

All-Gather：

GPU 0：A0 A1 A2 A3
GPU 1：A0 A1 A2 A3
GPU 2：A0 A1 A2 A3
GPU 3：A0 A1 A2 A3
```

适合：

- 收集张量分片；
- Pipeline 阶段之间交换激活；
- MoE 路由后的结果合并。

---

## 4. Reduce-Scatter

每张 GPU 都参与计算，但最终只保留聚合结果的一部分：

```text
所有 GPU 共同聚合
  ↓
GPU 0 保留结果片段 0
GPU 1 保留结果片段 1
GPU 2 保留结果片段 2
GPU 3 保留结果片段 3
```

相比 All-Reduce，它可以减少每张 GPU 最终保存的数据量。

---

## 5. Send/Recv

一张 GPU 生成数据，另一张 GPU 消费：

```text
GPU 0：
  Layer 0-15
  ↓ 激活
GPU 1：
  Layer 16-31
```

这是 Pipeline Parallel 的典型模式。

流程类似：

```text
GPU 0 Kernel A 写 Activation X
  ↓
P2P/NVLink Send X
  ↓
GPU 1 接收 Buffer Y
  ↓
GPU 1 Kernel B 读取 Y
```

---

# 五、一个完整的跨 GPU Buffer 依赖例子

假设 GPU 0 和 GPU 1 执行两段模型：

```text
GPU 0：
  H0 → Layer 0-15 → H1

GPU 1：
  H1 → Layer 16-31 → H2
```

完整数据流：

```text
1. GPU 0 Kernel A 读取 H0
2. GPU 0 Kernel A 写入 H1
3. GPU 0 记录完成 Event E
4. GPU 0 通过 NVLink/PCIe 将 H1 发送到 GPU 1
5. GPU 1 等待 Copy 完成
6. GPU 1 Kernel B 读取 H1
7. GPU 1 写入 H2
```

可以表示为：

```text
GPU 0:
  [Kernel A]
      │
      ▼
  [P2P Copy]
      │
      ▼
GPU 1:
  [Kernel B]
```

这里至少有三个依赖：

```text
Kernel A → P2P Copy
P2P Copy → Kernel B
Kernel B → 后续 GPU 1 操作
```

任何一个依赖缺失，都可能导致数据竞争。

---

# 六、跨 GPU 访问与显式复制的区别

有两种方式。

## 1. 远端直接访问

GPU 1 Kernel 直接读取 GPU 0 的远端显存：

```text
GPU 1 Kernel
  → NVLink
  → GPU 0 VRAM
```

优点：

```text
少一次显式 Copy
代码路径简单
```

缺点：

```text
每次远端访问都经过互连
访问延迟较高
访问模式难以优化
可能占用链路带宽
```

适合：

- 少量共享数据；
- 权重只读访问；
- 元数据；
- 不值得显式复制的数据。

## 2. 显式 P2P Copy

先把数据复制到本地：

```text
GPU 0 VRAM
  ↓ P2P Copy
GPU 1 VRAM 本地 Buffer
  ↓
GPU 1 Kernel 高频读取
```

优点：

```text
后续访问变成本地显存访问
适合重复使用的大数据
```

缺点：

```text
需要额外一次传输
占用显存
需要显式管理 Buffer 生命周期
```

通常的原则是：

```text
一次性或少量访问：远端直接读
重复高频访问：复制到本地
```

---

# 七、GPU 内存一致性需要谨慎理解

多卡系统并不一定提供“所有显存都像一个完全一致的共享内存”。

需要区分：

```text
地址可访问
数据传输完成
写入对另一 GPU 可见
缓存已失效或已更新
```

例如 GPU 0 写了 Buffer X，GPU 1 不能仅因为知道 X 的地址，就立即安全读取。必须确保：

```text
GPU 0 写入完成
传输完成
相关 Event/Fence 已满足
GPU 1 Kernel 之后启动
```

因此高性能通信库通常会在传输和同步中处理：

- DMA 完成；
- Stream 顺序；
- Memory Fence；
- Cache 可见性；
- 通信协议状态。

---

# 八、不同场景的典型选择

| 场景 | 常见机制 |
|---|---|
| 同卡 Kernel 间依赖 | 同一 CUDA Stream、Event、CUDA Graph |
| 两张同机 GPU | P2P、PCIe、NVLink |
| 多张高端 GPU | NVLink + NVSwitch |
| Tensor Parallel | NCCL All-Reduce、Reduce-Scatter、All-Gather |
| Pipeline Parallel | P2P Send/Recv、NVLink/PCIe |
| MoE Expert Parallel | NCCL All-to-All |
| 多机多卡 | InfiniBand/RoCE + GPUDirect RDMA |
| Prefill/Decode 分离 | P2P、NVLink、RDMA、KV Cache Transfer |
| GPU 与 NVMe 直传 | GPUDirect Storage |
| 稀疏共享数据 | 远端显存直接访问 |
| 高频重复数据 | P2P 复制到本地显存 |

---

# 九、和 Kubernetes 节点的关系

在 Kubernetes 中，多卡通信通常受设备拓扑影响。

Device Plugin 负责把 GPU 暴露给 Pod：

```text
nvidia.com/gpu: 4
```

但真正决定性能的还包括：

```text
Pod 是否拿到同一台节点上的 GPU
GPU 是否支持 NVLink
GPU 是否位于同一 NVSwitch Fabric
是否跨 NUMA
是否能使用 RDMA
是否启用 GPUDirect
```

因此仅仅调度：

```yaml
resources:
  limits:
    nvidia.com/gpu: 4
```

并不等于获得理想的多 GPU 通信性能。

生产环境还需要关注：

- GPU 拓扑感知调度；
- NVIDIA GPU Operator；
- NCCL 拓扑发现；
- RDMA Device Plugin；
- Node Feature Discovery；
- MIG 划分；
- PCIe NUMA 亲和性；
- 网络和 GPU 的 GPUDirect RDMA 支持。

---

# 十、最核心的结论

多卡 Buffer 依赖通常不是靠单一技术解决，而是分成四层：

```text
1. Buffer 所在位置：
   GPU 0 显存、GPU 1 显存、Host RAM、远端节点

2. 数据传输路径：
   PCIe P2P、NVLink、NVSwitch、RDMA

3. 执行依赖：
   CUDA Stream、Event、Fence、CUDA Graph

4. 集体通信算法：
   NCCL Ring、Tree、All-Reduce、All-Gather、All-to-All
```

可以把主流架构概括为：

```text
GPU 0 Kernel 写 Buffer
  ↓
CUDA Stream 保证计算完成
  ↓
P2P / NVLink / NVSwitch / RDMA 传输
  ↓
通信完成事件
  ↓
GPU 1 Kernel 读取 Buffer
```

其中：

```text
NVLink：
  高速 GPU-GPU 链路

NVSwitch：
  多 GPU 交换网络

PCIe P2P：
  通用但带宽和拓扑通常较弱

GPUDirect RDMA：
  GPU 显存直接参与跨节点网络 DMA

NCCL：
  将这些硬件链路组织成高效集体通信

CUDA Event/Stream/Graph：
  保证数据依赖和执行顺序
```

所以，NVLink 和 NVSwitch 解决的是“数据怎么快地过去”，NCCL 解决的是“多张卡如何协同通信”，而 Stream、Event 和 Graph 解决的是“什么时候可以安全地读取”。