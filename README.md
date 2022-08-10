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
consensus 6.6.0
Rivtower Technologies <contact@rivtower.com>
This doc string acts as a help message when the user runs '--help' as do all doc strings on fields

USAGE:
    consensus <SUBCOMMAND>

OPTIONS:
    -h, --help       Print help information
    -V, --version    Print version information

SUBCOMMANDS:
    help    Print this message or the help of the given subcommand(s)
    run     run this service
```

### consensus-run

运行`consensus`服务。

```
$ consensus run -h
consensus-run 
run this service

USAGE:
    consensus run [OPTIONS]

OPTIONS:
    -c, --config <CONFIG_PATH>    Chain config path [default: config.toml]
    -h, --help                    Print help information
    -l, --log <LOG_FILE>          log config path [default: consensus-log4rs.yaml]

```

参数：
1. 微服务配置文件。

   参见示例`example/config.toml`。

   其中：
    * `consensus_port` 为该服务监听的端口号。
2. 日志配置文件。

   参见示例`consensus-log4rs.yaml`。

   其中：

    * `level` 为日志等级。可选项有：`Error`，`Warn`，`Info`，`Debug`，`Trace`，默认为`Info`。
    * `appenders` 为输出选项，类型为一个数组。可选项有：标准输出(`stdout`)和滚动的日志文件（`journey-service`），默认为同时输出到两个地方。

```
$ consensus run -c example/config.toml -l consensus-log4rs.yaml
2022-04-28T02:33:22.839993245+00:00 INFO consensus - start consensus overlord
2022-04-28T02:33:22.840116114+00:00 INFO consensus - grpc port of this service: 50001
```
