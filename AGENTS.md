# Consensus Overlord Agent

## 项目基本信息
- **项目名称**: Consensus Overlord
- **项目描述**: CITA-Cloud 区块链平台的共识微服务实现，基于 [Overlord](https://github.com/nervosnetwork/overlord) 共识算法。
- **许可证**: Apache-2.0
- **开发语言**: Rust (Edition 2024)
- **维护者**: Rivtower Technologies

## 核心能力
1. **共识服务**:
   - 实现了 CITA-Cloud 的 `ConsensusService` gRPC 接口。
   - 负责处理区块的共识流程，确保分布式节点间的数据一致性。
   - 支持动态配置更新 (`reconfigure`)。

2. **Overlord 算法集成**:
   - 采用 Overlord 算法作为核心共识引擎，解耦了共识逻辑与状态机。
   - 支持高效的 BFT (Byzantine Fault Tolerance) 共识。

3. **微服务交互**:
   - **Network**: 通过 gRPC 与网络微服务交互，广播和接收共识消息。
   - **Controller**: 与控制器微服务交互，提交 Proposal 和验证区块。

4. **可观测性**:
   - **Metrics**: 集成 Prometheus 指标采集，支持可配置的 Metrics 端口和 Buckets。
   - **Tracing/Logging**: 使用 `tracing` 和 `cloud-util` 实现结构化日志和分布式追踪 (Jaeger)。
   - **Health Check**: 提供 gRPC 健康检查接口。

5. **持久化与安全**:
   - **WAL (Write Ahead Log)**: 使用预写日志保证共识状态的持久化和故障恢复。
   - **Crypto**: 集成 `ophelia` 系列库进行加密操作。

## 运行依赖
### 系统环境
- **操作系统**: Linux (推荐 Debian/Ubuntu，Docker 镜像基于 Debian Bullseye)
- **构建依赖**:
  - Rust Toolchain (最新稳定版)
  - `protobuf-compiler` (用于编译 gRPC proto 文件)
  - `make`, `gcc`, `g++` 等基础构建工具

### 微服务依赖
此服务作为 CITA-Cloud 的一部分，通常需要配合以下微服务运行：
- **Network Microservice**: 用于节点间通信。
- **Controller Microservice**: 用于处理交易和区块组装。

## 使用示例

### 1. 编译构建
```bash
# 本地构建
cargo build --release

# Docker 构建
docker build -t citacloud/consensus_overlord .
```

### 2. 配置文件 (config.toml)
```toml
# 示例配置片段
[consensus_overlord]
network_port = 50000
consensus_port = 50001
controller_port = 50004
wal_path = "overlord_wal"
enable_metrics = true
metrics_port = 60001
domain = "node1"

[consensus_overlord.log_config]
max_level = "info"
service_name = "consensus"
rolling_file_path = "logs/consensus.log"
```

### 3. 启动服务
```bash
# 命令行启动
./target/release/consensus run \
    -c example/config.toml \
    -p example/private_key

# 参数说明
# -c, --config <CONFIG_PATH>: 配置文件路径 (默认: config.toml)
# -p, --private_key_path <PRIVATE_KEY_PATH>: 私钥文件路径 (默认: private_key)
```

### 4. 常用命令
```bash
# 查看帮助
./target/release/consensus --help

# 查看版本
./target/release/consensus --version
```
