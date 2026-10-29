# qq-wechat-cli

在一个命令行中查询本机 QQ 和微信记录。TypeScript 负责命令与流式导出，Rust 负责密钥读取、快照、独立解密、解析和持久 SQLite 索引。

首版面向 **Windows x64、Node.js 24**，支持 Classic QQ **9.7.25.29417** 和迁移基线的微信 4.x 数据库格式。安装包附带核心程序；使用时只需 Node.js。SQLite、AES、XXTEA 和 zstd 随核心交付。

## 安装与迁移

通过 npm 安装：

```powershell
npm install -g qq-wechat-cli
qq-wechat-cli --help
```

也可下载 [GitHub Release 压缩包](https://github.com/Chihiro521/qq-wechat-cli/releases/download/v0.1.0/qq-wechat-cli-0.1.0.tgz)，执行 `npm install -g .\qq-wechat-cli-0.1.0.tgz` 安装。

源码与发行包：[GitHub](https://github.com/Chihiro521/qq-wechat-cli)、[Releases](https://github.com/Chihiro521/qq-wechat-cli/releases)、[npm](https://www.npmjs.com/package/qq-wechat-cli)。

旧工具与旧配置继续独立存在。显式导入旧配置后，新工具会建立自己的缓存和索引：

```powershell
qq-wechat-cli qq import
qq-wechat-cli wechat import
# 也可明确指定旧配置
qq-wechat-cli migrate --from wechat --config "C:\path\config.json"
```

QQ 默认读取 `%LOCALAPPDATA%\qq-cli\config.json`，微信默认读取 `%USERPROFILE%\.wechat-cli\config.json`。旧环境变量 `QQ_CLI_CONFIG`、`QQ_CLI_HOME`、`WECHAT_CLI_CONFIG` 仍可定位导入来源。`import --dry-run` 只检查配置位置。

也可直接初始化：

```powershell
qq-wechat-cli qq init --db "D:\QQ-data\123456\Msg3.0.db" --account 123456
qq-wechat-cli wechat init --db-dir "D:\xwechat_files\wxid_example\db_storage"
```

初始化加密数据时，保持对应客户端登录。QQ 会核对客户端版本及目标数据库上下文；微信会用目标页面验证找到的密钥。密钥缓存使用当前 Windows 用户的 DPAPI 封装。`--keys-file` 可显式提供已有密钥文件；这类文件不会被复制进项目或安装包。

微信自动取钥还取决于客户端是否已装载目标库。已有旧配置时，优先使用显式 `wechat import` 复用并验证密钥；仅登录但尚未装载部分库时，首次自动取钥可能返回 `KEY_NOT_FOUND`。

默认状态目录为 `%LOCALAPPDATA%\qq-wechat-cli`，可用 `--data-dir DIR` 或 `QQ_WECHAT_HOME` 指定。多个账号共用索引时，用 `--account ID` 选择；平台与账号的消息标识相互隔离。

## 常用命令

```powershell
qq-wechat-cli qq status
qq-wechat-cli wechat refresh
qq-wechat-cli qq sessions --limit 20
qq-wechat-cli qq history group:123456 --limit 50
qq-wechat-cli wechat history "群名称" --start-time 2026-10-01 --end-time 2026-10-29
qq-wechat-cli qq search "关键词" --chat group:123456 --type text
qq-wechat-cli wechat search "关键词" --days 7 --refresh
qq-wechat-cli wechat contacts --query "名字"
qq-wechat-cli wechat contacts --detail wxid_example
qq-wechat-cli qq members group:123456
qq-wechat-cli wechat favorites --query "文章"
qq-wechat-cli qq stats group:123456 --top 10
qq-wechat-cli qq unread
qq-wechat-cli wechat new-messages --limit 500
qq-wechat-cli qq summarize group:123456 --chunk-size 80 --limit 500
```

普通查询直接读取缓存，结果的 `meta.syncedAt` 显示最近成功同步时间。`refresh` 或查询加 `--refresh` 获取最新数据；`--force` 用于完整重新解析。未变化的数据源会跳过全库复制、解密和索引重建。

首次登录客户端后，等待客户端补齐消息再刷新。首次建立索引期间源库仍可能更新，可再执行一次 `refresh` 复查。`syncedAt` 表示处理完成时间，导出范围可通过带时区的时间参数明确指定，例如 `--start-time "2026-10-30T00:00:00+08:00"`。

输出默认 JSON；`--format text` 显示文字。查询参数包括 `--limit`、`--offset`、`--before` 游标、`--order asc|desc`、`--start-time`、`--end-time`、`--days`、`--type`、`--sender`。搜索可重复使用 `--chat` 或 `--type`；名称有歧义时用会话 ID。日期按本机时区理解，消息同时保留 Unix 秒与 ISO 时间。

三字符及以上的子串搜索使用 FTS5 trigram，再核对正文；一、二字符仍作完整子串匹配，可能扫描更多记录。`--media` 额外解析本机附件引用；附件实体仍存放在原客户端目录。

QQ 联系人与成员来自已有消息记录；微信来自客户端辅助数据库。QQ `unread` 是本工具的检查点未读，微信 `unread` 是最后同步时的客户端未读。`new-messages` 首次建立基线，之后只推进实际返回消息的检查点。`summarize` 输出带消息引用的本地总结素材。

## 导出与清理

```powershell
qq-wechat-cli qq export group:123456 --format chatlab --output "D:\exports\qq.jsonl"
qq-wechat-cli wechat export "群名称" --format jsonl --output "D:\exports\wechat.jsonl"
qq-wechat-cli qq export group:123456 --format markdown --output history.md
qq-wechat-cli qq clean
qq-wechat-cli qq clean --all
```

导出支持 `markdown`、`txt` / `text`、`jsonl` 和 ChatLab JSONL 0.0.2。默认导出所选范围全部消息，可用 `--limit` 限制。按行传输并处理写入背压，避免把整段聊天载入内存。ChatLab 成员先于消息输出。已有目标需加 `--force`；文件写入完成后发布，源库、缓存及索引路径会被核对。普通 `clean` 清除该账号的缓存与索引数据，保留配置；`--all` 同时移除账号配置。

当前 `clean` 删除索引记录后不会自动压缩 SQLite 文件；`native/target/` 等开发构建缓存另行清理。

## 刷新实现

- 微信独立按页解密并验证页面 HMAC，解析 zstd 与消息 XML；同一代 WAL 校验盐值、链式校验和及提交边界，仅应用已提交的新页面。WAL 重置、checkpoint、截断或基库变化时重建受影响分片。
- QQ 独立还原 1024 字节扩展头后的 8192 字节页，以小端 XXTEA 解密；通过密文页摘要复用未变化的明文页。源文件变化时仍需完整读取和快照复制，后续可跳过未变页面的解密。大于 1 GB 的 PCQQ 数据库采用其实际锁页位置，统一查询索引保持标准 SQLite 格式。
- 源文件复制采用有界只读内存映射，保留文件锁覆盖的原始字节。必要时由单独看护短暂停顿已验证的客户端，每次刷新累计最多 5 秒；复制后立即恢复。超时和工作进程退出都会触发恢复。
- 新副本通过 SQLite 校验后，在事务中更新变化记录、删除已消失记录并发布同步状态。失败或取消保留上次成功结果；同一账号并发刷新会返回 `REFRESH_BUSY`。

算法与对照依据见 [QQ-CODEC.md](research/QQ-CODEC.md)，本机测量见 [性能与验收](docs/validation.md)。SQLite 官方参考：[FTS5 trigram](https://sqlite.org/fts5.html#the_trigram_tokenizer)、[WAL 格式](https://sqlite.org/fileformat2.html#walformat)。

## 开发

开发环境额外需要 Rust stable、MSVC C++ Build Tools 与 npm：

```powershell
npm ci
npm run build
npm run test:rust
npm test
npm pack --pack-destination artifacts
```

TS 与 Rust 使用版本 1 的 NDJSON 标准输入输出协议；进度写入标准错误，结果、错误及流式行分别传递。每个业务命令启动一个核心进程。`QQ_WECHAT_ENGINE` 可在开发时指定核心路径。构建采用静态 CRT。

`native/Cargo.lock`、`package-lock.json` 固定开发依赖。测试夹具仅含合成记录与合成密钥；`research/` 中的旧工具 / DLL 对照脚本仅供开发，发行包采用文件白名单，仅含编译后的 TS、Rust 核心与说明和许可文件。真实记录、账号状态、Python 依赖目录、目标编译目录均被排除。

项目原有 QQ 与新增代码沿用 UNLICENSED 声明；迁入的 Apache 许可部分保留原声明，见 LICENSE、NOTICE 与 LICENSE-APACHE-2.0。
