不完全是。更准确地说：

```text
GPU 编排通常会尽量稳定 Buffer 的基地址，
但不意味着所有地址都能在执行前完全固定，
也不意味着 GPU 没有动态分支或动态索引。
```

## 1. CUDA Graph 中地址通常要求稳定

对于 CUDA Graph，常见做法是提前分配：

```text
输入 Buffer
权重 Buffer
中间结果 Buffer
KV Cache Pool
输出 Buffer
Metadata Buffer
```

例如：

```text
X      = 0x10000000
W      = 0x20000000
Hidden = 0x30000000
KV     = 0x40000000
Output = 0x50000000
```

Graph 记录：

```text
Kernel A: X + W → Hidden
Kernel B: Hidden → KV
Kernel C: Hidden + KV → Output
```

Replay 时通常复用相同地址，只更新这些地址中的内容：

```text
Metadata Buffer 更新
输入 Buffer 更新
重新 Replay Graph
```

这样 GPU 不需要在每轮执行中重新分配大量内存。

## 2. 但“固定基地址”不等于“所有访问地址固定”

Kernel 内部经常动态计算地址：

```cpp
value = buffer[thread_id];
value = buffer[page_table[request_id] * page_size + offset];
```

实际访问地址是：

```text
base_address
  + page_id * page_size
  + token_offset
  + head_offset
```

例如 Paged KV Cache：

```text
逻辑 token 100
  ↓
page_id = page_table[request][100 / page_size]
  ↓
physical_address =
    kv_cache_base
    + page_id * page_bytes
    + (100 % page_size) * token_bytes
```

因此：

```text
KV Cache Pool 的基地址可以固定
每个请求映射到哪些 Page 可以动态变化
每个线程访问哪个元素也可以动态计算
```

这和操作系统页表很像：

```text
物理内存池地址稳定
虚拟页到物理页的映射动态变化
```

## 3. GPU 仍然有 `if` 和循环

GPU Kernel 内部完全可以有：

```cpp
for (...) {
    ...
}

if (valid_token) {
    ...
}
```

例如 Attention Kernel：

```cpp
for (int token = 0; token < seq_len; token++) {
    int page_id = page_table[request_id][token / page_size];

    if (token < actual_seq_len) {
        load_kv(page_id, token % page_size);
    }
}
```

区别在于：

```text
CPU：
  一个线程顺序执行复杂控制流

GPU：
  大量线程同时执行类似控制流
```

GPU 更擅长规则化、并行化的分支；复杂且高度不规则的分支可能导致 Warp Divergence：

```text
Warp 中部分线程进入 if
另一部分线程不进入
  ↓
GPU 可能依次执行两条路径
```

所以框架和 Kernel 会尽量把动态逻辑转化为：

```text
Metadata
  + mask
  + page table
  + valid length
  + padding
```

而不是频繁改变整个执行结构。

## 4. GPU 也可以动态分配内存

GPU 并非不能动态产生新地址。常见方式包括：

```text
cudaMalloc
cudaMallocAsync
CUDA Memory Pool
设备端 malloc
框架自己的显存 Block Allocator
```

但是动态分配通常较慢，且可能破坏 CUDA Graph 的稳定性。

因此推理框架更常见的方式是：

```text
启动时预分配大块显存
  ↓
框架内部切分成 Block/Page
  ↓
请求到达时只分配逻辑 Page
  ↓
请求结束时回收 Page
```

例如：

```text
KV Pool:
  [Page 0][Page 1][Page 2][Page 3]...

Request A → [Page 2][Page 9]
Request B → [Page 1][Page 7][Page 8]
```

这里并没有频繁调用底层显存分配器，而是由框架修改 Page Table。

## 5. CUDA Graph 对动态性的限制

CUDA Graph 更适合：

```text
Kernel 数量基本稳定
Buffer 地址稳定
Tensor shape 稳定
控制流结构稳定
```

如果发生以下情况，可能无法直接复用同一个 Graph：

```text
Batch Size 变化
Tensor Shape 变化
新增或删除 Kernel
重新分配 Buffer
不同的 Kernel 参数布局
```

常见解决方案是：

```text
捕获多个 Graph
  Graph-1：batch 1
  Graph-2：batch 2
  Graph-4：batch 4
  Graph-8：batch 8
```

运行时选择合适的 Graph，并对无效位置进行 padding。

所以它不是：

```text
所有运行情况共用一个绝对固定的 Graph
```

而是：

```text
把动态情况离散成有限组稳定模板
```

## 6. 不同 Buffer 的稳定程度不同

### 权重 Buffer

通常最稳定：

```text
模型加载后常驻显存
地址和布局长期不变
```

### KV Cache Pool

池子的基地址通常稳定，但：

```text
请求对应的 Page 会动态分配
Page Table 会动态更新
有效长度会动态变化
```

### 激活值 Buffer

通常预分配多个工作区：

```text
workspace_0
workspace_1
workspace_2
```

通过双缓冲或环形缓冲复用：

```text
Kernel A → Buffer 0
Kernel B → Buffer 1
下一轮 → 交换
```

### 临时 Workspace

根据 Kernel 需要分配，通常从预先分配的 workspace 中切片。

## 7. 一个更准确的地址模型

GPU 推理通常不是：

```text
所有具体元素地址提前写死
```

而是：

```text
固定的资源池基地址
+
运行时索引和偏移
+
稳定的 Kernel 参数布局
+
显式的执行依赖
```

可以表示为：

```text
物理/虚拟 Buffer Base
  ↓
Page Table / Slot Mapping
  ↓
Request Index
  ↓
Token Offset
  ↓
实际访问地址
```

例如：

```text
actual_address =
    kv_pool_base
    + page_table[request_id][page_index] * page_size
    + in_page_offset
```

基地址固定，地址映射和偏移动态。

## 8. 和 CPU 程序的核心区别

CPU 程序常见：

```cpp
for (...) {
    auto* p = malloc(size);
    if (...) {
        ...
    }
}
```

每次循环可能发生：

```text
动态分配
指针变化
分支变化
函数调用变化
```

GPU 高性能执行更倾向于：

```cpp
Buffer* pool = preallocated_pool;

for (...) {
    int slot = slot_table[index];

    if (valid[index]) {
        pool[slot] = ...
    }
}
```

也就是：

```text
预分配资源池
运行时改变索引、长度和有效位
尽量不改变整体执行结构
```

但这不是 GPU 的硬性限制，而是为了：

- 减少分配开销；
- 提高显存复用；
- 适配 CUDA Graph；
- 保持 Kernel 高吞吐；
- 减少 CPU 和 Driver 介入。

## 结论

你的理解需要改成：

```text
GPU 编排通常提前固定主要 Buffer 的基地址和内存布局，
但不会固定每一次实际访问的具体地址。
```

更完整地说：

```text
固定：
  权重地址
  显存池地址
  工作区地址
  CUDA Graph 中的资源关系
  Kernel 间依赖关系

动态：
  Page Table
  Slot Mapping
  Token 数量
  请求数量
  实际访问偏移
  有效数据范围
  Kernel 内部 if/for
  多卡通信目标
```

因此主流 GPU 推理框架的典型设计是：

```text
静态显存池
  + 动态索引
  + 动态 Metadata
  + 有限数量的执行模板
  + GPU Kernel 内部并行分支
```

不是“所有地址都提前写死”，而是把昂贵的内存分配和执行结构尽量提前固定，把运行期变化压缩为少量的索引、长度、页表和参数更新。