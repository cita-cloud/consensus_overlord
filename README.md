# consensus_overlord
`CITA-Cloud`中[consensus微服务](https://github.com/cita-cloud/cita_cloud_proto/blob/master/protos/consensus.proto)的实现。

基于[overlord算法](https://github.com/nervosnetwork/overlord)实现。

## 编译docker镜像
```
docker build -t citacloud/consensus_overlord .
```

## 使用方法

```
$ consensus -h       
This doc string acts as a help message when the user runs '--help' as do all doc strings on fields

Usage: consensus <COMMAND>

Commands:
  run   run this service
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### consensus-run

运行`consensus`服务。

```
$ consensus run -h
run this service

Usage: consensus run [OPTIONS]

Options:
  -c, --config <CONFIG_PATH>                 Chain config path [default: config.toml]
  -p, --private_key_path <PRIVATE_KEY_PATH>  private key path [default: private_key]
  -h, --help                                 Print help
```

参数：
1. 微服务配置文件。

   参见示例`example/config.toml`。

   其中`[consensus_overlord]`段为微服务的配置：
    * `consensus_port` 为该服务监听的端口号。
    * `domain` 节点的域名
    * `network_port` 网络微服务的gRPC端口
    * `controller_port` 控制器微服务的gRPC端口
    * `metrics_port` 是metrics信息的导出端口
    * `enable_metrics` 是metrics功能的开关
    * `node_address` 节点地址文件路径

    其中`[consensus_overlord.log_config]`段为微服务日志的配置：
    * `max_level` 日志等级
    * `filter` 日志过滤配置
    * `service_name` 服务名称，用作日志文件名与日志采集的服务名称
    * `rolling_file_path` 日志文件路径
    * `agent_endpoint` jaeger 采集端地址

```
$ consensus run -c example/config.toml -p example/private_key
2023-02-08T06:20:57.500676Z  INFO consensus: grpc port of consensus_overlord: 50001
```
